//! Mapping between files on disk and the URLs the browser loads them by.
//!
//! Files under the app root are served at their root-relative path (`/src/App.tsx`); anything
//! else the resolver reaches (workspace packages, hoisted `node_modules`) at `/@fs/<abs path>`.

use std::path::{Component, Path, PathBuf};

/// Extensions compiled to JavaScript.
pub const COMPILABLE: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// The URL a file is served at.
pub fn url_of(root: &Path, file: &Path) -> String {
    match file.strip_prefix(root) {
        Ok(rel) => format!("/{}", rel.to_string_lossy().replace('\\', "/")),
        Err(_) => format!("/@fs{}", file.display()),
    }
}

/// The file a URL path names, or `None` when it escapes every allowed directory. Installed
/// packages (any path through a `node_modules` directory) are always allowed: Bun's isolated
/// installs and linked workspaces place them outside the app root.
pub fn file_of(root: &Path, allow: &[PathBuf], url_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(url_path);
    let file = match decoded.strip_prefix("/@fs") {
        Some(abs) => normalize(Path::new(abs)),
        None => normalize(&root.join(decoded.trim_start_matches('/'))),
    };
    let installed = file.components().any(|c| c.as_os_str() == "node_modules");
    (installed || allow.iter().any(|dir| file.starts_with(dir))).then_some(file)
}

/// `path` and query of a request URL.
pub fn split_query(url: &str) -> (&str, &str) {
    url.split_once('?').unwrap_or((url, ""))
}

/// Whether the query carries the flag `name` (`?import`, `?import&t=1`).
pub fn has_flag(query: &str, name: &str) -> bool {
    query
        .split('&')
        .any(|kv| kv == name || kv.starts_with(&format!("{name}=")))
}

/// The lowercase extension of a path, or `""`.
pub fn ext(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// A package import (`react`, `@scope/pkg/sub`), not a path or URL.
pub fn is_bare(spec: &str) -> bool {
    !(spec.starts_with('.')
        || spec.starts_with('/')
        || spec.contains("://")
        || spec.starts_with("data:"))
}

/// Lexical normalization: resolves `.` and `..` without touching the disk.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Content type for a served static file.
pub fn mime(ext: &str) -> &'static str {
    match ext {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_round_trip_inside_and_outside_root() {
        let root = Path::new("/w/app");
        let allow = vec![PathBuf::from("/w")];
        assert_eq!(url_of(root, Path::new("/w/app/src/a.tsx")), "/src/a.tsx");
        assert_eq!(url_of(root, Path::new("/w/pkg/x.ts")), "/@fs/w/pkg/x.ts");
        assert_eq!(
            file_of(root, &allow, "/src/a.tsx"),
            Some(PathBuf::from("/w/app/src/a.tsx"))
        );
        assert_eq!(
            file_of(root, &allow, "/@fs/w/pkg/x.ts"),
            Some(PathBuf::from("/w/pkg/x.ts"))
        );
        assert_eq!(
            file_of(root, &allow, "/src/a%20b.ts"),
            Some(PathBuf::from("/w/app/src/a b.ts"))
        );
    }

    #[test]
    fn paths_outside_the_allow_list_are_refused() {
        let root = Path::new("/w/app");
        let allow = vec![PathBuf::from("/w/app")];
        assert_eq!(file_of(root, &allow, "/../../etc/passwd"), None);
        assert_eq!(file_of(root, &allow, "/@fs/etc/passwd"), None);
    }

    #[test]
    fn flags_and_bare_specifiers() {
        assert!(has_flag("import&t=3", "import"));
        assert!(has_flag("t=3", "t"));
        assert!(!has_flag("imports", "import"));
        assert!(is_bare("react"));
        assert!(is_bare("@scope/pkg/sub"));
        assert!(!is_bare("./a"));
        assert!(!is_bare("/src/a.ts"));
    }
}
