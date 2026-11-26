//! Embeds every template folder of this crate (a directory holding `template.json`)
//! as `FILES` in `$OUT_DIR/files.rs`: one `("<template>/<path>", bytes)` per file.
//!
//! Decisions:
//! - The folders are the source of truth: adding a file to a template needs no Rust
//!   edit, and Cargo reruns this script when anything in the crate changes.
//! - Names starting with `.` are skipped (`.DS_Store`, editor files); a template
//!   ships its ignore file as `gitignore`, which `init` writes as `.gitignore`.
//! - Files are listed in sorted order, so the generated table is deterministic.

use std::fs;
use std::path::{Path, PathBuf};

/// Collects the files under `dir`, sorted, as paths relative to `root`.
///
/// # Arguments
/// * `root` - the crate directory.
/// * `dir` - the folder to walk.
/// * `out` - receives each file's path relative to `root`.
fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir).expect("template folder").flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, out);
        } else {
            out.push(path.strip_prefix(root).expect("under the crate").to_path_buf());
        }
    }
}

/// Writes `$OUT_DIR/files.rs`.
fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-changed={}", root.display());
    let mut templates: Vec<PathBuf> = fs::read_dir(&root).expect("crate directory").flatten().map(|e| e.path()).filter(|p| p.join("template.json").is_file()).collect();
    templates.sort();
    let mut files = Vec::new();
    for template in &templates {
        walk(&root, template, &mut files);
    }
    let mut code = String::from("/// Every template file: `(\"<template>/<path>\", bytes)`, sorted by path.\npub const FILES: &[(&str, &[u8])] = &[\n");
    for file in &files {
        let name = file.to_string_lossy().replace('\\', "/");
        code += &format!("    ({name:?}, include_bytes!({:?})),\n", root.join(file));
    }
    code += "];\n";
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("files.rs");
    fs::write(out, code).expect("write files.rs");
}
