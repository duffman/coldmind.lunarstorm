//! `.jbn` packages: a signed container of an application's compiled modules, its
//! resources and the import table that links them, ported from jetbunny's
//! `src/jbn` without its engine coupling. The container names an engine (see
//! [`Manifest::engine`]) but never touches one; the Runtime compiles modules into
//! it and loads them out of it.
//!
//! Layout (all integers little-endian):
//!
//! | offset   | size | field                                         |
//! |----------|------|-----------------------------------------------|
//! | 0        | 4    | magic `JBNY`                                  |
//! | 4        | 4    | container version `[1, 0, 0, 0]`              |
//! | 8        | 8    | manifest length                               |
//! | 16       | 8    | signature length                              |
//! | 24       | 8    | payload length                                |
//! | 32       | 32   | SHA-256 of the manifest                       |
//! | 64       | m    | manifest (canonical CBOR, see [`manifest`])   |
//! | 64+m     | s    | signature (see [`trust`])                     |
//! | aligned  | p    | payload, starting at the next multiple of 8   |
//!
//! Decision: [`Package::open`] verifies everything before returning: the header,
//! the manifest digest, the signature against a [`TrustStore`], the manifest's own
//! consistency, and every blob's digest. A `Package` is therefore fully trusted,
//! and its bytecode may be handed to an engine without further checks.

pub mod manifest;
pub mod trust;
mod wire;

pub use manifest::{Blob, Compression, Manifest, Module, Resource};
pub use trust::{Signer, TrustStore, Trusted, to_hex};

use sha2::{Digest, Sha256};
use std::fmt;

/// First bytes of every package.
pub const MAGIC: &[u8; 4] = b"JBNY";
/// Container version this crate writes and reads.
pub const VERSION: [u8; 4] = [1, 0, 0, 0];
/// Size of the fixed header.
const HEADER: usize = 64;

