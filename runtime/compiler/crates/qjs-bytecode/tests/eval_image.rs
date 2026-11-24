use qjs_bytecode::{
    image::{eval::*, *},
    production::ids,
};
fn slot(name: &str, index: u32, lexical: bool, scope: u32) -> EnvBinding {
    EnvBinding {
        name: name.encode_utf16().collect(),
        is_lexical: lexical,
        scope_level: scope,
        is_captured: true,
        var_ref_idx: index,
        initialized: true,
        live: true,
        ..Default::default()
    }
}
fn environment() -> EnvLayout {
    EnvLayout {
        args: vec![slot("a", 0, false, 0)],
        locals: vec![
            slot("x", 1, false, 0),
            slot("x", 2, true, 1),
            slot("inactive", 3, true, 2),
        ],
        refs: vec![EnvBinding {
            name: "outer".encode_utf16().collect(),
            initialized: true,
            live: true,
            ..Default::default()
        }],
        var_ref_count: 4,
        scope_head: ScopeLink::Local(1),
        ..Default::default()
    }
}
fn reader(name: &str, kind: u8, index: u32, lexical: bool) -> FunctionImage {
    FunctionImage {
        code: vec![
            if lexical {
                ids::get_var_ref_check
            } else {
                ids::get_var_ref
            },
            0,
            0,
            ids::r#return,
        ],
        stack_size: 1,
        function_name: "<eval>".into(),
        filename: "eval.js".into(),
        captures: vec![Capture {
            name: name.encode_utf16().collect(),
            var_idx: index,
            closure_type: kind,
            is_lexical: lexical,
            ..Default::default()
        }],
        ..Default::default()
    }
}
#[test]
fn eval_proof_is_distinct_and_supports_only_exact_visible_live_slots() {
    let env = verify_env_layout(environment()).unwrap();
    assert_eq!(
        env.bindings().iter().map(|b| b.slot).collect::<Vec<_>>(),
        vec![Slot::Local(1), Slot::Arg(0), Slot::Ref(0)]
    );
    for f in [
        reader("x", 0, 1, true),
        reader("a", 1, 0, false),
        reader("outer", 2, 0, false),
    ] {
        assert!(encode(&f).is_err());
        let bytes = encode_eval(&f, &env).unwrap();
        assert!(decode_and_verify(&bytes).is_err());
        let proof = decode_eval_and_verify(&bytes, &env).unwrap();
        assert_eq!(proof.function(), &f);
        assert_eq!(proof.layout_digest(), env.digest());
        assert_eq!(proof.layout(), &env);
    }
    for f in [
        reader("x", 0, 0, false),
        reader("inactive", 0, 2, true),
        reader("outer", 3, 0, false),
        reader("a", 1, 1, false),
    ] {
        assert!(encode_eval(&f, &env).is_err());
    }
}
#[test]
fn eval_writes_preserve_tdz_const_and_external_initialization_rules() {
    let env = verify_env_layout(environment()).unwrap();
    let mut f = reader("x", 0, 1, true);
    f.code = vec![
        ids::push_i8,
        42,
        ids::dup,
        ids::put_var_ref_check,
        0,
        0,
        ids::r#return,
    ];
    f.stack_size = 2;
    decode_eval_and_verify(&encode_eval(&f, &env).unwrap(), &env).unwrap();
    f.code[3] = ids::put_var_ref_check_init;
    assert!(encode_eval(&f, &env).is_err());
    f.code[3] = ids::put_var_ref;
    assert!(encode_eval(&f, &env).is_err());
    let mut raw = environment();
    raw.locals[1].initialized = false;
    let env = verify_env_layout(raw).unwrap();
    decode_eval_and_verify(&encode_eval(&reader("x", 0, 1, true), &env).unwrap(), &env).unwrap();
    let mut f = reader("x", 0, 1, true);
    f.code[0] = ids::get_var_ref;
    assert!(encode_eval(&f, &env).is_err());
    let mut raw = environment();
    raw.locals[1].is_const = true;
    let env = verify_env_layout(raw).unwrap();
    let mut f = reader("x", 0, 1, true);
    f.captures[0].is_const = true;
    f.code = vec![ids::push_1, ids::put_var_ref_check, 0, 0, ids::return_undef];
    assert!(encode_eval(&f, &env).is_err());
}
#[test]
fn eval_layout_rejects_forged_links_targets_and_missing_live_references() {
    let mut raw = environment();
    raw.target_fingerprint = "wrong".into();
    assert!(verify_env_layout(raw).is_err());
    let mut raw = environment();
    raw.scope_head = ScopeLink::Local(4);
    assert!(verify_env_layout(raw).is_err());
    let mut raw = environment();
    raw.locals[1].scope_next = ScopeLink::Local(2);
    raw.locals[2].scope_next = ScopeLink::Local(1);
    assert!(verify_env_layout(raw).is_err());
    let mut raw = environment();
    raw.locals[1].var_ref_idx = 4;
    assert!(verify_env_layout(raw).is_err());
    let mut raw = environment();
    raw.locals[1].var_ref_idx = 0;
    assert!(verify_env_layout(raw).is_err());
    let mut raw = environment();
    raw.locals[1].live = false;
    let env = verify_env_layout(raw).unwrap();
    assert!(encode_eval(&reader("x", 0, 1, true), &env).is_err());
    let mut raw = environment();
    raw.refs[0].live = false;
    let env = verify_env_layout(raw).unwrap();
    assert!(encode_eval(&reader("outer", 2, 0, false), &env).is_err());
    let mut raw = environment();
    raw.locals[1].is_captured = false;
    assert!(verify_env_layout(raw).is_err());
}
#[test]
fn inherited_strictness_permissions_and_receiver_effects_are_checked() {
    let mut raw = environment();
    raw.strict = true;
    let env = verify_env_layout(raw).unwrap();
    let mut f = reader("x", 0, 1, true);
    assert!(encode_eval(&f, &env).is_err());
    f.strict = true;
    encode_eval(&f, &env).unwrap();
    for code in [
        vec![ids::push_this, ids::r#return],
        vec![ids::special_object, 2, ids::r#return],
        vec![ids::define_var, 0, 0, 0, 0, 0, ids::return_undef],
    ] {
        let f = FunctionImage {
            code,
            stack_size: 1,
            strict: true,
            atoms: vec![vec![b'a' as u16]],
            ..Default::default()
        };
        assert!(encode_eval(&f, &env).is_err());
    }
    let mut f = reader("x", 0, 1, true);
    f.strict = true;
    f.flags.new_target = true;
    assert!(encode_eval(&f, &env).is_err());
}
#[test]
fn parameter_tail_roundtrips_without_claiming_parameter_eval_support() {
    assert_eq!(ScopeLink::from_signed(-2).unwrap(), ScopeLink::Parameters);
    assert_eq!(ScopeLink::Parameters.to_signed().unwrap(), -2);
    assert!(ScopeLink::from_signed(-3).is_err());
    assert!(ScopeLink::Local(u32::MAX).to_signed().is_err());
    let mut f = FunctionImage {
        code: vec![ids::return_undef],
        local_count: 1,
        var_ref_count: 1,
        locals: vec![VarDef {
            scope_level: 1,
            scope_next: ScopeLink::Parameters,
            is_lexical: true,
            is_captured: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    f.flags.simple_parameters = false;
    assert_eq!(
        decode_and_verify(&encode(&f).unwrap())
            .unwrap()
            .function()
            .locals[0]
            .scope_next,
        ScopeLink::Parameters
    );
    let mut raw = environment();
    raw.locals[1].scope_next = ScopeLink::Parameters;
    let env = verify_env_layout(raw).unwrap();
    assert!(env.parameter_scope());
    assert!(env.binding(&[b'a' as u16]).is_none());
    assert!(encode_eval(&reader("x", 0, 1, true), &env).is_err());
}
#[test]
fn layout_digest_binds_metadata_and_verified_copy_is_immutable() {
    let mut raw = environment();
    let first = verify_env_layout(raw.clone()).unwrap();
    raw.locals[1].initialized = false;
    let second = verify_env_layout(raw).unwrap();
    assert_ne!(first.digest(), second.digest());
    assert!(first.layout().locals[1].initialized);
    let proof = decode_eval_and_verify(
        &encode_eval(&reader("x", 0, 1, true), &first).unwrap(),
        &first,
    )
    .unwrap();
    assert_eq!(proof.layout_digest(), first.digest());
}

#[test]
fn hidden_context_bindings_are_exact_visible_readonly_slots() {
    let raw = EnvLayout {
        locals: vec![slot("this", 0, false, 0), slot("new.target", 1, false, 0)],
        var_ref_count: 2,
        ..Default::default()
    };
    let env = verify_env_layout(raw).unwrap();
    assert_eq!(
        env.hidden(HiddenBinding::This).unwrap().slot,
        Slot::Local(0)
    );
    assert_eq!(
        env.hidden(HiddenBinding::NewTarget).unwrap().slot,
        Slot::Local(1)
    );
    assert!(env.hidden(HiddenBinding::Arguments).is_none());
    encode_eval(&reader("this", 0, 0, false), &env).unwrap();
    let mut f = reader("this", 0, 0, false);
    f.code = vec![ids::undefined, ids::put_var_ref, 0, 0, ids::return_undef];
    assert!(encode_eval(&f, &env).is_err());
}

#[test]
fn parameter_layout_preserves_only_proven_materialized_arguments_cell() {
    let mut actual = slot("arguments", 0, false, 0);
    actual.var_kind = VAR_KIND_ARGUMENTS;
    let mut layout = EnvLayout {
        locals: vec![
            actual,
            slot("arguments", 1, false, 0),
            EnvBinding {
                scope_next: ScopeLink::Parameters,
                ..slot("", 2, true, 1)
            },
            EnvBinding {
                scope_next: ScopeLink::Local(2),
                var_kind: VAR_KIND_BODY_ARGUMENTS_ANCHOR,
                ..slot("", 3, true, 2)
            },
        ],
        var_ref_count: 4,
        scope_head: ScopeLink::Local(2),
        ..Default::default()
    };
    layout.flags.arguments_allowed = true;
    layout.flags.simple_parameters = false;
    let proof = verify_env_layout(layout.clone()).unwrap();
    assert_eq!(
        proof.bindings().iter().map(|v| v.slot).collect::<Vec<_>>(),
        vec![Slot::Local(0)]
    );
    layout.scope_head = ScopeLink::Local(3);
    let body = verify_env_layout(layout.clone()).unwrap();
    assert_eq!(
        body.bindings().iter().map(|v| v.slot).collect::<Vec<_>>(),
        vec![Slot::Local(1)]
    );
    layout.locals.push(EnvBinding {
        scope_next: ScopeLink::Local(2),
        ..slot("class_local", 4, true, 3)
    });
    layout.var_ref_count = 5;
    layout.scope_head = ScopeLink::Local(4);
    let nested = verify_env_layout(layout.clone()).unwrap();
    assert!(
        nested.parameter_scope(),
        "a nested parameter lexical scope is not a body anchor"
    );
    assert_eq!(
        nested
            .binding(&"arguments".encode_utf16().collect::<Vec<_>>())
            .unwrap()
            .slot,
        Slot::Local(0)
    );
    let captured = EnvLayout {
        refs: vec![layout.locals[0].clone()],
        ..Default::default()
    };
    let closure = verify_env_layout(captured).unwrap();
    assert_eq!(
        closure.bindings()[0].slot,
        Slot::Ref(0),
        "genuine parameter-cell captures remain visible"
    );
    layout.locals[1].var_kind = VAR_KIND_ARGUMENTS;
    assert!(
        verify_env_layout(layout).is_err(),
        "two same-named materialized arguments proofs are invalid"
    );
}
