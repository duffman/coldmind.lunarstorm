use qjs_compiler_core::{EmissionLimits, JsEmitOptions, compile_script, emit_program_with_limits};
#[test]
fn property_reference_operations_have_verified_images() {
    for source in [
        "let o={x:'40'};let old=o.x++;old+o.x;",
        "let o={x:40n};++o.x;",
        "let o={x:40};o.x+=2;",
        "let o={x:0};o.x||=42;",
        "let x=0;x||=42;",
        "let o={x:42};delete o.x;",
        "delete notDeclared;",
        "function f(){return delete x;let x;}f();",
    ] {
        compile_script(source, "property.js").unwrap_or_else(|e| panic!("{source}: {e}"));
    }
}
#[test]
fn property_reference_temporaries_obey_local_budget() {
    for source in [
        "let o={x:0};o.x+=42;",
        "let o={x:0};o.x++;",
        "let o={x:0};o.x||=42;",
    ] {
        let program =
            qjs_frontend::compile_program(source, "property.js", qjs_frontend::SourceMode::Script)
                .unwrap();
        let image = compile_script(source, "property.js").unwrap();
        let required = image.function().local_count as usize;
        assert!(required >= 3, "base, key and assigned-value temporaries");
        for maximum in [required - 1, required, required + 1] {
            let limits = EmissionLimits {
                max_locals: maximum,
                ..Default::default()
            };
            let result = emit_program_with_limits(&program, &JsEmitOptions::default(), &limits);
            if maximum < required {
                assert!(result.unwrap_err().starts_with("capacity:"), "{source}");
            } else {
                result.unwrap_or_else(|error| panic!("{source}: {error}"));
            }
        }
    }
}

#[test]
fn property_operations_rebind_selected_frame_references() {
    use qjs_bytecode::image::{
        ScopeLink,
        eval::{EnvBinding, EnvLayout, verify_env_layout},
    };
    let environment = verify_env_layout(EnvLayout {
        locals: vec![EnvBinding {
            name: "x".encode_utf16().collect(),
            is_lexical: true,
            is_captured: true,
            live: true,
            initialized: true,
            scope_level: 1,
            scope_next: ScopeLink::End,
            var_ref_idx: 0,
            ..Default::default()
        }],
        var_ref_count: 1,
        scope_head: ScopeLink::Local(0),
        ..Default::default()
    })
    .unwrap();
    for source in [
        "x ||= 42;",
        "let o={x:40};o[x]++;",
        "let o={x:40};o.x+=x;",
        "let o={x:40};o.x&&=x;",
        "delete x;",
    ] {
        let input = qjs_frontend::source::SourceInput::from_utf16(
            &source.encode_utf16().collect::<Vec<_>>(),
        )
        .unwrap();
        let image =
            qjs_compiler_core::eval::compile_eval(&input, "property-eval.js", &environment, false)
                .unwrap_or_else(|error| panic!("{source}: {error}"));
        assert_eq!(image.layout_digest(), environment.digest());
        assert_eq!(image.function().captures.len(), 1, "{source}");
        assert_eq!(image.function().captures[0].closure_type, 0, "{source}");
    }
}

#[test]
fn global_identifier_deletion_uses_runtime_binding_attributes() {
    let image = compile_script("var deletable;delete deletable;", "property.js").unwrap();
    let decoded = qjs_bytecode::decode_schema(
        &image.function().code,
        qjs_bytecode::Limits::default(),
        qjs_bytecode::production::FINAL_OPCODES,
    )
    .unwrap();
    assert!(
        decoded
            .instructions()
            .iter()
            .any(|i| i.opcode().name == "delete_var")
    );
}