/// Why a package was refused or could not be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Reading or writing failed.
    Io(String),
    /// Malformed bytes.
    Format(String),
    /// Past a [`Limits`] bound.
    Limit,
    /// Signature, signer or release refused.
    Trust(String),
    /// A blob is compressed and the `zstd` feature is off.
    Unsupported(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(m) => write!(f, "jbn: {m}"),
            Error::Format(m) => write!(f, "jbn: malformed package: {m}"),
            Error::Limit => f.write_str("jbn: package exceeds its limits"),
            Error::Trust(m) => write!(f, "jbn: untrusted package: {m}"),
            Error::Unsupported(m) => write!(f, "jbn: unsupported: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// Bounds a reader enforces before allocating anything a package asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Whole package size.
    pub package_bytes: usize,
    /// Manifest size.
    pub manifest_bytes: usize,
    /// One blob, stored or decoded.
    pub blob_bytes: usize,
    /// Number of modules.
    pub modules: usize,
    /// Number of resources.
    pub resources: usize,
    /// Number of import table entries over all modules.
    pub edges: usize,
}

impl Default for Limits {
    /// Decision: jetbunny's defaults (256 MiB package, 16 MiB manifest, 64 MiB
    /// blob, 10 000 modules and resources, 100 000 edges).
    fn default() -> Self {
        Limits {
            package_bytes: 256 << 20,
            manifest_bytes: 16 << 20,
            blob_bytes: 64 << 20,
            modules: 10_000,
            resources: 10_000,
            edges: 100_000,
        }
    }
}

/// # Returns
/// SHA-256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// # Returns
/// `n` rounded up to a multiple of 8.
fn align(n: usize) -> usize {
    n.div_ceil(8) * 8
}

/// Collects a package's contents, then writes and signs it.
pub struct Builder {
    /// The manifest under construction; blobs and indexes are set by [`Builder::build`].
    manifest: Manifest,
    /// Module bytecode, by module position after sorting.
    modules: Vec<(Module, Vec<u8>)>,
    /// Resource bytes.
    resources: Vec<(String, Vec<u8>)>,
    /// How blobs are stored.
    compression: Compression,
}

impl Builder {
    /// # Arguments
    /// * `name`, `version` - the package's identity.
    /// * `release` - its release sequence number.
    /// * `engine` - the engine identity the bytecode is for.
    /// * `entry` - name of the entry module.
    pub fn new(name: &str, version: &str, release: u64, engine: &str, entry: &str) -> Builder {
        Builder {
            manifest: Manifest {
                name: name.to_owned(),
                version: version.to_owned(),
                release,
                engine: engine.to_owned(),
                entry: entry.to_owned(),
                modules: Vec::new(),
                resources: Vec::new(),
                metadata: Vec::new(),
                blobs: Vec::new(),
            },
            modules: Vec::new(),
            resources: Vec::new(),
            compression: Compression::None,
        }
    }

    /// Adds a compiled module with its import table (`specifier -> target name`).
    pub fn module(&mut self, name: &str, bytecode: Vec<u8>, mut imports: Vec<(String, String)>) -> &mut Self {
        imports.sort();
        imports.dedup();
        self.modules.push((Module { name: name.to_owned(), blob: 0, imports }, bytecode));
        self
    }

    /// Adds a resource.
    pub fn resource(&mut self, name: &str, bytes: Vec<u8>) -> &mut Self {
        self.resources.push((name.to_owned(), bytes));
        self
    }

    /// Adds a metadata entry.
    pub fn metadata(&mut self, key: &str, value: &str) -> &mut Self {
        self.manifest.metadata.push((key.to_owned(), value.to_owned()));
        self
    }

    /// Stores blobs zstd-compressed; refused at build time without the feature.
    pub fn compression(&mut self, compression: Compression) -> &mut Self {
        self.compression = compression;
        self
    }

    /// Writes the package and signs its manifest.
    ///
    /// # Returns
    /// The package bytes.
    ///
    /// # Errors
    /// The manifest is inconsistent (duplicate names, missing entry), or
    /// compression is unavailable.
    pub fn build(mut self, signer: &Signer) -> Result<Vec<u8>, Error> {
        self.modules.sort_by(|a, b| a.0.name.cmp(&b.0.name));
        self.resources.sort_by(|a, b| a.0.cmp(&b.0));
        self.manifest.metadata.sort();
        let mut payload = Vec::new();
        let mut blobs = Vec::new();
        let compression = self.compression;
        let mut store = |bytes: &[u8]| -> Result<u64, Error> {
            let stored = encode(bytes, compression)?;
            payload.resize(align(payload.len()), 0);
            blobs.push(Blob {
                offset: payload.len() as u64,
                stored: stored.len() as u64,
                size: bytes.len() as u64,
                digest: sha256(bytes),
                compression,
            });
            payload.extend_from_slice(&stored);
            Ok(blobs.len() as u64 - 1)
        };
        for (module, bytecode) in &mut self.modules {
            module.blob = store(bytecode)?;
        }
        let mut resources = Vec::new();
        for (name, bytes) in &self.resources {
            resources.push(Resource { name: name.clone(), blob: store(bytes)? });
        }
        self.manifest.modules = self.modules.into_iter().map(|(m, _)| m).collect();
        self.manifest.resources = resources;
        self.manifest.blobs = blobs;
        self.manifest.validate(&Limits::default())?;

        let manifest = self.manifest.encode();
        let digest = sha256(&manifest);
        let signature = signer.sign(&digest);
        let mut out = Vec::with_capacity(HEADER + manifest.len() + signature.len() + payload.len() + 8);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION);
        out.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
        out.extend_from_slice(&(signature.len() as u64).to_le_bytes());
        out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        out.extend_from_slice(&digest);
        out.extend_from_slice(&manifest);
        out.extend_from_slice(&signature);
        out.resize(align(out.len()), 0);
        out.extend_from_slice(&payload);
        Ok(out)
    }
}

/// # Returns
/// `bytes` as stored under `compression`.
///
/// # Errors
/// [`Error::Unsupported`] for zstd without the feature.
fn encode(bytes: &[u8], compression: Compression) -> Result<Vec<u8>, Error> {
    match compression {
        Compression::None => Ok(bytes.to_vec()),
        #[cfg(feature = "zstd")]
        Compression::Zstd => zstd::bulk::compress(bytes, 19).map_err(|e| Error::Io(e.to_string())),
        #[cfg(not(feature = "zstd"))]
        Compression::Zstd => Err(Error::Unsupported(String::from("zstd compression (feature `zstd`)"))),
    }
}

/// # Returns
/// The decoded bytes of a stored blob.
///
/// # Errors
/// Decoding failed, or the decoded size is not the manifest's.
fn decode(stored: &[u8], blob: &Blob) -> Result<Vec<u8>, Error> {
    let bytes = match blob.compression {
        Compression::None => stored.to_vec(),
        #[cfg(feature = "zstd")]
        Compression::Zstd => {
            zstd::bulk::decompress(stored, blob.size as usize).map_err(|e| Error::Format(format!("zstd: {e}")))?
        }
        #[cfg(not(feature = "zstd"))]
        Compression::Zstd => return Err(Error::Unsupported(String::from("zstd compression (feature `zstd`)"))),
    };
    if bytes.len() as u64 != blob.size {
        return Err(Error::Format(String::from("a blob's decoded size differs from its manifest entry")));
    }
    Ok(bytes)
}

