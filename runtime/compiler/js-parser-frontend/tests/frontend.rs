//! The scalar frontend end to end: arithmetic, branch merges, shadowing, diagnostics,
//! IEEE edge cases and seeded programs checked against the reference engine.

use qjs_js_parser_frontend::{Category, compile, compile_with_metadata};
use qjs_scalar_ssa_ir::{Scalar, execute};
fn run(s: &str) -> Scalar {
    execute(&compile(s, "test.js").unwrap(), &[], 1000).unwrap()
}
#[test]
fn compiles_arithmetic_42() {
    assert_eq!(run("function main(){return 6*7;}"), Scalar::Number(42.0));
}
#[test]
fn branch_merge_has_parameters() {
    let s = "function main(){let x=1;if(true){x=42;}else{x=3;}return x;}";
    let f = compile(s, "x").unwrap();
    assert!(f.blocks.iter().any(|b| !b.params.is_empty()));
    assert_eq!(execute(&f, &[], 100).unwrap(), Scalar::Number(42.0));
}
#[test]
fn handles_returning_branches() {
    for s in [
        "function main(){if(true){return 42;}else{return 2;}}",
        "function main(){if(false){return 2;}return 42;}",
    ] {
        assert_eq!(run(s), Scalar::Number(42.0));
    }
}
#[test]
fn block_shadowing_preserves_outer_binding() {
    assert_eq!(
        run("function main(){let x=42;{let x=1;x=2;}return x;}"),
        Scalar::Number(42.0)
    );
}
#[test]
fn rejects_unsupported_before_emission() {
    for s in [
        "function main(){return 1;while(true){}}",
        "function main(){if(false){foo();}return 1;}",
        "function main(){const a=1;a=2;return a;}",
        "function main(){let a=a;return a;}",
        "function main(){return unknown;}",
        "function main(){let a=1;if(a){return 2;}return 3;}",
        "function main(){if(true){return 2;}}",
        "function main(){let a=1;return a+true;}",
        "function main(){let a=1;let a=2;return a;}",
    ] {
        assert!(compile(s, "x").is_err(), "{s}");
    }
}
#[test]
fn result_outlives_source_and_arena() {
    let f = {
        let s = String::from("function main(){return 42;}");
        compile(&s, "x").unwrap()
    };
    assert_eq!(execute(&f, &[], 100).unwrap(), Scalar::Number(42.0));
}
#[test]
fn diagnostics_use_original_unicode_positions() {
    let s = "function main(){/* 😀 å */ return missing;}";
    let e = compile(s, "unicode.js").unwrap_err();
    assert_eq!(e[0].category, Category::Semantic);
    assert_eq!(
        &s[e[0].span.start as usize..e[0].span.end as usize],
        "missing"
    );
}
#[test]
fn preserves_strict_metadata() {
    assert!(
        compile_with_metadata("function main(){'use strict';return 1;}", "x")
            .unwrap()
            .metadata
            .strict
    );
}
#[test]
fn ieee754_edge_cases_match_reference() {
    assert!(matches!(run("function main(){return 0/0;}"),Scalar::Number(n) if n.is_nan()));
    assert!(
        matches!(run("function main(){return -0;}"),Scalar::Number(n) if n.to_bits()==(-0.0f64).to_bits())
    );
    assert_eq!(run("function main(){return !false;}"), Scalar::Bool(true));
}
#[test]
fn unreachable_returns_do_not_constrain_reachable_type() {
    assert_eq!(
        run("function main(){return 42;return true;}"),
        Scalar::Number(42.0)
    );
}
#[test]
fn nested_mutations_and_returning_joins() {
    for (condition, expected) in [("true", 42.0), ("false", 7.0)] {
        let s = format!(
            "function main(){{let x=1;if({condition}){{if(false){{return 9;}}x=42;}}else{{x=7;}}return x;}}"
        );
        assert_eq!(run(&s), Scalar::Number(expected));
    }
}
#[test]
fn lexical_shadow_tdz_and_dead_unsupported() {
    for s in [
        "function main(){let x=1;{let x=x;}return x;}",
        "function main(){return 42;let x=missing;}",
        "function main(){return 42;const x=1;x=2;}",
        "function main(){return 42;class X{}}",
        "function main(){return 42;let x=1;return x.foo;}",
    ] {
        assert!(compile(s, "test.js").is_err(), "{s}");
    }
}
#[test]
fn all_supported_operators_match_reference() {
    use qjs_reference_engine_bridge::{ReferenceRunner, ReferenceValue};
    for expression in [
        "6+7",
        "6-7",
        "6*7",
        "6/7",
        "-6%4",
        "3<4",
        "3<=3",
        "3>4",
        "3>=3",
        "0===-0",
        "0!==-0",
        "true===false",
        "true!==false",
        "-0",
        "!false",
        "(0/0)===(0/0)",
        "1/0",
        "-1/0",
        "1e308*1e308",
    ] {
        let source = format!("function main(){{return {expression};}}");
        let actual = run(&source);
        let reference = ReferenceRunner::from_env()
            .evaluate(&format!("{source}\nmain()"))
            .unwrap();
        match (actual, reference) {
            (Scalar::Number(a), ReferenceValue::Number(b)) => assert!(
                a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()),
                "{expression}: {a:?} {b:?}"
            ),
            (Scalar::Bool(a), ReferenceValue::Boolean(b)) => assert_eq!(a, b, "{expression}"),
            other => panic!("type mismatch {other:?}"),
        }
    }
}
#[test]
fn seeded_programs_match_reference() {
    use qjs_reference_engine_bridge::{ReferenceRunner, ReferenceValue};
    let mut state = 0x51A7_E001u64;
    for index in 0..128 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let a = (state >> 32) % 97;
        let c = if state & 1 == 0 { "true" } else { "false" };
        let edge = match index % 5 {
            0 => "-0",
            1 => "0/0",
            2 => "1/0",
            3 => "1e308*1e308",
            _ => "-7%3",
        };
        let source = format!(
            "function main(){{let x={a};let y={edge};if({c}){{x=x-3;if(x<40){{y=y+1;}}else{{y=y-2;}}}}else{{x=x+7;}}return x+y;}}"
        );
        let mut ir = compile(&source, "seeded.js").unwrap();
        let actual = execute(&ir, &[], 1000).unwrap();
        qjs_scalar_ssa_ir::fold_constants(&mut ir).unwrap();
        let folded = execute(&ir, &[], 1000).unwrap();
        let ReferenceValue::Number(reference) = ReferenceRunner::from_env()
            .evaluate(&format!("{source}\nmain()"))
            .unwrap()
        else {
            panic!("reference type");
        };
        for result in [actual, folded] {
            let Scalar::Number(n) = result else {
                panic!("IR type");
            };
            assert!(
                n.to_bits() == reference.to_bits() || (n.is_nan() && reference.is_nan()),
                "seed 0x51A7_E001 index {index}: {source} {n:?} != {reference:?}"
            );
        }
    }
}
