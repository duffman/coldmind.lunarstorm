//! Asset store, custom protocol responses, and the release archive (spec 4 ARCHIVE, 6).
//!
//! - **Debug:** the store lives in memory and the embedded pipeline refills it.
//! - **Release:** the store is read from the archive attached to the runtime.
//!
//! Archive layout (`.zebra`):
//!
//! ```text
//! "ZEBRAARC" | u64 LE manifest length | manifest JSON | blob region
//! ```
//!
//! The manifest holds the host configuration, the artifact kind and engine version, and
//! for every file its offset and length in the blob region and its SHA-256.
//!
//! Where the archive sits next to a release runtime:
//! - macOS: `Contents/Resources/app.zebra` in the bundle (a bundle resource, signed with it);
//! - Linux: `<executable>.zebra`, a sibling file;
//! - Windows: appended to the executable as an overlay ending in a trailer
//!   (`u64 LE length | "ZEBRATRL"`).
//!
//! Decision (Windows): the spec names a resource section. Writing PE resources onto a
//! prebuilt executable needs a resource compiler or a PE rewriter; an overlay is read the
//! same way, survives Authenticode signing (the certificate table goes after it), and
//! needs no tool on the developer's machine. Dropped: rewriting `.rsrc`.

use crate::manifest::HostConfig;
use crate::pipeline::BuildOutput;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Archive magic.
pub const MAGIC: &[u8; 8] = b"ZEBRAARC";

/// Windows overlay trailer magic.
pub const TRAILER: &[u8; 8] = b"ZEBRATRL";

/// Kind of the backend artifact stored in the archive.
pub const ARTIFACT_KIND: &str = "es-module-source";

/// Engine the backend artifact is built for. A release runtime refuses an archive built
/// for another engine version.
pub const ENGINE: &str = "quickjs/rquickjs-0.14";

/// Shared, swappable map of frontend files served by the custom protocol.
#[derive(Clone, Default)]
pub struct AssetStore {
    /// path (no leading slash) -> bytes.
    files: Arc<RwLock<BTreeMap<String, Arc<[u8]>>>>,
}

impl AssetStore {
    /// Replaces every file at once (a new build).
    ///
    /// # Arguments
    /// * `files` - the new frontend files.
    pub fn replace(&self, files: BTreeMap<String, Arc<[u8]>>) {
        *self.files.write().expect("asset store lock") = files;
    }

    /// Reads one file.
    ///
    /// # Arguments
    /// * `path` - path without a leading slash.
    ///
    /// # Returns
    /// The bytes, if present.
    pub fn get(&self, path: &str) -> Option<Arc<[u8]>> {
        self.files.read().expect("asset store lock").get(path).cloned()
    }

    /// Answers one custom-protocol request.
    ///
    /// Decision: every response carries a CSP that only allows the asset origin, so a
    /// generated page cannot load or post to anything else even if navigation is locked.
    ///
    /// # Arguments
    /// * `path` - URL path, with or without the leading slash; a query is ignored.
    ///
    /// # Returns
    /// The HTTP response.
    pub fn respond(&self, path: &str) -> wry::http::Response<Cow<'static, [u8]>> {
        let path = path.split('?').next().unwrap_or("").trim_start_matches('/');
        let path = if path.is_empty() { "index.html" } else { path };
        let builder = wry::http::Response::builder()
            .header("Content-Security-Policy", "default-src 'self' 'unsafe-inline' data: blob:; connect-src 'self'")
            .header("Cache-Control", "no-store");
        match self.get(path) {
            Some(bytes) => builder
                .status(200)
                .header("Content-Type", mime(path))
                .body(Cow::Owned(bytes.to_vec()))
                .expect("static response"),
            None => builder.status(404).body(Cow::Borrowed(&b"not found"[..])).expect("static response"),
        }
    }
}

/// MIME type by extension.
///
/// # Arguments
/// * `path` - file path.
///
/// # Returns
/// The content type.
pub fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// One file's row in the archive manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    /// File path; `backend.js` is the VM artifact.
    path: String,
    /// Offset in the blob region.
    offset: u64,
    /// Length in bytes.
    len: u64,
    /// Lowercase hex SHA-256.
    sha256: String,
}

