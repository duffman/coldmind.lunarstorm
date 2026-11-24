use qjs_bytecode::{image::*, production::ids};
fn function(strict: bool) -> FunctionImage {
    let mut f = FunctionImage {
        code: vec![
            ids::special_object,
            u8::from(!strict),
            ids::put_loc,
            0,
            0,
            ids::get_loc,
            0,
            0,
            ids::push_0,
            ids::get_array_el,
            ids::r#return,
        ],
        arg_count: 2,
        defined_arg_count: 2,
        local_count: 1,
        var_ref_count: 3,
        stack_size: 2,
        strict,
        locals: vec![
            VarDef {
                name: vec![b'a' as u16],
                is_captured: true,
                var_ref_idx: 0,
                ..Default::default()
            },
            VarDef {
                name: vec![b'b' as u16],
                is_captured: true,
                var_ref_idx: 1,
                ..Default::default()
            },
            VarDef {
                name: "arguments".encode_utf16().collect(),
                var_kind: VAR_KIND_ARGUMENTS,
                is_captured: true,
                var_ref_idx: 2,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    f.flags.arguments_allowed = true;
    f
}
#[test]
fn exact_strict_and_mapped_argument_initializers_are_admissible() {
    for strict in [true, false] {
        let f = function(strict);
        decode_and_verify(&encode(&f).unwrap()).unwrap();
    }
}
#[test]
fn mapping_requires_real_unique_captured_argument_slots() {
    let mut f = function(false);
    f.locals[0].is_captured = false;
    assert!(encode(&f).is_err());
    let mut f = function(false);
    f.locals[0].var_ref_idx = 1;
    assert!(encode(&f).is_err());
    let mut f = function(false);
    f.locals[0].name = f.locals[1].name.clone();
    assert!(encode(&f).is_err());
    f.locals[0].name.clear();
    decode_and_verify(&encode(&f).unwrap()).unwrap();
    let mut f = function(false);
    f.locals[0].name = "arguments".encode_utf16().collect();
    assert!(encode(&f).is_err());
    let mut f = function(false);
    f.locals[0].scope_next = ScopeLink::Parameters;
    assert!(encode(&f).is_err());
}
#[test]
fn only_supported_arguments_variants_and_binding_metadata_are_admitted() {
    for strict in [true, false] {
        let mut f = function(strict);
        f.code[1] = u8::from(strict);
        assert!(encode(&f).is_err());
        // Non-simple parameters need an unmapped arguments object: the strict fixture has
        // one, the sloppy fixture's mapped one is refused.
        let mut f = function(strict);
        f.flags.simple_parameters = false;
        if strict {
            decode_and_verify(&encode(&f).unwrap()).unwrap();
        } else {
            assert!(encode(&f).is_err());
        }
        let mut f = function(strict);
        f.flags.arguments_allowed = false;
        assert!(encode(&f).is_err());
        let mut f = function(strict);
        f.defined_arg_count = 1;
        assert!(encode(&f).is_err());
        let mut f = function(strict);
        f.locals[2].is_captured = false;
        assert!(encode(&f).is_err());
        let mut f = function(strict);
        f.locals[2].name.clear();
        assert!(encode(&f).is_err());
        let mut f = function(strict);
        f.code[2] = ids::set_loc;
        assert!(encode(&f).is_err());
    }
    for value in [3, 4, 5, 6, 7, 255] {
        let mut f = function(true);
        f.code[1] = value;
        assert!(encode(&f).is_err());
    }
}
#[test]
fn arguments_object_is_created_once_before_effects_and_cannot_be_reentered() {
    let mut f = function(false);
    f.code
        .splice(5..5, [ids::special_object, 1, ids::put_loc, 0, 0]);
    assert!(encode(&f).is_err());
    let mut f = function(false);
    f.code.splice(0..0, [ids::push_1, ids::put_arg, 0, 0]);
    assert!(encode(&f).is_err());
    let mut f = function(false);
    f.code.extend([ids::goto, 244, 255, 255, 255]);
    assert!(encode(&f).is_err());
}
#[test]
fn arguments_identity_uses_one_function_scope_binding() {
    let mut f = function(false);
    f.locals.push(VarDef {
        name: "arguments".encode_utf16().collect(),
        is_captured: true,
        var_ref_idx: 3,
        ..Default::default()
    });
    f.local_count += 1;
    f.var_ref_count += 1;
    assert!(encode(&f).is_err());
}

#[test]
fn materialized_arguments_tag_requires_verified_initializer() {
    let mut f = function(true);
    f.locals[2].var_kind = VAR_KIND_ARGUMENTS;
    f.code.drain(..5);
    assert!(
        encode(&f).is_err(),
        "forged materialized-arguments tag without initializer was admitted"
    );
}

#[test]
fn materialized_arguments_marker_cannot_move_or_lose_capture_identity() {
    let mut f = function(true);
    f.constants.push(Constant::Function(Box::new(FunctionImage {
        code: vec![ids::get_var_ref, 0, 0, ids::r#return],
        stack_size: 1,
        captures: vec![Capture {
            name: "arguments".encode_utf16().collect(),
            var_idx: 0,
            closure_type: 0,
            var_kind: 12,
            ..Default::default()
        }],
        ..Default::default()
    })));
    assert!(encode(&f).is_ok());
    let Constant::Function(child) = &mut f.constants[0] else {
        panic!()
    };
    child.captures[0].var_kind = 0;
    assert!(
        encode(&f).is_err(),
        "materialized-arguments provenance cannot be dropped in capture"
    );
    let Constant::Function(child) = &mut f.constants[0] else {
        panic!()
    };
    child.captures[0].var_kind = 12;
    child.captures[0].name = "renamed".encode_utf16().collect();
    assert!(
        encode(&f).is_err(),
        "materialized-arguments provenance cannot rename a binding"
    );
}

#[test]
fn materialized_arguments_marker_cannot_be_misplaced_or_forged_on_body_cell() {
    let mut f = function(true);
    f.locals[2].var_kind = 0;
    f.locals[0].var_kind = VAR_KIND_ARGUMENTS;
    assert!(
        encode(&f).is_err(),
        "raw argv cannot claim the materialized arguments tag"
    );
    let mut f = function(true);
    f.locals.push(VarDef {
        name: "other".encode_utf16().collect(),
        is_captured: true,
        var_ref_idx: 3,
        var_kind: VAR_KIND_ARGUMENTS,
        ..Default::default()
    });
    f.local_count += 1;
    f.var_ref_count += 1;
    assert!(
        encode(&f).is_err(),
        "an unrelated body slot cannot forge arguments initialization"
    );
}
