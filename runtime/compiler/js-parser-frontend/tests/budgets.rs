//! Frontend budgets: AST node, nesting, function and binding limits, capacity
//! diagnostics, and limits that cannot be loosened.

use qjs_js_parser_frontend::{Category, SourceMode, compile_program};
#[test]
fn default_frontend_rejects_excessive_original_ast_nesting_before_semantics() {
    // The larger test stack protects the uninstrumented parser, not the AST pass.
    let result = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let source = format!("{}42;{}", "{".repeat(256), "}".repeat(256));
            compile_program(&source, "nesting.js", SourceMode::Script)
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(
        matches!(result,Err(ref errors) if errors.iter().any(|error|error.category==Category::Capacity)),
        "over-limit AST reached semantic/lowering: {result:?}"
    );
}

use qjs_js_parser_frontend::{
    CompileLimits, ParsePhase, compile_eval_program_input_with_limits,
    compile_eval_program_input_with_limits_and_observer, compile_module_input_with_limits,
    compile_program_input_with_limits, compile_program_input_with_limits_and_observer,
    compile_program_with_limits, source::SourceInput,
};
fn bounded(
    source: &str,
    limits: CompileLimits,
) -> Result<qjs_scalar_ssa_ir::js::Program, Vec<qjs_js_parser_frontend::Diagnostic>> {
    compile_program_with_limits(source, "bounded.js", SourceMode::Script, &limits)
}
fn capacity(result: Result<qjs_scalar_ssa_ir::js::Program, Vec<qjs_js_parser_frontend::Diagnostic>>, resource: &str) {
    let errors = result.unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].category, Category::Capacity);
    assert!(errors[0].message.contains(resource), "{errors:?}");
}
#[test]
fn original_ast_node_budget_accepts_limit_minus_one_and_limit_rejects_plus_one() {
    let limits = CompileLimits {
        max_ast_nodes: 3,
        ..Default::default()
    };
    bounded(";", limits).unwrap();
    bounded("42;", limits).unwrap();
    capacity(bounded("let x;", limits), "node");
}
#[test]
fn ast_nesting_budget_accepts_limit_minus_one_and_limit_rejects_plus_one() {
    let limits = CompileLimits {
        max_nesting: 3,
        ..Default::default()
    };
    bounded(";", limits).unwrap();
    bounded("42;", limits).unwrap();
    capacity(bounded("-42;", limits), "nesting");
}
#[test]
fn function_budget_accepts_limit_minus_one_and_limit_rejects_plus_one() {
    let limits = CompileLimits {
        max_functions: 2,
        ..Default::default()
    };
    bounded("function f(){}", limits).unwrap();
    bounded("function f(){}function g(){}", limits).unwrap();
    capacity(
        bounded("function f(){}function g(){}function h(){}", limits),
        "function",
    );
    // Unsupported forms are still budgeted before expensive semantic analysis.
    capacity(
        bounded(
            "(()=>42);",
            CompileLimits {
                max_functions: 0,
                ..Default::default()
            },
        ),
        "function",
    );
}
#[test]
fn source_binding_occurrences_are_bounded_before_symbol_merging() {
    let limits = CompileLimits {
        max_bindings: 2,
        ..Default::default()
    };
    bounded("let a;", limits).unwrap();
    bounded("let a;let b;", limits).unwrap();
    capacity(bounded("let a;let b;let c;", limits), "binding");
    capacity(bounded("var a;var a;var a;", limits), "binding");
}
#[test]
fn every_general_goal_applies_limits_before_semantic_errors() {
    let input =
        SourceInput::from_utf16(&"let a;let a;".encode_utf16().collect::<Vec<_>>()).unwrap();
    let limits = CompileLimits {
        max_bindings: 1,
        ..Default::default()
    };
    for result in [
        compile_program_input_with_limits(&input, "script.js", &limits),
        compile_module_input_with_limits(&input, "module.mjs", &limits),
        compile_eval_program_input_with_limits(&input, "eval.js", true, &limits),
    ] {
        capacity(result, "binding");
    }
    let errors = compile_program_input_with_limits(&input, "default.js", &CompileLimits::default())
        .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.category == Category::Semantic)
    );
}
#[test]
fn capacity_diagnostics_retain_original_utf16_surrogate_positions() {
    let mut source: Vec<_> = "//".encode_utf16().collect();
    source.push(0xd800);
    source.extend("\nlet x;".encode_utf16());
    let input = SourceInput::from_utf16(&source).unwrap();
    let errors = compile_program_input_with_limits(
        &input,
        "original.js",
        &CompileLimits {
            max_bindings: 0,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(errors[0].filename, "original.js");
    assert_eq!(errors[0].span, qjs_scalar_ssa_ir::Span { start: 8, end: 9 });
    assert_eq!(input.original(), source);
}
#[test]
fn observers_cover_only_parse_calls_and_do_not_charge_synthetic_strict_nodes() {
    let input = SourceInput::from_utf16(&"42;".encode_utf16().collect::<Vec<_>>()).unwrap();
    let limits = CompileLimits {
        max_ast_nodes: 3,
        ..Default::default()
    };
    let mut events = Vec::new();
    let program = compile_eval_program_input_with_limits_and_observer(
        &input,
        "strict.js",
        true,
        &limits,
        &mut |phase| events.push(phase),
    )
    .unwrap();
    assert!(program.strict);
    assert_eq!(program.source, input.original());
    assert_eq!(
        events,
        vec![
            ParsePhase::Begin,
            ParsePhase::End,
            ParsePhase::Begin,
            ParsePhase::End,
            ParsePhase::Begin,
            ParsePhase::End
        ]
    );
    events.clear();
    capacity(
        compile_program_input_with_limits_and_observer(
            &input,
            "capacity.js",
            &CompileLimits {
                max_ast_nodes: 2,
                ..Default::default()
            },
            &mut |phase| events.push(phase),
        ),
        "node",
    );
    assert_eq!(events, vec![ParsePhase::Begin, ParsePhase::End]);
}
#[test]
fn limits_cannot_be_loosened_and_invalid_overrides_never_enter_parser() {
    let input = SourceInput::from_utf16(&[]).unwrap();
    let defaults = CompileLimits::default();
    for limits in [
        CompileLimits {
            max_ast_nodes: defaults.max_ast_nodes + 1,
            ..defaults
        },
        CompileLimits {
            max_nesting: defaults.max_nesting + 1,
            ..defaults
        },
        CompileLimits {
            max_functions: defaults.max_functions + 1,
            ..defaults
        },
        CompileLimits {
            max_bindings: defaults.max_bindings + 1,
            ..defaults
        },
    ] {
        let mut events = Vec::new();
        capacity(
            compile_program_input_with_limits_and_observer(
                &input,
                "override.js",
                &limits,
                &mut |phase| events.push(phase),
            ),
            "override",
        );
        assert!(events.is_empty());
    }
    capacity(
        bounded(
            "",
            CompileLimits {
                max_ast_nodes: 0,
                ..Default::default()
            },
        ),
        "node",
    );
    bounded(
        "42;",
        CompileLimits {
            max_functions: 0,
            max_bindings: 0,
            ..Default::default()
        },
    )
    .unwrap();
}
#[test]
fn legacy_scalar_entry_has_the_same_presemantic_ast_budgets() {
    let errors = qjs_js_parser_frontend::compile_with_metadata_with_limits(
        "function main(){return 42;}",
        "legacy.js",
        &CompileLimits {
            max_functions: 0,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(errors[0].category, Category::Capacity);
    qjs_js_parser_frontend::compile_with_metadata("function main(){return 42;}", "legacy.js").unwrap();
}

#[test]
fn legacy_scalar_capacity_spans_remain_utf8() {
    let source = "/* 😀 */ function main(){return 42;}";
    let errors = qjs_js_parser_frontend::compile_with_limits(
        source,
        "legacy.js",
        &CompileLimits {
            max_functions: 0,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(errors[0].category, Category::Capacity);
    assert_eq!(
        errors[0].span.start as usize,
        source.find("function").unwrap()
    );
    assert_eq!(errors[0].span.end as usize, source.len());
}

#[test]
fn legacy_scalar_dominator_capacity_is_a_capacity_diagnostic() {
    let source = format!("function main(){{{}return 42;}}", "if(true){}".repeat(944));
    let errors = qjs_js_parser_frontend::compile_with_metadata(&source, "wide-cfg.js").unwrap_err();
    assert_eq!(errors[0].category, Category::Capacity, "{errors:?}");
    assert_eq!(errors[0].span.start, 0);
    assert_eq!(errors[0].span.end as usize, source.len());
}
