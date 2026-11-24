use qjs_frontend::{Category, SourceMode, compile_program};

#[test]
fn in_and_instanceof_are_supported_dynamic_binary_expressions() {
    for source in [
        "key() in object();",
        "value() instanceof constructor();",
        "'\\ud800' in object;",
        "value instanceof constructor;",
    ] {
        compile_program(source, "operators.js", SourceMode::Script).unwrap();
    }
}
#[test]
fn missing_operands_and_invalid_assignment_targets_are_early_errors() {
    for source in [
        "key in;",
        "instanceof Constructor;",
        "(key in object) = true;",
        "(value instanceof Constructor)++;",
    ] {
        let errors = compile_program(source, "operators.js", SourceMode::Script).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e.category, Category::Parse | Category::Semantic)),
            "{errors:?}"
        );
    }
}

use qjs_ir::js::{Binary, ExpressionKind as E, Literal, StatementKind};
#[test]
fn owned_operands_keep_order_and_original_utf16() {
    for (source, expected) in [
        ("left() in right();", Binary::In),
        ("left() instanceof right();", Binary::Instanceof),
    ] {
        let program = compile_program(source, "operators.js", SourceMode::Script).unwrap();
        let StatementKind::Expression(expression) = &program.body[0].kind else {
            panic!()
        };
        let E::Binary { op, left, right } = &expression.kind else {
            panic!()
        };
        assert_eq!(*op, expected);
        for (operand, name) in [(left, "left"), (right, "right")] {
            let E::Call { callee, arguments } = &operand.kind else {
                panic!()
            };
            let E::Read(reference) = &callee.kind else {
                panic!()
            };
            assert_eq!(reference.name, name);
            assert!(arguments.is_empty());
        }
    }
    let program = compile_program(
        "'\\ud800\\u0000' in object;",
        "surrogate.js",
        SourceMode::Script,
    )
    .unwrap();
    let StatementKind::Expression(expression) = &program.body[0].kind else {
        panic!()
    };
    let E::Binary {
        op: Binary::In,
        left,
        ..
    } = &expression.kind
    else {
        panic!()
    };
    assert_eq!(left.kind, E::Literal(Literal::String(vec![0xd800, 0])));
}
#[test]
fn dynamic_type_errors_are_preserved_for_runtime() {
    // These have valid grammar. The operator, rather than the frontend, must
    // throw for a primitive/non-callable RHS after evaluating both operands.
    for source in ["'key' in 42;", "value instanceof 42;"] {
        compile_program(source, "dynamic.js", SourceMode::Script).unwrap();
    }
    compile_program(
        "for(let i=0;i<1;i++){let present='x' in object;}",
        "loop.js",
        SourceMode::Script,
    )
    .unwrap();
    // `for-in` is a statement, not the relational operator, and compiles.
    compile_program("for(key in object){}", "for-in.js", SourceMode::Script).unwrap();
}
