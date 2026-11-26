use qjs_js_parser_frontend::{compile_eval_program_input, source::SourceInput};
use qjs_scalar_ssa_ir::js::StatementKind;
fn input(source: &str) -> SourceInput {
    SourceInput::from_utf16(&source.encode_utf16().collect::<Vec<_>>()).unwrap()
}
#[test]
fn inherited_strictness_checks_original_script_and_nested_bindings() {
    for source in [
        "var eval=1;",
        "var arguments=1;",
        "var implements=1;",
        "010;",
        "'\\1';",
        "function f(a,a){return a;}",
    ] {
        assert!(
            compile_eval_program_input(&input(source), "strict.js", false).is_ok(),
            "loose {source}"
        );
        assert!(
            compile_eval_program_input(&input(source), "strict.js", true).is_err(),
            "strict {source}"
        );
    }
    let program = compile_eval_program_input(
        &input("let await=42;function f(){return await;}await;"),
        "strict.js",
        true,
    )
    .unwrap();
    assert!(program.strict);
    let StatementKind::Function { function, .. } = &program.body[1].kind else {
        panic!()
    };
    assert!(function.strict);
}
#[test]
fn inherited_strict_validation_preserves_source_spans_and_hashbang() {
    for source in [
        "let x=42;x;",
        "#!/usr/bin/env qjs\nlet x=42;x;",
        "#!/qjs\r\nlet x=42;x;",
        "#!/qjs",
    ] {
        let source = input(source);
        let program = compile_eval_program_input(&source, "original.js", true).unwrap();
        assert_eq!(program.source, source.original());
        assert_eq!(
            program.body.len(),
            if source.parser_text().ends_with("x;") {
                2
            } else {
                0
            }
        );
        if let Some(statement) = program.body.first() {
            let original = source
                .original_span(
                    source.parser_text().find("let").unwrap()
                        ..source.parser_text().find("let").unwrap() + 9,
                )
                .unwrap();
            assert_eq!(statement.span.start, original.start);
        }
    }
    let source = input("#!/qjs\n'😀';var eval=1;");
    let diagnostics = compile_eval_program_input(&source, "unicode.js", true).unwrap_err();
    let expected = source
        .original()
        .iter()
        .position(|u| *u == b'e' as u16)
        .unwrap() as u32;
    assert!(
        diagnostics.iter().any(|d| d.span.start == expected),
        "{diagnostics:?}"
    );
}

#[test]
fn inherited_strict_early_errors_match_the_pinned_reference() {
    let runner = qjs_reference_engine_bridge::ReferenceRunner::from_env();
    for source in [
        "var eval=1;",
        "var arguments=1;",
        "var implements=1;",
        "010;",
        "'\\1';",
        "function f(a,a){return a;}",
        "let await=42;function f(){return await;}f();",
        "let x=42;x;",
    ] {
        let units = source
            .encode_utf16()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        for strict in [false, true] {
            let directive = if strict { "'use strict';" } else { "" };
            let reference = format!(
                "(function(){{{directive}try{{eval(String.fromCharCode({units}));return false;}}catch(error){{return error instanceof SyntaxError;}}}})();"
            );
            let reference = runner.evaluate(&reference).unwrap();
            let ours = compile_eval_program_input(&input(source), "eval.js", strict).is_err();
            assert_eq!(
                reference,
                qjs_reference_engine_bridge::ReferenceValue::Boolean(ours),
                "strict={strict} source={source}"
            );
        }
    }
}