/// A verified package.
#[derive(Debug, Clone)]
pub struct Package {
    /// What it holds.
    manifest: Manifest,
    /// SHA-256 of the manifest: the package identity.
    digest: [u8; 32],
    /// The signer's public key.
    signer: [u8; 32],
    /// Decoded blobs, by manifest index.
    blobs: Vec<Vec<u8>>,
}

impl Package {
    /// Reads and verifies a package (see the crate documentation).
    ///
    /// # Errors
    /// Any check failing; nothing of the package is returned then.
    pub fn open(bytes: &[u8], trust: &TrustStore, limits: &Limits) -> Result<Package, Error> {
        let bad = |message: &str| Err(Error::Format(message.to_owned()));
        if bytes.len() > limits.package_bytes {
            return Err(Error::Limit);
        }
        if bytes.len() < HEADER || &bytes[0..4] != MAGIC {
            return bad("not a .jbn package");
        }
        if bytes[4..8] != VERSION {
            return Err(Error::Unsupported(format!("container version {:?}", &bytes[4..8])));
        }
        let length = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 bytes"));
        let (manifest_len, signature_len, payload_len) = (length(8), length(16), length(24));
        if manifest_len > limits.manifest_bytes as u64 || signature_len > 4096 {
            return Err(Error::Limit);
        }
        let manifest_end = HEADER + manifest_len as usize;
        let signature_end = manifest_end + signature_len as usize;
        let payload_start = align(signature_end);
        if (payload_start as u64).checked_add(payload_len) != Some(bytes.len() as u64) {
            return bad("section lengths do not add up to the package size");
        }
        if bytes[signature_end..payload_start].iter().any(|&b| b != 0) {
            return bad("nonzero padding");
        }
        let manifest_bytes = &bytes[HEADER..manifest_end];
        let digest = sha256(manifest_bytes);
        if digest[..] != bytes[32..64] {
            return bad("manifest digest mismatch");
        }
        // Trust first: nothing of an unsigned manifest is interpreted beyond what
        // the signature check needs (its name and release).
        let manifest = Manifest::decode(manifest_bytes)?;
        let (signer, signature) = trust::signature(&bytes[manifest_end..signature_end])?;
        trust.check(&signer, &signature, &digest, &manifest.name, manifest.release)?;
        manifest.validate(limits)?;

        let payload = &bytes[payload_start..];
        let mut end = 0u64;
        let mut blobs = Vec::with_capacity(manifest.blobs.len());
        for blob in &manifest.blobs {
            if blob.offset % 8 != 0 || blob.offset < end {
                return bad("blobs must be 8-aligned and in order");
            }
            let stop = blob.offset.checked_add(blob.stored).filter(|&s| s <= payload.len() as u64);
            let Some(stop) = stop else { return bad("a blob lies outside the payload") };
            let bytes = decode(&payload[blob.offset as usize..stop as usize], blob)?;
            if sha256(&bytes) != blob.digest {
                return bad("a blob's digest does not match");
            }
            blobs.push(bytes);
            end = stop;
        }
        if end != payload.len() as u64 {
            return bad("bytes after the last blob");
        }
        Ok(Package { manifest, digest, signer, blobs })
    }

