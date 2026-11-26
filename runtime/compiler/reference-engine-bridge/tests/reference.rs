//! The reference engine bridge: evaluation of arithmetic, closures and UTF-8 files,
//! lone surrogates, exceptions, syntax errors and interrupts.

use qjs_reference_engine_bridge::{ReferenceError, ReferenceRunner, ReferenceValue};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

fn runner() -> ReferenceRunner {
    static RUNNER: OnceLock<PathBuf> = OnceLock::new();
    let executable = RUNNER.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let status = Command::new("sh")
            .arg(root.join("scripts/build-reference.sh"))
            .status()
            .expect("execute C reference build");
        assert!(status.success(), "C reference build failed");
        root.join("build/qjs-reference-engine-bridge")
    });
    ReferenceRunner::new(executable)
}

#[test]
fn evaluates_arithmetic_from_stdin() {
    assert_eq!(
        runner().evaluate("6 * 7").unwrap(),
        ReferenceValue::Number(42.0)
    );
}

#[test]
fn evaluates_captured_closure_after_outer_returns() {
    let source = "(() => { let x = 40; return () => ++x; })()()";
    assert_eq!(
        runner().evaluate(source).unwrap(),
        ReferenceValue::Number(41.0)
    );
}

#[test]
fn preserves_supplementary_lone_surrogate_and_nul_string_units() {
    let value = runner().evaluate(r#""A\u{1f600}\ud800\0""#).unwrap();
    assert_eq!(
        value,
        ReferenceValue::String(vec![0x41, 0xd83d, 0xde00, 0xd800, 0])
    );
    assert_eq!(
        runner().evaluate("''").unwrap(),
        ReferenceValue::String(vec![])
    );
}

#[test]
fn preserves_primitive_types_and_special_numbers() {
    for (source, expected) in [
        ("undefined", ReferenceValue::Undefined),
        ("null", ReferenceValue::Null),
        ("true", ReferenceValue::Boolean(true)),
        ("false", ReferenceValue::Boolean(false)),
        (
            "12345678901234567890n",
            ReferenceValue::BigInt("12345678901234567890".into()),
        ),
        ("Infinity", ReferenceValue::Number(f64::INFINITY)),
        ("-Infinity", ReferenceValue::Number(f64::NEG_INFINITY)),
    ] {
        assert_eq!(runner().evaluate(source).unwrap(), expected);
    }
    let ReferenceValue::Number(zero) = runner().evaluate("-0").unwrap() else {
        panic!("expected number");
    };
    assert_eq!(zero.to_bits(), (-0.0_f64).to_bits());
    let ReferenceValue::Number(positive_zero) = runner().evaluate("0").unwrap() else {
        panic!("expected number");
    };
    assert_eq!(positive_zero.to_bits(), 0.0_f64.to_bits());
    let ReferenceValue::Number(nan) = runner().evaluate("NaN").unwrap() else {
        panic!("expected number");
    };
    assert!(nan.is_nan());
}

#[test]
fn reports_javascript_exception_without_success_value() {
    let error = runner()
        .evaluate("throw new Error('reference boom')")
        .unwrap_err();
    match error {
        ReferenceError::Execution { status, stderr } => {
            assert!(!status.success());
            assert!(stderr.contains("reference boom"), "{stderr}");
            assert!(stderr.contains("JavaScript exception"), "{stderr}");
        }
        other => panic!("expected JavaScript failure, got {other:?}"),
    }
}

#[test]
fn rejects_syntax_errors_and_unsupported_result_values() {
    for (source, message) in [
        ("const = ;", "SyntaxError"),
        ("({answer: 42})", "unsupported result type"),
    ] {
        let error = runner().evaluate(source).unwrap_err();
        assert!(error.to_string().contains(message), "{error}");
    }
    assert_eq!(
        runner().evaluate("({answer: 42}).answer").unwrap(),
        ReferenceValue::Number(42.0)
    );
}

#[test]
fn evaluates_utf8_source_file() {
    static NEXT_FILE: AtomicUsize = AtomicUsize::new(0);
    let file = std::env::temp_dir().join(format!(
        "qjs-reference-{}-{}.js",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&file, "const café = 21; café * 2").unwrap();
    let value = runner().evaluate_file(&file);
    fs::remove_file(file).unwrap();
    assert_eq!(value.unwrap(), ReferenceValue::Number(42.0));
}

#[test]
fn reports_missing_executable_as_io_error() {
    let error = ReferenceRunner::new("/nonexistent/qjs-reference-executable")
        .evaluate("42")
        .unwrap_err();
    assert!(matches!(error, ReferenceError::Io(_)));
}

#[test]
fn interrupts_infinite_javascript_loop() {
    let error = runner().evaluate("while (true) {}").unwrap_err();
    assert!(
        error.to_string().contains("execution budget exceeded"),
        "{error}"
    );
}
