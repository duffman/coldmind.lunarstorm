use qjs_bytecode::{
    image::{eval::*, *},
    production::ids,
};

fn property_delete(strict: bool) -> FunctionImage {
    FunctionImage {
        code: vec![
            ids::object,
            ids::push_atom_value,
            0,
            0,
            0,
            0,
            ids::delete,
            ids::r#return,
        ],
        atoms: vec!["key".encode_utf16().collect()],
        stack_size: 2,
        strict,
        ..Default::default()
    }
}
fn global_delete() -> FunctionImage {
    FunctionImage {
        code: vec![ids::delete_var, 0, 0, 0, 0, ids::r#return],
        atoms: vec!["globalKey".encode_utf16().collect()],
        stack_size: 1,
        strict: false,
        ..Default::default()
    }
}
#[test]
fn property_delete_accepts_exact_two_to_one_effect_in_both_modes() {
    for strict in [false, true] {
        let f = property_delete(strict);
        assert_eq!(
            decode_and_verify(&encode(&f).unwrap()).unwrap().function(),
            &f
        );
    }
}
#[test]
fn property_delete_rejects_underflow_leftovers_and_undersized_stack() {
    for code in [
        vec![ids::delete, ids::r#return],
        vec![ids::object, ids::delete, ids::r#return],
        vec![
            ids::push_0,
            ids::object,
            ids::push_0,
            ids::delete,
            ids::r#return,
        ],
    ] {
        let mut f = property_delete(false);
        f.code = code;
        f.stack_size = 3;
        assert!(encode(&f).is_err());
    }
    let mut f = property_delete(false);
    f.stack_size = 1;
    assert!(encode(&f).is_err());
}
#[test]
fn global_delete_validates_atom_and_sloppy_only_semantics() {
    let f = global_delete();
    assert_eq!(
        decode_and_verify(&encode(&f).unwrap()).unwrap().function(),
        &f
    );
    let mut f = global_delete();
    f.strict = true;
    assert!(encode(&f).unwrap_err().contains("strict"));
    let mut f = global_delete();
    f.code[1] = 1;
    assert!(encode(&f).unwrap_err().contains("atom"));
    let mut f = global_delete();
    f.code.truncate(4);
    assert!(encode(&f).is_err());
}
#[test]
fn eval_identifier_delete_requires_name_absent_from_external_frame() {
    let env = verify_env_layout(EnvLayout::default()).unwrap();
    let f = property_delete(false);
    let bytes = encode_eval(&f, &env).unwrap();
    assert_eq!(decode_eval_and_verify(&bytes, &env).unwrap().function(), &f);
    let f = global_delete();
    let bytes = encode_eval(&f, &env).unwrap();
    decode_eval_and_verify(&bytes, &env).unwrap();
    let binding = EnvBinding {
        name: f.atoms[0].clone(),
        is_captured: true,
        initialized: true,
        live: true,
        ..Default::default()
    };
    for raw in [
        EnvLayout {
            args: vec![binding.clone()],
            var_ref_count: 1,
            ..Default::default()
        },
        EnvLayout {
            locals: vec![binding.clone()],
            var_ref_count: 1,
            ..Default::default()
        },
        EnvLayout {
            refs: vec![binding.clone()],
            ..Default::default()
        },
        EnvLayout {
            locals: vec![EnvBinding {
                is_lexical: true,
                initialized: false,
                scope_level: 1,
                ..binding
            }],
            var_ref_count: 1,
            scope_head: ScopeLink::Local(0),
            ..Default::default()
        },
    ] {
        let env = verify_env_layout(raw).unwrap();
        assert!(encode_eval(&f, &env).unwrap_err().contains("frame binding"));
        // Root captures can be empty: the opcode must still respect the external
        // environment, including bindings currently in their TDZ.
        assert!(decode_eval_and_verify(&bytes, &env).is_err());
    }
}
#[test]
fn unrelated_dynamic_scope_deletion_remains_unsupported() {
    let mut f = global_delete();
    f.code = vec![
        ids::push_0,
        ids::with_delete_var,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        ids::return_undef,
    ];
    assert!(encode(&f).is_err());
}
