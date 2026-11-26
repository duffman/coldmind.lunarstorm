//! The committed `dist/` is what `lunarstorm types` writes today (plan AC5): the
//! files are regenerated in memory and compared byte for byte, and `dist/` may hold
//! no other `.d.ts` file.

use std::path::{Path, PathBuf};

/// Lists the `.d.ts` files under a directory.
///
/// # Arguments
/// * `root` - the directory searched.
/// * `dir` - the directory being read, `root` or below it.
///
/// # Returns
/// Their paths relative to `root`.
fn listed(root: &Path, dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(listed(root, &path));
        } else if path.to_string_lossy().ends_with(".d.ts") {
            found.push(path.strip_prefix(root).expect("below root").to_path_buf());
        }
    }
    found
}

#[test]
fn dist_matches_the_declarations() {
    const FIX: &str = "run `lunarstorm types` at the repository root and commit runtime/typescript-declarations/dist";
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("dist");
    let files = lunarstorm_host_command_line::type_files().expect("collect the declarations");
    for (path, text) in &files {
        let committed = std::fs::read_to_string(dist.join(path)).unwrap_or_default();
        assert!(committed == *text, "runtime/typescript-declarations/dist/{} is stale: {FIX}", path.display());
    }
    for path in listed(&dist, &dist) {
        assert!(files.iter().any(|(made, _)| *made == path), "runtime/typescript-declarations/dist/{} is no longer generated: remove it, then {FIX}", path.display());
    }
}
