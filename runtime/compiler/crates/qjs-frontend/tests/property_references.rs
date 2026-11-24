use qjs_frontend::{SourceMode, compile_program};

#[test]
fn property_prefix_and_postfix_updates_are_supported() {
    for source in [
        "object.value++;",
        "--object.value;",
        "object[key()]--;",
        "++base()[key()];",
    ] {
        compile_program(source, "property.js", SourceMode::Script).unwrap();
    }
}
#[test]
fn compound_property_assignment_supports_every_binary_assignment_operator() {
    for operator in [
        "+=", "-=", "*=", "/=", "%=", "**=", "<<=", ">>=", ">>>=", "&=", "|=", "^=",
    ] {
        for target in ["base().value", "base()[key()]"] {
            let source = format!("{target} {operator} rhs();");
            compile_program(&source, "property.js", SourceMode::Script).unwrap();
        }
    }
}
#[test]
fn logical_assignment_supports_bindings_and_properties() {
    for operator in ["||=", "&&=", "??="] {
        for target in ["name", "base().value", "base()[key()]"] {
            let source = format!("{target} {operator} rhs();");
            compile_program(&source, "property.js", SourceMode::Script).unwrap();
        }
    }
}
#[test]
fn delete_supports_property_identifier_and_nonreference_forms() {
    for source in [
        "delete base().value;",
        "delete base()[key()];",
        "delete name;",
        "delete (0,name);",
        "delete effect();",
    ] {
        compile_program(source, "property.js", SourceMode::Script).unwrap();
    }
}

use qjs_ir::js::{Binary, Expression, ExpressionKind as E, Literal, Logical, StatementKind, Unary};
fn expression(source: &str) -> Expression {
    let mut program = compile_program(source, "property.js", SourceMode::Script).unwrap();
    let StatementKind::Expression(expression) = program.body.remove(0).kind else {
        panic!()
    };
    expression
}
fn call_name(expression: &Expression) -> &str {
    let E::Call { callee, arguments } = &expression.kind else {
        panic!("{expression:?}")
    };
    assert!(arguments.is_empty());
    let E::Read(reference) = &callee.kind else {
        panic!()
    };
    &reference.name
}
#[test]
fn member_assignment_keeps_each_effectful_operand_once_in_owned_ir() {
    for source in [
        "base()[key()] += rhs();",
        "base()[key()] ||= rhs();",
        "base()[key()] = rhs();",
    ] {
        let expression = expression(source);
        let (object, key, value) = match expression.kind {
            E::CompoundAssignMember {
                object,
                key,
                value,
                op: Binary::Add,
            }
            | E::LogicalAssignMember {
                object,
                key,
                value,
                op: Logical::Or,
            }
            | E::AssignMember { object, key, value } => (object, key, value),
            other => panic!("{other:?}"),
        };
        assert_eq!(call_name(&object), "base");
        assert_eq!(call_name(&key), "key");
        assert_eq!(call_name(&value), "rhs");
    }
}
#[test]
fn updates_keep_the_reference_and_result_mode() {
    for (source, increment, prefix) in [
        ("base()[key()]++;", true, false),
        ("--base()[key()];", false, true),
        ("(base()[key()])--;", false, false),
    ] {
        let E::UpdateMember {
            object,
            key,
            increment: actual_increment,
            prefix: actual_prefix,
        } = expression(source).kind
        else {
            panic!()
        };
        assert_eq!(call_name(&object), "base");
        assert_eq!(call_name(&key), "key");
        assert_eq!((actual_increment, actual_prefix), (increment, prefix));
    }
}
#[test]
fn logical_binding_assignment_keeps_symbol_identity_without_eager_read_desugaring() {
    let program = compile_program(
        "let value=0;value ??= rhs();",
        "logical.js",
        SourceMode::Script,
    )
    .unwrap();
    let StatementKind::Declaration(declarations) = &program.body[0].kind else {
        panic!()
    };
    let StatementKind::Expression(expression) = &program.body[1].kind else {
        panic!()
    };
    let E::LogicalAssign {
        target,
        op: Logical::Nullish,
        value,
    } = &expression.kind
    else {
        panic!()
    };
    let qjs_ir::js::Pattern::Binding { binding, .. } = &declarations[0].pattern else {
        panic!("{:?}", declarations[0].pattern)
    };
    assert_eq!(target.symbol, Some(binding.symbol));
    assert_eq!(call_name(value), "rhs");
}
#[test]
fn delete_retains_reference_vs_value_distinction_through_parentheses() {
    for source in ["delete object.value;", "delete ((object.value));"] {
        let E::Unary {
            op: Unary::Delete,
            argument,
        } = expression(source).kind
        else {
            panic!()
        };
        assert!(matches!(argument.kind, E::Member { .. }));
    }
    let E::Unary {
        op: Unary::Delete,
        argument,
    } = expression("delete (0,object.value);").kind
    else {
        panic!()
    };
    assert!(matches!(argument.kind, E::Sequence(_)));
    let E::Unary {
        op: Unary::Delete,
        argument,
    } = expression("delete name;").kind
    else {
        panic!()
    };
    assert!(matches!(argument.kind, E::Read(_)));
}
#[test]
fn strict_delete_identifier_and_invalid_assignment_targets_are_early_errors() {
    for source in [
        "'use strict';delete name;",
        "'use strict';delete ((name));",
        "function f(){'use strict';delete name;}",
        "1++;",
        "call() ||= 1;",
    ] {
        let errors = compile_program(source, "invalid.js", SourceMode::Script).unwrap_err();
        assert!(
            errors.iter().any(|e| matches!(
                e.category,
                qjs_frontend::Category::Parse | qjs_frontend::Category::Semantic
            )),
            "{source}: {errors:?}"
        );
    }
    assert!(compile_program("delete name;", "invalid.mjs", SourceMode::Module).is_err());
    compile_program(
        "'use strict';delete (0,name);delete object.value;",
        "strict.js",
        SourceMode::Script,
    )
    .unwrap();
}
#[test]
fn property_keys_keep_unpaired_surrogate_units() {
    let E::CompoundAssignMember { key, .. } = expression("object['\\ud800'] += rhs();").kind else {
        panic!()
    };
    assert_eq!(key.kind, E::Literal(Literal::String(vec![0xd800])));
}
#[test]
fn unsupported_optional_and_private_references_remain_explicit_errors() {
    for source in [
        "object?.value++;",
        "object?.value ||= rhs();",
        "object.#value += 1;",
    ] {
        assert!(
            compile_program(source, "unsupported.js", SourceMode::Script).is_err(),
            "{source}"
        );
    }
}