    /// # Returns
    /// What the package holds.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// # Returns
    /// SHA-256 of the manifest.
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }

    /// # Returns
    /// The public key that signed it.
    pub fn signer(&self) -> [u8; 32] {
        self.signer
    }

    /// # Returns
    /// A module's bytecode.
    pub fn bytecode(&self, module: &str) -> Option<&[u8]> {
        self.manifest.module(module).map(|m| self.blobs[m.blob as usize].as_slice())
    }

    /// # Returns
    /// The name an import of `specifier` from `module` resolved to at build time.
    pub fn import(&self, module: &str, specifier: &str) -> Option<&str> {
        let imports = &self.manifest.module(module)?.imports;
        let i = imports.binary_search_by(|(s, _)| s.as_str().cmp(specifier)).ok()?;
        Some(imports[i].1.as_str())
    }

    /// # Returns
    /// A resource's bytes.
    pub fn resource(&self, name: &str) -> Option<&[u8]> {
        self.manifest.resource(name).map(|r| self.blobs[r.blob as usize].as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trust store that trusts `signer` for every package.
    fn trusting(signer: &Signer) -> TrustStore {
        TrustStore { signers: vec![Trusted { public_key: signer.public_key(), packages: Vec::new(), minimum_release: 0 }] }
    }

    /// A two-module package with one resource.
    fn sample(compression: Compression, signer: &Signer) -> Vec<u8> {
        let mut builder = Builder::new("app", "1.0.0", 3, "quickjs", "/app/main.js");
        builder
            .module("/app/main.js", b"main bytecode".to_vec(), vec![("./dep.js".into(), "/app/dep.js".into())])
            .module("/app/dep.js", vec![7; 1000], Vec::new())
            .resource("config.json", b"{}".to_vec())
            .metadata("built", "today")
            .compression(compression);
        builder.build(signer).expect("builds")
    }

    #[test]
    fn a_package_round_trips_with_and_without_compression() {
        let signer = Signer::generate().expect("key");
        for compression in [Compression::None, Compression::Zstd] {
            let bytes = sample(compression, &signer);
            let package = Package::open(&bytes, &trusting(&signer), &Limits::default()).expect("opens");
            assert_eq!(package.manifest().entry, "/app/main.js");
            assert_eq!(package.bytecode("/app/main.js"), Some(&b"main bytecode"[..]));
            assert_eq!(package.bytecode("/app/dep.js").map(<[u8]>::len), Some(1000));
            assert_eq!(package.import("/app/main.js", "./dep.js"), Some("/app/dep.js"));
            assert_eq!(package.resource("config.json"), Some(&b"{}"[..]));
            assert_eq!(package.signer(), signer.public_key());
        }
        // Builds are deterministic for the same inputs and key.
        assert_eq!(sample(Compression::None, &signer), sample(Compression::None, &signer));
    }

    #[test]
    fn tampering_untrusted_signers_and_old_releases_are_refused() {
        let signer = Signer::generate().expect("key");
        let trust = trusting(&signer);
        let bytes = sample(Compression::None, &signer);

        // Any flipped payload byte breaks a blob digest; a manifest byte, its digest.
        let mut payload = bytes.clone();
        *payload.last_mut().expect("payload") ^= 1;
        assert!(matches!(Package::open(&payload, &trust, &Limits::default()), Err(Error::Format(_))));
        let mut manifest = bytes.clone();
        manifest[HEADER + 3] ^= 1;
        assert!(matches!(Package::open(&manifest, &trust, &Limits::default()), Err(Error::Format(_))));

        let stranger = Signer::generate().expect("key");
        assert!(matches!(Package::open(&bytes, &trusting(&stranger), &Limits::default()), Err(Error::Trust(_))));

        let scoped = |packages: Vec<String>, minimum_release| TrustStore {
            signers: vec![Trusted { public_key: signer.public_key(), packages, minimum_release }],
        };
        assert!(Package::open(&bytes, &scoped(vec!["app".into()], 3), &Limits::default()).is_ok());
        assert!(matches!(Package::open(&bytes, &scoped(vec!["other".into()], 0), &Limits::default()), Err(Error::Trust(_))));
        assert!(matches!(Package::open(&bytes, &scoped(Vec::new(), 4), &Limits::default()), Err(Error::Trust(_))));

        let tight = Limits { modules: 1, ..Limits::default() };
        assert_eq!(Package::open(&bytes, &trust, &tight).unwrap_err(), Error::Limit);
    }

    #[test]
    fn keys_and_trust_files_round_trip() {
        let signer = Signer::generate().expect("key");
        let der = signer.to_pkcs8_der().expect("der");
        assert_eq!(Signer::from_pkcs8_der(&der).expect("reads").public_key(), signer.public_key());
        let store = TrustStore {
            signers: vec![Trusted { public_key: signer.public_key(), packages: vec!["app".into()], minimum_release: 2 }],
        };
        assert_eq!(TrustStore::from_json(store.to_json().as_bytes()).expect("parses"), store);
        assert!(TrustStore::from_json(br#"{"signers":[{"public_key":"zz"}]}"#).is_err());
    }

    #[test]
    fn an_inconsistent_manifest_is_not_built() {
        let signer = Signer::generate().expect("key");
        let mut missing = Builder::new("app", "1", 1, "quickjs", "/app/none.js");
        missing.module("/app/main.js", Vec::new(), Vec::new());
        assert!(missing.build(&signer).is_err());
        let mut twice = Builder::new("app", "1", 1, "quickjs", "/app/main.js");
        twice.module("/app/main.js", Vec::new(), Vec::new()).module("/app/main.js", Vec::new(), Vec::new());
        assert!(twice.build(&signer).is_err());
    }
}