/// The archive manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    /// Archive format version.
    version: u32,
    /// Artifact kind; see [`ARTIFACT_KIND`].
    artifact_kind: String,
    /// Engine; see [`ENGINE`].
    engine: String,
    /// Host configuration.
    host: HostConfig,
    /// Hash of `package.json` at build time.
    package_hash: String,
    /// Frontend files followed by the backend artifact.
    files: Vec<Entry>,
}

/// Hex SHA-256 of bytes.
///
/// # Arguments
/// * `bytes` - input.
///
/// # Returns
/// Lowercase hex digest.
pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Path of the backend artifact inside an archive.
const BACKEND: &str = "\0backend.js";

/// Writes a build into archive bytes.
///
/// # Arguments
/// * `build` - pipeline output.
///
/// # Returns
/// The archive.
pub fn write_archive(build: &BuildOutput) -> Vec<u8> {
    let mut blobs = Vec::new();
    let mut files = Vec::new();
    let mut push = |path: &str, bytes: &[u8]| {
        files.push(Entry { path: path.to_owned(), offset: blobs.len() as u64, len: bytes.len() as u64, sha256: sha256(bytes) });
        blobs.extend_from_slice(bytes);
    };
    for (path, bytes) in &build.files {
        push(path, bytes);
    }
    // Decision: the backend is stored under a name no frontend path can have (it starts
    // with NUL), so the custom protocol can never serve backend source to a WebView.
    push(BACKEND, build.backend.as_bytes());
    let manifest = Manifest {
        version: 1,
        artifact_kind: ARTIFACT_KIND.into(),
        engine: ENGINE.into(),
        host: build.config.clone(),
        package_hash: build.package_hash.clone(),
        files,
    };
    let manifest = serde_json::to_vec(&manifest).expect("manifest serializes");
    let mut out = Vec::with_capacity(16 + manifest.len() + blobs.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
    out.extend_from_slice(&manifest);
    out.extend_from_slice(&blobs);
    out
}

/// Reads and verifies archive bytes.
///
/// # Arguments
/// * `bytes` - the archive.
///
/// # Returns
/// The build it holds.
///
/// # Errors
/// Bad magic, truncation, an engine mismatch, or a hash mismatch.
pub fn read_archive(bytes: &[u8]) -> Result<BuildOutput, String> {
    if bytes.len() < 16 || &bytes[..8] != MAGIC {
        return Err("not a zebra archive".into());
    }
    let len = u64::from_le_bytes(bytes[8..16].try_into().expect("8 bytes")) as usize;
    let manifest_end = 16usize.checked_add(len).filter(|end| *end <= bytes.len()).ok_or("archive manifest is truncated")?;
    let manifest: Manifest = serde_json::from_slice(&bytes[16..manifest_end]).map_err(|e| format!("archive manifest: {e}"))?;
    if manifest.artifact_kind != ARTIFACT_KIND || manifest.engine != ENGINE {
        return Err(format!(
            "archive built for {} / {}, this runtime runs {ARTIFACT_KIND} / {ENGINE}",
            manifest.artifact_kind, manifest.engine
        ));
    }
    let blobs = &bytes[manifest_end..];
    let mut files = BTreeMap::new();
    let mut backend = None;
    for entry in manifest.files {
        let start = entry.offset as usize;
        let end = start.checked_add(entry.len as usize).filter(|e| *e <= blobs.len()).ok_or(format!("{} is truncated", entry.path))?;
        let data = &blobs[start..end];
        if sha256(data) != entry.sha256 {
            return Err(format!("{} fails its hash", entry.path));
        }
        if entry.path == BACKEND {
            backend = Some(String::from_utf8(data.to_vec()).map_err(|_| "backend artifact is not UTF-8")?);
        } else {
            files.insert(entry.path, Arc::from(data));
        }
    }
    let backend = backend.ok_or("archive has no backend artifact")?;
    Ok(BuildOutput { config: manifest.host, files, backend: Arc::from(backend), package_hash: manifest.package_hash })
}

/// Where a packaged runtime finds its archive, per OS.
///
/// # Arguments
/// * `exe` - path of the running executable.
///
/// # Returns
/// The archive bytes.
///
/// # Errors
/// No archive is attached, or it cannot be read.
pub fn read_attached(exe: &Path) -> Result<Vec<u8>, String> {
    if cfg!(windows) {
        let bytes = std::fs::read(exe).map_err(|e| format!("{}: {e}", exe.display()))?;
        return split_overlay(&bytes).map(<[u8]>::to_vec).ok_or_else(|| "no archive is attached to this executable".into());
    }
    let path = attached_path(exe);
    std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Path of the attached archive on macOS and Linux.
///
/// # Arguments
/// * `exe` - path of the executable.
///
/// # Returns
/// `Contents/Resources/app.zebra` inside a macOS bundle, else `<exe>.zebra`.
pub fn attached_path(exe: &Path) -> PathBuf {
    let macos_dir = exe.parent().filter(|d| d.ends_with("Contents/MacOS"));
    match macos_dir {
        Some(dir) => dir.parent().expect("Contents").join("Resources").join("app.zebra"),
        None => exe.with_extension("zebra"),
    }
}

/// Appends an archive to an executable as a Windows overlay.
///
/// # Arguments
/// * `exe` - executable bytes.
/// * `archive` - archive bytes.
///
/// # Returns
/// The executable with the overlay and trailer.
pub fn append_overlay(exe: &[u8], archive: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(exe.len() + archive.len() + 16);
    out.extend_from_slice(exe);
    out.extend_from_slice(archive);
    out.extend_from_slice(&(archive.len() as u64).to_le_bytes());
    out.extend_from_slice(TRAILER);
    out
}

/// Finds an overlay archive at the end of an executable.
///
/// # Arguments
/// * `bytes` - executable bytes.
///
/// # Returns
/// The archive slice, if a trailer is present.
pub fn split_overlay(bytes: &[u8]) -> Option<&[u8]> {
    // A signed executable has its certificate table after the overlay, so the trailer is
    // searched from the end rather than read at a fixed position.
    let window = bytes.len().saturating_sub(1 << 20);
    let at = bytes[window..].windows(8).rposition(|w| w == TRAILER)? + window;
    let len = u64::from_le_bytes(bytes.get(at.checked_sub(8)?..at)?.try_into().ok()?) as usize;
    let start = at.checked_sub(8)?.checked_sub(len)?;
    let archive = &bytes[start..at - 8];
    archive.starts_with(MAGIC).then_some(archive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::tests::sample_build;

    #[test]
    fn archive_round_trips_and_detects_tampering() {
        let build = sample_build();
        let bytes = write_archive(&build);
        let back = read_archive(&bytes).unwrap();
        assert_eq!(back.config, build.config);
        assert_eq!(back.files, build.files);
        assert_eq!(&*back.backend, &*build.backend);
        let mut tampered = bytes.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert!(read_archive(&tampered).unwrap_err().contains("hash"));
        assert!(read_archive(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn backend_is_never_served() {
        let build = read_archive(&write_archive(&sample_build())).unwrap();
        let store = AssetStore::default();
        store.replace(build.files);
        assert_eq!(store.respond("/index.html").status(), 200);
        assert_eq!(store.respond("/backend.js").status(), 404);
    }

    #[test]
    fn overlay_survives_trailing_signature_bytes() {
        let archive = write_archive(&sample_build());
        let mut exe = append_overlay(b"MZ fake executable", &archive);
        exe.extend_from_slice(&[0u8; 512]); // certificate table appended by signing
        assert_eq!(split_overlay(&exe), Some(&archive[..]));
        assert_eq!(split_overlay(b"MZ no overlay"), None);
    }

    #[test]
    fn attached_path_per_layout() {
        assert_eq!(
            attached_path(Path::new("/A.app/Contents/MacOS/A")),
            PathBuf::from("/A.app/Contents/Resources/app.zebra")
        );
        assert_eq!(attached_path(Path::new("/opt/a/app")), PathBuf::from("/opt/a/app.zebra"));
    }
}
