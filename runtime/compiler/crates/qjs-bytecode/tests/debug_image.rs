use qjs_bytecode::{image::*, production::ids};
fn marker(raw: u32) -> Vec<u8> {
    let mut b = vec![ids::debug_stmt];
    b.extend(raw.to_le_bytes());
    b
}
fn raw(code: Vec<u8>, stack_size: u32) -> FunctionImage {
    FunctionImage {
        code,
        stack_size,
        ..Default::default()
    }
}
fn lexical(code: Vec<u8>) -> FunctionImage {
    FunctionImage {
        code,
        local_count: 1,
        var_ref_count: 1,
        locals: vec![VarDef {
            name: vec![b'x' as u16],
            scope_level: 1,
            is_lexical: true,
            is_captured: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}
#[test]
fn exact_end_marker_is_stack_neutral() {
    let mut code = vec![ids::push_1];
    code.extend(marker(0));
    code.push(ids::r#return);
    decode_and_verify(&encode(&raw(code, 1)).unwrap()).unwrap();
    let mut code = marker(0);
    code.push(ids::return_undef);
    decode_and_verify(&encode(&raw(code, 0)).unwrap()).unwrap();
}
#[test]
fn markers_bind_exact_active_lexical_chain() {
    let mut code = vec![ids::set_loc_uninitialized, 0, 0];
    code.extend(marker(1));
    code.push(ids::return_undef);
    decode_and_verify(&encode(&lexical(code.clone())).unwrap()).unwrap();
    let mut inactive = marker(1);
    inactive.push(ids::return_undef);
    assert!(encode(&lexical(inactive)).is_err());
    let mut omitted = code.clone();
    omitted[4..8].copy_from_slice(&0u32.to_le_bytes());
    assert!(encode(&lexical(omitted)).is_err());
    let mut invalid = code;
    invalid[4..8].copy_from_slice(&2u32.to_le_bytes());
    assert!(encode(&lexical(invalid)).is_err());
}
#[test]
fn marker_ids_and_signed_head_boundaries_are_not_unchecked() {
    for bits in [0x00010000, 0x7fff0000, 0xffff] {
        let mut code = marker(bits);
        code.push(ids::return_undef);
        assert!(encode(&raw(code, 0)).is_err());
    }
    let mut duplicate = marker(0);
    duplicate.extend(marker(0));
    duplicate.push(ids::return_undef);
    assert!(encode(&raw(duplicate, 0)).is_err());
    let mut debugger = marker(0x80000000);
    debugger.push(ids::return_undef);
    decode_and_verify(&encode(&raw(debugger, 0)).unwrap()).unwrap();
    let mut f = lexical(vec![ids::set_loc_uninitialized, 0, 0]);
    f.code.extend(marker(1));
    f.code.push(ids::return_undef);
    f.locals[0].scope_next = ScopeLink::Parameters;
    assert!(encode(&f).is_err());
    let mut incomplete = raw(marker(0), 0);
    incomplete.code.push(ids::return_undef);
    incomplete.local_count = 1;
    assert!(encode(&incomplete).is_err());
}
#[test]
fn markers_reject_inconsistent_join_activity() {
    let mut code = vec![
        ids::push_true,
        ids::if_false,
        8,
        0,
        0,
        0,
        ids::set_loc_uninitialized,
        0,
        0,
        ids::nop,
    ];
    code.extend(marker(1));
    code.push(ids::return_undef);
    let mut f = lexical(code);
    f.stack_size = 1;
    assert!(encode(&f).is_err());
}
#[test]
fn close_reference_requires_explicit_scope_reentry_before_active_marker() {
    let mut code = vec![
        ids::set_loc_uninitialized,
        0,
        0,
        ids::push_1,
        ids::put_loc_check_init,
        0,
        0,
        ids::close_loc,
        0,
        0,
    ];
    code.extend(marker(1));
    code.push(ids::return_undef);
    let mut f = lexical(code);
    f.stack_size = 1;
    assert!(encode(&f).is_err());
    let mut code = vec![
        ids::set_loc_uninitialized,
        0,
        0,
        ids::push_1,
        ids::put_loc_check_init,
        0,
        0,
        ids::get_loc_check,
        0,
        0,
        ids::close_loc,
        0,
        0,
        ids::set_loc_uninitialized,
        0,
        0,
        ids::put_loc_check_init,
        0,
        0,
    ];
    code.extend(marker(1));
    code.push(ids::return_undef);
    let mut f = lexical(code);
    f.stack_size = 1;
    decode_and_verify(&encode(&f).unwrap()).unwrap();
}
#[test]
fn inner_marker_cannot_claim_only_outer_lexical_scope() {
    let mut f = lexical(vec![
        ids::set_loc_uninitialized,
        0,
        0,
        ids::set_loc_uninitialized,
        1,
        0,
    ]);
    f.local_count = 2;
    f.var_ref_count = 2;
    f.locals.push(VarDef {
        name: vec![b'y' as u16],
        scope_level: 2,
        scope_next: ScopeLink::Local(0),
        is_lexical: true,
        is_captured: true,
        var_ref_idx: 1,
        ..Default::default()
    });
    f.code.extend(marker(1));
    f.code.push(ids::return_undef);
    assert!(encode(&f).is_err());
    f.code[7..11].copy_from_slice(&2u32.to_le_bytes());
    decode_and_verify(&encode(&f).unwrap()).unwrap();
}
#[test]
fn active_marker_chain_cannot_reverse_outer_and_inner_scope_order() {
    let mut f = lexical(vec![
        ids::set_loc_uninitialized,
        0,
        0,
        ids::set_loc_uninitialized,
        1,
        0,
    ]);
    f.local_count = 2;
    f.var_ref_count = 2;
    f.locals[0].scope_level = 2;
    f.locals.push(VarDef {
        name: vec![b'y' as u16],
        scope_level: 1,
        scope_next: ScopeLink::Local(0),
        is_lexical: true,
        is_captured: true,
        var_ref_idx: 1,
        ..Default::default()
    });
    f.code.extend(marker(2));
    f.code.push(ids::return_undef);
    assert!(
        encode(&f).is_err(),
        "all active slots alone do not prove correct lexical shadowing order"
    );
}
#[test]
fn marker_positions_must_stay_on_instruction_boundaries() {
    let mut f = raw(marker(0), 0);
    f.code.push(ids::return_undef);
    f.debug_locations = vec![DebugLocation {
        code_offset: 0,
        line: 1,
        column: 1,
    }];
    decode_and_verify(&encode(&f).unwrap()).unwrap();
    f.debug_locations[0].code_offset = 1;
    assert!(encode(&f).is_err());
}

#[test]
fn malformed_debug_location_image_cannot_inherit_a_marker_position() {
    let mut code = vec![ids::nop];
    code.extend(marker(0));
    code.extend(marker(1 << 16));
    code.push(ids::return_undef);
    let mut f = raw(code, 0);
    f.debug_locations = vec![
        DebugLocation {
            code_offset: 0,
            line: 1,
            column: 1,
        },
        DebugLocation {
            code_offset: 1,
            line: 1,
            column: 8,
        },
        DebugLocation {
            code_offset: 6,
            line: 1,
            column: 17,
        },
    ];
    let mut bytes = encode(&f).unwrap();
    decode_and_verify(&bytes).unwrap();
    // The final fields are the initial line/column, a 12-byte delta table,
    // and the empty source. Remove PC1 and preserve the exact PC6 location.
    let trailer = [1, 1, 12, 0, 0, 0, 0, 0, 1, 0, 14, 0, 5, 0, 18, 0];
    assert!(bytes.ends_with(&trailer));
    bytes.truncate(bytes.len() - trailer.len());
    bytes.extend([1, 1, 8, 0, 0, 0, 0, 0, 6, 0, 32, 0]);
    // Recompute the pinned native checksum so rejection must come from the
    // marker/location contract rather than incidental byte corruption.
    let payload = &bytes[5..];
    let mut checksum = 0u32;
    let mut at = 0;
    while at + 4 < payload.len() {
        checksum = checksum
            .wrapping_add(u32::from_le_bytes(payload[at..at + 4].try_into().unwrap()))
            .wrapping_mul(0x9e370001);
        at += 4;
    }
    let mut tail = 0;
    // The pinned checksum intentionally ignores an exact four-byte tail.
    if payload.len() - at < 4 {
        for (shift, byte) in payload[at..].iter().enumerate() {
            tail |= u32::from(*byte) << (8 * shift);
        }
    }
    checksum = checksum.wrapping_add(tail).wrapping_mul(0x9e370001);
    bytes[1..5].copy_from_slice(&checksum.to_le_bytes());
    let error = decode_and_verify(&bytes)
        .expect_err("a preceding instruction's location cannot locate an executable marker");
    assert!(error.contains("exact debug location"), "{error}");
    f.debug_locations.remove(1);
    let error = encode(&f).unwrap_err();
    assert!(error.contains("exact debug location"), "{error}");
}

#[test]
fn stripped_marker_locations_remain_unknown_and_sparse_non_marker_locations_are_valid() {
    let mut code = vec![ids::nop];
    code.extend(marker(0));
    code.push(ids::return_undef);
    let mut f = raw(code, 0);
    let stripped = decode_and_verify(&encode(&f).unwrap()).unwrap();
    assert!(stripped.function().debug_locations.is_empty());
    f.debug_locations = vec![
        DebugLocation {
            code_offset: 0,
            line: 1,
            column: 1,
        },
        DebugLocation {
            code_offset: 1,
            line: 1,
            column: 8,
        },
    ];
    let sparse = decode_and_verify(&encode(&f).unwrap()).unwrap();
    assert_eq!(sparse.function().debug_locations, f.debug_locations);
}
