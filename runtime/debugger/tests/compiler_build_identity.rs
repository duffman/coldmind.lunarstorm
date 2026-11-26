//! Artifact/cache identity must change with the actual production engine inputs.
#![cfg(feature = "quickjs-ng")]
use coldmind_runtime::{engine::JsEngine, engines::quickjs_ng::QuickJsNgEngine};

#[test]
fn production_compiler_identity_includes_actual_inputs_and_bytecode_version() {
    let engine = QuickJsNgEngine::new().unwrap();
    let fingerprint = &engine.descriptor().build_fingerprint;
    // lunarstorm patch (expectation approved by the user, 2026-11-24): jetbunny's
    // BC_VERSION byte is 93; Lunarstorm's is 94 since the user-catch table.
    assert!(fingerprint.contains(";bytecode=94;"), "{fingerprint}");
    let (_, digest) = fingerprint
        .split_once(";inputs_sha256=")
        .expect("actual build inputs must be hashed");
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()));
}
