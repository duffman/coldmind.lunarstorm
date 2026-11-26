//! Optional chain lowering: members, calls and `delete`, contiguous segments, and
//! parentheses ending a short circuit.

use qjs_js_parser_frontend::{SourceMode, compile_program};
#[test]
fn optional_members_calls_and_delete_lower_without_fallback() {
    for source in [
        "base?.value;",
        "base?.[key()];",
        "base?.method(argument());",
        "base.method?.(argument());",
        "base?.method?.(argument());",
        "(base?.method)();",
        "(base?.method)?.();",
        "delete base?.[key()];",
        "eval?.('42');",
    ] {
        compile_program(source, "chains.js", SourceMode::Script).unwrap();
    }
}

use qjs_scalar_ssa_ir::js::{ChainSegment as S, Expression, ExpressionKind as E, StatementKind, Unary};
fn expression(source: &str) -> Expression {
    let mut program = compile_program(source, "chains.js", SourceMode::Script).unwrap();
    let StatementKind::Expression(expression) = program.body.remove(0).kind else {
        panic!()
    };
    expression
}
#[test]
fn contiguous_chain_segments_keep_each_optional_boundary() {
    let E::Chain { base, segments } = expression("base.method?.(argument())?.[key()].last;").kind
    else {
        panic!()
    };
    assert!(matches!(base.kind, E::Read(_)));
    assert_eq!(segments.len(), 4);
    assert!(matches!(
        segments[0],
        S::Member {
            optional: false,
            ..
        }
    ));
    assert!(matches!(segments[1], S::Call { optional: true, .. }));
    assert!(matches!(segments[2], S::Member { optional: true, .. }));
    assert!(matches!(
        segments[3],
        S::Member {
            optional: false,
            ..
        }
    ));
    let S::Call { arguments, .. } = &segments[1] else {
        panic!()
    };
    assert_eq!(arguments.len(), 1);
    assert!(matches!(
        &arguments[0],
        qjs_scalar_ssa_ir::js::CallArgument::Value(argument) if matches!(argument.kind, E::Call { .. })
    ));
    let S::Member { key, .. } = &segments[2] else {
        panic!()
    };
    assert!(matches!(key.kind, E::Call { .. }));
}
#[test]
fn parentheses_end_short_circuiting_without_erasing_method_references() {
    let E::Call { callee, .. } = expression("(base?.method)();").kind else {
        panic!()
    };
    assert!(matches!(callee.kind, E::Chain { .. }));
    let E::Member { object, .. } = expression("(base?.method).property;").kind else {
        panic!()
    };
    assert!(matches!(object.kind, E::Chain { .. }));
    let E::Chain { base, segments } = expression("(base?.method)?.();").kind else {
        panic!()
    };
    assert!(matches!(base.kind, E::Chain { .. }));
    assert_eq!(segments.len(), 1);
    assert!(matches!(segments[0], S::Call { optional: true, .. }));
    let E::Chain { base, .. } = expression("(base.method)?.();").kind else {
        panic!()
    };
    assert!(matches!(base.kind, E::Member { .. }));
    let E::Chain { segments, .. } = expression("base?.method.property;").kind else {
        panic!()
    };
    assert_eq!(segments.len(), 2);
}
#[test]
fn delete_preserves_the_final_chain_member_reference() {
    let E::Unary {
        op: Unary::Delete,
        argument,
    } = expression("delete ((base?.[key()]));").kind
    else {
        panic!()
    };
    let E::Chain { segments, .. } = &argument.kind else {
        panic!()
    };
    assert!(matches!(
        segments.last(),
        Some(S::Member { optional: true, .. })
    ));
}
#[test]
fn chain_grammar_errors_and_direct_eval_remain_explicit() {
    for source in [
        "base?.value=1;",
        "base?.value++;",
        "new base?.Constructor();",
        "base?.tag`text`;",
    ] {
        let errors = compile_program(source, "invalid.js", SourceMode::Script).unwrap_err();
        assert!(
            errors.iter().any(|e| matches!(
                e.category,
                qjs_js_parser_frontend::Category::Parse | qjs_js_parser_frontend::Category::Semantic
            )),
            "{errors:?}"
        );
    }
    // Direct eval in an optional chain compiles since sloppy direct eval is supported.
    for source in ["eval('42')?.value;", "(eval)('42')?.value;"] {
        compile_program(source, "eval.js", SourceMode::Script)
            .unwrap_or_else(|e| panic!("{source}: {e:?}"));
    }
}
