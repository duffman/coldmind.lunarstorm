use qjs_js_parser_frontend::{SourceMode, compile_program};
use qjs_scalar_ssa_ir::js::*;
#[test]
fn general_script_keeps_completion_expression_and_symbol_identity() {
    let p = compile_program("let x=1; x+41;", "script.js", SourceMode::Script).unwrap();
    assert_eq!(p.body.len(), 2);
    let StatementKind::Declaration(declarations) = &p.body[0].kind else {
        panic!()
    };
    let Pattern::Binding { binding, .. } = &declarations[0].pattern else {
        panic!("{:?}", declarations[0].pattern)
    };
    let symbol = binding.symbol;
    let StatementKind::Expression(e) = &p.body[1].kind else {
        panic!()
    };
    let ExpressionKind::Binary { left, .. } = &e.kind else {
        panic!()
    };
    let ExpressionKind::Read(r) = &left.kind else {
        panic!()
    };
    assert_eq!(r.symbol, Some(symbol));
}
#[test]
fn ordinary_function_parameters_and_capture_identity_are_owned() {
    let p = compile_program(
        "let x=2;function f(y){return x+y;} f(40);",
        "closure.js",
        SourceMode::Script,
    )
    .unwrap();
    let StatementKind::Function { function, .. } = &p.body[1].kind else {
        panic!()
    };
    assert_eq!(function.parameters.len(), 1);
    let Pattern::Binding { binding, .. } = &function.parameters[0] else {
        panic!("{:?}", function.parameters[0])
    };
    assert_eq!(binding.kind, DeclarationKind::Parameter);
    assert!(matches!(p.body[2].kind, StatementKind::Expression(_)));
}
#[test]
fn enclosing_strict_mode_is_inherited_by_nested_functions() {
    let p = compile_program(
        "\"use strict\";function f(){return 42;}",
        "strict.js",
        SourceMode::Script,
    )
    .unwrap();
    let StatementKind::Function { function, .. } = &p.body[1].kind else {
        panic!()
    };
    assert!(
        function.strict,
        "nested function must inherit enclosing strictness"
    );
}
#[test]
fn unsupported_dead_code_is_not_silently_discarded() {
    assert!(
        compile_program(
            "function f(){return 42;function hidden(){return import.source('./x.js')}}",
            "dead.js",
            SourceMode::Script
        )
        .is_err()
    );
}
#[test]
fn dynamic_strings_are_owned_utf16_not_scalar_coercions() {
    let p = compile_program("'\\ud800'+1;", "string.js", SourceMode::Script).unwrap();
    let StatementKind::Expression(e) = &p.body[0].kind else {
        panic!()
    };
    let ExpressionKind::Binary { left, .. } = &e.kind else {
        panic!()
    };
    assert_eq!(
        left.kind,
        ExpressionKind::Literal(Literal::String(vec![0xd800]))
    );
}

#[test]
fn script_goal_rejects_top_level_return() {
    assert!(compile_program("return 42", "script.js", SourceMode::Script).is_err());
}

#[test]
fn directive_retains_script_completion_value() {
    let program = compile_program("'text';", "text.js", SourceMode::Script).unwrap();
    assert!(
        matches!(&program.body[0].kind,StatementKind::Expression(Expression{kind:ExpressionKind::Literal(Literal::String(units)),..}) if units==&"text".encode_utf16().collect::<Vec<_>>())
    );
}
