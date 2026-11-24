// Copyright (c) 2026 Coldmind AB
// SPDX-License-Identifier: MIT
//! The provenance records in `quickjs/` agree with the tree.
//!
//! `REVISION` must hash every file in `quickjs/` except itself, and
//! `JETBUNNY-SHA256` must hash files that are still byte-identical to the
//! jetbunny copies they were taken from. Any edit to an engine source without
//! regenerating them (`scripts/provenance.sh`) fails here.
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The engine source directory.
///
/// # Returns
/// `<crate>/quickjs`.
fn engine_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("quickjs")
}

/// The `<sha256>  <file>` lines of a record.
///
/// # Arguments
/// * `record` - file name inside `quickjs/`.
///
/// # Returns
/// File name to recorded hex digest.
fn recorded(record: &str) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(engine_dir().join(record))
        .unwrap_or_else(|error| panic!("{record}: {error}"));
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.split_once("  ")?;
            (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| (name.to_owned(), hash.to_owned()))
        })
        .collect()
}

/// The SHA-256 of a file in `quickjs/`.
///
/// # Arguments
/// * `name` - file name inside `quickjs/`.
///
/// # Returns
/// Lower-case hex digest.
fn digest(name: &str) -> String {
    let bytes = std::fs::read(engine_dir().join(name)).unwrap_or_else(|error| panic!("{name}: {error}"));
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Every recorded hash that differs from the file on disk.
///
/// # Arguments
/// * `record` - the record's entries.
///
/// # Returns
/// The names whose hash is stale.
fn stale(record: &BTreeMap<String, String>) -> Vec<&str> {
    record
        .iter()
        .filter(|(name, hash)| digest(name) != **hash)
        .map(|(name, _)| name.as_str())
        .collect()
}

#[test]
fn revision_hashes_every_engine_file() {
    let record = recorded("REVISION");
    let mut on_disk: Vec<String> = std::fs::read_dir(engine_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name != "REVISION")
        .collect();
    on_disk.sort();
    let listed: Vec<String> = record.keys().cloned().collect();
    assert_eq!(listed, on_disk, "REVISION does not list exactly the files in quickjs/; run scripts/provenance.sh revision");
    assert_eq!(stale(&record), Vec::<&str>::new(), "REVISION hashes are stale; run scripts/provenance.sh revision");
}

/// `PATCHES.md`'s "Sites" list has one entry per `lunarstorm patch` comment
/// in the sources, so no Lunarstorm change to jetbunny's engine goes unlisted.
/// Only the count is checked: line numbers move with every edit.
#[test]
fn every_lunarstorm_site_is_listed() {
    let in_sources: usize = std::fs::read_dir(engine_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| matches!(path.extension().and_then(|e| e.to_str()), Some("c" | "h" | "inc")))
        .map(|path| std::fs::read_to_string(&path).unwrap().matches("lunarstorm patch").count())
        .sum();
    let patches = std::fs::read_to_string(engine_dir().join("PATCHES.md")).unwrap();
    let sites = patches
        .split_once("\n## Sites\n")
        .expect("PATCHES.md has a `## Sites` section")
        .1;
    let listed = sites.lines().filter(|line| line.starts_with("- `")).count();
    assert_eq!(listed, in_sources, "PATCHES.md `## Sites` is out of date");
}

#[test]
fn jetbunny_files_are_unchanged() {
    let record = recorded("JETBUNNY-SHA256");
    assert!(!record.is_empty(), "JETBUNNY-SHA256 lists no files");
    assert_eq!(
        stale(&record),
        Vec::<&str>::new(),
        "files recorded as jetbunny's verbatim copy were edited; mark the change in PATCHES.md and rerun scripts/provenance.sh jetbunny <checkout>"
    );
}
