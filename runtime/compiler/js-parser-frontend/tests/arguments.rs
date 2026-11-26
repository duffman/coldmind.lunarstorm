use qjs_js_parser_frontend::{SourceMode, compile_program};
use qjs_scalar_ssa_ir::js::{ExpressionKind, StatementKind};
#[test]
fn ordinary_arguments_is_an_owned_binding_not_an_unresolved_global() {
    let program = compile_program(
        "let arguments=1;function f(x){return arguments[0];}",
        "arguments.js",
        SourceMode::Script,
    )
    .unwrap();
    let StatementKind::Function { function, .. } = &program.body[1].kind else {
        panic!()
    };
    let binding = function.arguments_binding.as_ref().unwrap();
    let StatementKind::Return(Some(expression)) = &function.body[0].kind else {
        panic!()
    };
    let ExpressionKind::Member { object, .. } = &expression.kind else {
        panic!()
    };
    let ExpressionKind::Read(reference) = &object.kind else {
        panic!()
    };
    assert_eq!(reference.symbol, Some(binding.symbol));
}
#[test]
fn explicit_arguments_shadowing_uses_the_existing_symbol() {
    for (source, implicit) in [
        ("function f(arguments){return arguments;}", false),
        ("function f(){let arguments=42;return arguments;}", false),
        ("function f(){var arguments;return arguments;}", true),
        (
            "function f(){function arguments(){return 42;}return arguments();}",
            true,
        ),
    ] {
        let p = compile_program(source, "shadow.js", SourceMode::Script).unwrap();
        let StatementKind::Function { function, .. } = &p.body[0].kind else {
            panic!()
        };
        assert_eq!(function.arguments_binding.is_some(), implicit, "{source}");
    }
}
