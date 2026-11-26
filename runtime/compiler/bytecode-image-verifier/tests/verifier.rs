use qjs_bytecode_image_verifier::{ids, snapshot::*, verify::*};
fn sample(code: Vec<u8>) -> Snapshot {
    Snapshot {
        code,
        constants: vec![],
        local_count: 1,
        arg_count: 0,
        stack_size: 2,
        source_len: 10,
        positions: vec![],
        function_name: "main".into(),
        provenance: Provenance::QuickJsFinalized,
        target_fingerprint: "test".into(),
    }
}
#[test]
fn verifies_scalar_return() {
    assert!(verify(sample(vec![ids::push_1, ids::r#return]), "test").is_ok());
}
#[test]
fn bytecode_verifier_rejects_corrupt_code_and_metadata() {
    for code in [
        vec![],
        vec![ids::r#return],
        vec![ids::push_i32],
        vec![ids::push_1],
        vec![ids::get_loc0, ids::r#return],
        vec![ids::push_1, ids::throw],
        vec![ids::push_const8, 0, ids::r#return],
        vec![ids::goto8, 127],
    ] {
        assert!(verify(sample(code.clone()), "test").is_err(), "{code:?}");
    }
    let mut s = sample(vec![ids::push_1, ids::r#return]);
    s.stack_size = 0;
    assert!(verify(s, "test").is_err());
    let mut s = sample(vec![ids::push_1, ids::r#return]);
    s.positions.push(Position {
        code_offset: 0,
        source_start: 0,
        source_end: 11,
    });
    assert!(verify(s, "test").is_err());
}
#[test]
fn target_fingerprint_mismatch_is_rejected() {
    assert!(verify(sample(vec![ids::push_1, ids::r#return]), "other").is_err());
}
#[test]
fn implicit_local_indices_are_checked_even_in_unreachable_code() {
    let mut s = sample(vec![ids::push_1, ids::r#return, ids::get_loc3]);
    s.local_count = 1;
    assert!(verify(s, "test").is_err());
    let mut s = sample(vec![ids::get_loc0_loc1, ids::add, ids::r#return]);
    s.local_count = 1;
    assert!(verify(s, "test").is_err());
}
#[test]
fn joins_require_equal_stack_depth_and_all_paths_initialized() {
    // true -> direct read; false -> initialized read. Definite initialization must intersect.
    let s = sample(vec![
        ids::push_true,
        ids::if_true8,
        4,
        ids::push_1,
        ids::put_loc0,
        ids::nop,
        ids::get_loc0,
        ids::r#return,
    ]);
    assert!(verify(s, "test").is_err());
    let s = sample(vec![
        ids::push_true,
        ids::if_true8,
        4,
        ids::push_1,
        ids::goto8,
        2,
        ids::nop,
        ids::push_1,
        ids::r#return,
    ]);
    assert!(verify(s, "test").is_err());
}
#[test]
fn stores_initialize_and_set_preserves_stack() {
    assert!(verify(
        sample(vec![
            ids::push_1,
            ids::set_loc0,
            ids::drop,
            ids::get_loc0,
            ids::r#return
        ]),
        "test"
    )
    .is_ok());
    assert!(verify(
        sample(vec![
            ids::push_1,
            ids::put_loc0,
            ids::get_loc0,
            ids::r#return
        ]),
        "test"
    )
    .is_ok());
}
#[test]
fn rejects_exception_and_non_scalar_opcodes() {
    for op in [
        ids::catch,
        ids::gosub,
        ids::ret,
        ids::return_undef,
        ids::fclosure8,
        ids::r#not,
        ids::get_arg0,
        ids::get_var_ref0,
    ] {
        assert!(verify(
            sample(vec![op, 0, 0, 0, 0, ids::push_1, ids::r#return]),
            "test"
        )
        .is_err());
    }
}
#[test]
fn rejects_unknown_provenance_and_extra_return_stack_values() {
    let mut s = sample(vec![ids::push_1, ids::r#return]);
    s.provenance = Provenance::Unknown;
    assert!(verify(s, "test").is_err());
    assert!(verify(
        sample(vec![ids::push_1, ids::push_1, ids::r#return]),
        "test"
    )
    .is_err());
}
#[test]
fn validates_constant_widths_source_boundaries_and_state_budget() {
    let mut s = sample(vec![ids::push_const8, 0, ids::r#return]);
    s.constants = vec![Constant::Number(-0.0)];
    assert!(verify(s.clone(), "test").is_ok());
    s.positions.push(Position {
        code_offset: 1,
        source_start: 0,
        source_end: 1,
    });
    assert!(verify(s, "test").is_err());
    let mut s = sample(vec![ids::nop; 200]);
    s.local_count = 65535;
    assert!(verify(s, "test").unwrap_err().message.contains("budget"));
    let mut s = sample(vec![ids::push_1, ids::r#return]);
    s.arg_count = 1;
    assert!(verify(s, "test").is_err());
    let mut s = sample(vec![ids::push_1, ids::r#return]);
    s.function_name = "other".into();
    assert!(verify(s, "test").is_err());
}

#[test]
fn every_scalar_push_width_obeys_stack_capacity() {
    let mut pushes = vec![
        vec![ids::push_minus1],
        vec![ids::push_0],
        vec![ids::push_1],
        vec![ids::push_2],
        vec![ids::push_3],
        vec![ids::push_4],
        vec![ids::push_5],
        vec![ids::push_6],
        vec![ids::push_7],
        vec![ids::push_true],
        vec![ids::push_false],
        vec![ids::push_i8, 127],
        vec![ids::push_const8, 0],
    ];
    for (op, bytes) in [
        (ids::push_i16, 42i16.to_ne_bytes().to_vec()),
        (ids::push_i32, 42i32.to_ne_bytes().to_vec()),
        (ids::push_const, 0u32.to_ne_bytes().to_vec()),
    ] {
        let mut code = vec![op];
        code.extend(bytes);
        pushes.push(code);
    }
    for mut code in pushes {
        code.push(ids::r#return);
        let mut s = sample(code);
        s.constants = vec![Constant::Number(-0.0)];
        assert!(verify(s.clone(), "test").is_ok());
        s.stack_size = 0;
        assert!(verify(s, "test").is_err());
    }
    for code in [vec![ids::push_const8, 1, ids::r#return], {
        let mut c = vec![ids::push_const];
        c.extend(1u32.to_ne_bytes());
        c.push(ids::r#return);
        c
    }] {
        let mut s = sample(code);
        s.constants = vec![Constant::Bool(true)];
        assert!(verify(s, "test").is_err());
    }
}

#[test]
fn every_local_width_checks_initialization_index_and_set_result() {
    let mut forms = vec![
        (
            0,
            vec![ids::get_loc0],
            vec![ids::put_loc0],
            vec![ids::set_loc0],
        ),
        (
            1,
            vec![ids::get_loc1],
            vec![ids::put_loc1],
            vec![ids::set_loc1],
        ),
        (
            2,
            vec![ids::get_loc2],
            vec![ids::put_loc2],
            vec![ids::set_loc2],
        ),
        (
            3,
            vec![ids::get_loc3],
            vec![ids::put_loc3],
            vec![ids::set_loc3],
        ),
        (
            7,
            vec![ids::get_loc8, 7],
            vec![ids::put_loc8, 7],
            vec![ids::set_loc8, 7],
        ),
    ];
    let encoded = |op| {
        let mut c = vec![op];
        c.extend(256u16.to_ne_bytes());
        c
    };
    forms.push((
        256,
        encoded(ids::get_loc),
        encoded(ids::put_loc),
        encoded(ids::set_loc),
    ));
    for (index, get, put, set) in forms {
        for store in [put, set.clone()] {
            let mut code = vec![ids::push_1];
            code.extend(&store);
            if store == set {
                code.push(ids::drop);
            }
            code.extend(&get);
            code.push(ids::r#return);
            let mut s = sample(code);
            s.local_count = index + 1;
            assert!(verify(s.clone(), "test").is_ok());
            s.local_count = index;
            assert!(verify(s, "test").is_err());
        }
        let mut code = get;
        code.push(ids::r#return);
        let mut s = sample(code);
        s.local_count = index + 1;
        assert!(verify(s, "test").is_err());
    }
    let mut s = sample(vec![
        ids::push_1,
        ids::put_loc0,
        ids::push_2,
        ids::put_loc1,
        ids::get_loc0_loc1,
        ids::add,
        ids::r#return,
    ]);
    s.local_count = 2;
    assert!(verify(s.clone(), "test").is_ok());
    s.stack_size = 1;
    assert!(verify(s, "test").is_err());
}

#[test]
fn every_branch_width_uses_operand_relative_base() {
    for (op, width) in [(ids::goto8, 2), (ids::goto16, 3), (ids::goto, 5)] {
        let mut code = vec![ids::push_1, op];
        match width {
            2 => code.push(width),
            3 => code.extend(i16::from(width).to_ne_bytes()),
            _ => code.extend(i32::from(width).to_ne_bytes()),
        }
        // Skip a drop which would empty the stack before return.
        code.extend([ids::drop, ids::r#return]);
        assert!(verify(sample(code), "test").is_ok());
    }
    for (op, width) in [
        (ids::if_true8, 2),
        (ids::if_false8, 2),
        (ids::if_true, 5),
        (ids::if_false, 5),
    ] {
        let mut code = vec![ids::push_true, op];
        let delta = width + 1;
        if width == 2 {
            code.push(delta)
        } else {
            code.extend(i32::from(delta).to_ne_bytes())
        }
        code.extend([ids::push_1, ids::r#return, ids::push_2, ids::r#return]);
        assert!(verify(sample(code.clone()), "test").is_ok());
        // Both branches require a condition; deleting its push causes underflow.
        code.remove(0);
        assert!(verify(sample(code), "test").is_err());
    }
}

#[test]
fn scalar_operators_dup_drop_and_nop_have_independent_stack_checks() {
    for op in [
        ids::mul,
        ids::div,
        ids::r#mod,
        ids::add,
        ids::sub,
        ids::strict_eq,
        ids::strict_neq,
        ids::lt,
        ids::lte,
        ids::gt,
        ids::gte,
    ] {
        assert!(verify(
            sample(vec![ids::push_1, ids::push_2, op, ids::r#return]),
            "test"
        )
        .is_ok());
        assert!(verify(sample(vec![ids::push_1, op, ids::r#return]), "test").is_err());
    }
    for (push, op) in [(ids::push_1, ids::neg), (ids::push_true, ids::lnot)] {
        assert!(verify(sample(vec![push, op, ids::r#return]), "test").is_ok());
        assert!(verify(sample(vec![op, ids::r#return]), "test").is_err());
    }
    assert!(verify(
        sample(vec![
            ids::push_1,
            ids::dup,
            ids::drop,
            ids::nop,
            ids::r#return
        ]),
        "test"
    )
    .is_ok());
    assert!(verify(sample(vec![ids::dup, ids::r#return]), "test").is_err());
    assert!(verify(sample(vec![ids::drop, ids::push_1, ids::r#return]), "test").is_err());
}
