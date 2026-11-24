// Copyright (c) 2026 Coldmind AB
// SPDX-License-Identifier: MIT
//! Every binding file numbers the predefined atoms as `quickjs/quickjs-atom.h` does.
//!
//! jetbunny's bindings kept `JS_ATOM_concat` after the 0.17.0 header dropped it,
//! which shifted every later atom by one. `scripts/bindings_from_jetbunny.py`
//! removes it; this test fails if any of the 20 files disagrees with the header.
use std::path::Path;

/// The `JS_ATOM_*` names in header order, each paired with its number.
///
/// # Returns
/// `(name, number)` for every `DEF` of `quickjs-atom.h` (numbered from 1, after
/// `__JS_ATOM_NULL`), followed by `JS_ATOM_END`.
fn header_atoms() -> Vec<(String, u32)> {
    let header = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("quickjs/quickjs-atom.h")).unwrap();
    let mut atoms: Vec<(String, u32)> = header
        .lines()
        .filter_map(|line| line.strip_prefix("DEF(")?.split_once(',').map(|(name, _)| name.trim().to_owned()))
        .enumerate()
        .map(|(index, name)| (format!("JS_ATOM_{name}"), index as u32 + 1))
        .collect();
    atoms.push(("JS_ATOM_END".into(), atoms.len() as u32 + 1));
    atoms
}

/// The number a binding file gives the constant `name`.
///
/// # Arguments
/// * `file` - the binding file's text.
/// * `name` - a `JS_ATOM_*` constant.
///
/// # Returns
/// Its value, or `None` when the file does not define it.
fn bound(file: &str, name: &str) -> Option<u32> {
    let prefix = format!("pub const {name}: ");
    file.lines().find_map(|line| line.strip_prefix(&prefix)?.split_once(" = ")?.1.strip_suffix(';')?.parse().ok())
}

#[test]
fn every_binding_file_numbers_atoms_as_the_header_does() {
    let atoms = header_atoms();
    assert_eq!(atoms.len(), 241, "quickjs-atom.h DEF count changed");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bindings");
    let mut files = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        files += 1;
        assert!(bound(&text, "JS_ATOM_concat").is_none(), "{}: JS_ATOM_concat is not in the header", path.display());
        for (name, number) in &atoms {
            assert_eq!(bound(&text, name), Some(*number), "{}: {name}", path.display());
        }
    }
    assert_eq!(files, 20);
}
