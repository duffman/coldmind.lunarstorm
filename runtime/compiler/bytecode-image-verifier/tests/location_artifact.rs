//! Protected location artifacts: native locations, truncation, span bounds and the
//! lossless UTF-16 source digest.

use qjs_bytecode_image_verifier::{image::*, production::ids, snapshot::Position};
use sha2::{Digest, Sha256};
fn function() -> FunctionImage {
    FunctionImage {
        code: vec![ids::push_i8, 42, ids::r#return],
        stack_size: 1,
        filename: "source.js".into(),
        debug_locations: vec![
            DebugLocation {
                code_offset: 0,
                line: 5,
                column: 17,
            },
            DebugLocation {
                code_offset: 2,
                line: 10,
                column: 1,
            },
        ],
        ..Default::default()
    }
}
fn spans(unit: SourceUnit) -> SpanMetadata {
    SpanMetadata {
        source_unit: unit,
        source_len: 20,
        source_digest: source_digest_utf16(&"source identity".encode_utf16().collect::<Vec<_>>()),
        functions: vec![FunctionSpans {
            path: vec![],
            function_start: 0,
            function_end: 20,
            positions: vec![
                Position {
                    code_offset: 0,
                    source_start: 1,
                    source_end: 19,
                },
                Position {
                    code_offset: 2,
                    source_start: 5,
                    source_end: 6,
                },
            ],
        }],
    }
}
#[test]
fn native_locations_do_not_invent_source_spans_or_compiler_identity() {
    let f = function();
    let bytes = encode(&f).unwrap();
    assert_eq!(bytes[0], 94); // lunarstorm patch: native format version 94 (user-catch table)
    let image = decode_and_verify(&bytes).unwrap();
    assert_eq!(image.function().debug_locations, f.debug_locations);
    assert!(image.spans().is_none());
    assert_eq!(image.profile(), NATIVE_PROFILE);
    let artifact = image.serialize_artifact().unwrap();
    let reloaded = deserialize_artifact(&artifact).unwrap();
    assert_eq!(reloaded.profile(), NATIVE_PROFILE);
    assert!(reloaded.spans().is_none());
    assert_eq!(reloaded.bytes(), bytes);
}
#[test]
fn protected_artifact_preserves_legacy_byte_spans_and_utf16_spans_separately() {
    for unit in [SourceUnit::Utf8, SourceUnit::Utf16] {
        let meta = spans(unit);
        let image = verify_and_encode(&function(), Some(meta.clone())).unwrap();
        assert_eq!(image.profile(), COMPILER_PROFILE);
        assert_eq!(image.spans(), Some(&meta));
        let artifact = image.serialize_artifact().unwrap();
        assert_ne!(&artifact[..8], &image.bytes()[..8]);
        let copy = deserialize_artifact(&artifact).unwrap();
        assert_eq!(copy.profile(), COMPILER_PROFILE);
        assert_eq!(copy.spans(), Some(&meta));
        assert_eq!(copy.function().debug_locations, function().debug_locations);
        assert_eq!(copy.bytes(), image.bytes());
        assert!(decode_and_verify(&artifact).is_err());
    }
}
#[test]
fn artifacts_reject_truncation_tampering_and_native_or_metadata_swaps() {
    let image = verify_and_encode(&function(), Some(spans(SourceUnit::Utf8))).unwrap();
    let artifact = image.serialize_artifact().unwrap();
    for end in 0..artifact.len() {
        assert!(deserialize_artifact(&artifact[..end]).is_err());
    }
    for at in 0..artifact.len() {
        let mut bad = artifact.clone();
        bad[at] ^= 0x40;
        assert!(deserialize_artifact(&bad).is_err(), "byte{at}");
    }
    let mut bad = artifact.clone();
    let pc_offset = bad.len() - 32 - 24;
    bad[pc_offset..pc_offset + 4].copy_from_slice(&1u32.to_le_bytes());
    let split = bad.len() - 32;
    let digest = Sha256::digest(&bad[..split]);
    bad[split..].copy_from_slice(&digest);
    assert!(
        deserialize_artifact(&bad).is_err(),
        "recomputed envelope hash cannot prove a mid-operand source span"
    );
}
#[test]
fn spans_are_bounded_to_real_function_paths_and_ranges() {
    let f = function();
    for edit in 0..5 {
        let mut meta = spans(SourceUnit::Utf16);
        match edit {
            0 => meta.functions[0].path = vec![0],
            1 => meta.functions[0].positions[0].code_offset = 1,
            2 => meta.functions[0].function_end = 21,
            3 => meta.functions[0].positions[0].source_end = 21,
            4 => meta.functions.push(meta.functions[0].clone()),
            _ => unreachable!(),
        }
        assert!(verify_and_encode(&f, Some(meta)).is_err());
    }
    let mut impossible = function();
    impossible.debug_locations[1].line = 100;
    assert!(verify_and_encode(&impossible, Some(spans(SourceUnit::Utf16))).is_err());
    let mut root = function();
    root.constants = vec![Constant::Function(Box::new(function()))];
    let mut meta = spans(SourceUnit::Utf16);
    let mut child = meta.functions[0].clone();
    child.path = vec![0];
    meta.functions.push(child);
    let image = verify_and_encode(&root, Some(meta.clone())).unwrap();
    assert_eq!(
        deserialize_artifact(&image.serialize_artifact().unwrap())
            .unwrap()
            .spans(),
        Some(&meta)
    );
}
#[test]
fn native_debug_location_fields_have_independent_one_based_bounds() {
    let mut f = function();
    f.debug_locations[0].line = 0;
    assert!(encode(&f).is_err());
    let mut f = function();
    f.debug_locations[0].column = 0;
    assert!(encode(&f).is_err());
    let mut f = function();
    f.debug_locations[0].column = u32::MAX;
    assert!(encode(&f).is_err());
    let mut f = function();
    f.debug_locations[0].code_offset = 1;
    assert!(encode(&f).is_err());
    let mut f = function();
    f.debug_locations[1].code_offset = 0;
    assert!(encode(&f).is_err());
}
#[test]
fn utf16_source_digest_is_lossless_little_endian_and_unit_explicit() {
    let units = [0x41, 0xd83d, 0xde00, 0xd800, 0, 0xdc00];
    let bytes: Vec<_> = units.iter().flat_map(|u| u16::to_le_bytes(*u)).collect();
    let expected: [u8; 32] = Sha256::digest(bytes).into();
    assert_eq!(source_digest_utf16(&units), expected);
}
#[test]
fn explicit_artifact_profile_and_source_units_fail_closed_even_with_new_hash() {
    let artifact = verify_and_encode(&function(), Some(spans(SourceUnit::Utf16)))
        .unwrap()
        .serialize_artifact()
        .unwrap();
    let mut bad = artifact.clone();
    let mut cursor = 12usize;
    for _ in 0..3 {
        let length = u32::from_le_bytes(bad[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4 + length;
    }
    bad[cursor + 1] = 2;
    let split = bad.len() - 32;
    let digest = Sha256::digest(&bad[..split]);
    bad[split..].copy_from_slice(&digest);
    assert!(
        deserialize_artifact(&bad).is_err(),
        "unsupported source unit with recomputed envelope hash"
    );
    for (needle, replacement) in [
        (
            COMPILER_PROFILE.as_bytes(),
            b"rust-native-locations-v2".as_slice(),
        ),
        (
            qjs_bytecode_image_verifier::production::TARGET_FINGERPRINT.as_bytes(),
            b"other-target".as_slice(),
        ),
    ] {
        let mut bad = artifact.clone();
        let at = bad.windows(needle.len()).position(|v| v == needle).unwrap();
        bad[at..at + replacement.len()].copy_from_slice(replacement);
        let split = bad.len() - 32;
        let digest = Sha256::digest(&bad[..split]);
        bad[split..].copy_from_slice(&digest);
        assert!(deserialize_artifact(&bad).is_err());
    }
}
#[test]
fn eval_profile_and_spans_are_retained_without_becoming_ordinary_root_proof() {
    use qjs_bytecode_image_verifier::image::eval::*;
    let layout = verify_env_layout(EnvLayout::default()).unwrap();
    let f = function();
    let meta = spans(SourceUnit::Utf16);
    let image = verify_and_encode_eval(&f, &layout, Some(meta.clone())).unwrap();
    assert_eq!(image.profile(), COMPILER_PROFILE);
    assert_eq!(image.spans(), Some(&meta));
    assert_eq!(
        decode_eval_and_verify(image.bytes(), &layout)
            .unwrap()
            .profile(),
        NATIVE_PROFILE
    );
}
