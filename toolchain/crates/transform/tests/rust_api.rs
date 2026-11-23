//! Public API contract shared by direct Rust, Bun and Tauri adapters.

use devermind_native::{DefinePair, Loader, ReplacePair, TransformOptions, Transformer};

#[test]
fn transforms_all_stages_through_the_public_api() {
    let engine = Transformer::new(TransformOptions {
        replace: Some(vec![ReplacePair {
            from: "__NAME__".into(),
            to: "__MODE__".into(),
        }]),
        define: Some(vec![DefinePair {
            key: "__MODE__".into(),
            value: "\"dev\"".into(),
        }]),
        inject_all: Some(true),
    })
    .unwrap();
    let source = "class Dep {}\nclass App { constructor(dep: Dep) {} }\nexport const mode: string = __NAME__;";
    let result = engine.transform_source("/app.ts", source, Loader::Ts);
    assert!(result.changed && result.parsed);
    assert_eq!(
        (result.replaced, result.defined, result.injected),
        (1, 1, 1)
    );
    assert!(result.code.contains("mode: string = \"dev\";"));
    assert!(result
        .code
        .contains("static get $inject() { return [Dep]; }"));
    assert!(result.warnings.is_empty());
    assert_eq!(engine.stats().files, 1);
    assert_eq!(engine.stats().changed, 1);
}

#[test]
fn no_op_preserves_input_exactly() {
    let engine = Transformer::new(TransformOptions::default()).unwrap();
    let source = "// λ\nexport const x: number = 1;\n";
    let result = engine.transform_source("not-a-file.ts", source, Loader::Ts);
    assert_eq!(result.code, source);
    assert!(!result.changed && !result.parsed);
    assert_eq!(engine.stats().files, 1);
}

#[test]
fn validates_configuration_without_a_host_runtime() {
    assert!(Transformer::new(TransformOptions {
        replace: Some(vec![ReplacePair {
            from: String::new(),
            to: "x".into()
        }]),
        ..Default::default()
    })
    .err()
    .unwrap()
    .contains("must not be empty"));
    for (key, value, message) in [
        ("not-valid", "1", "dotted identifier"),
        ("VALID", "1 +", "JS expression"),
    ] {
        assert!(Transformer::new(TransformOptions {
            define: Some(vec![DefinePair {
                key: key.into(),
                value: value.into()
            }]),
            ..Default::default()
        })
        .err()
        .unwrap()
        .contains(message));
    }
}

#[test]
fn returns_per_input_warnings_and_a_drainable_queue() {
    let engine = Transformer::new(TransformOptions {
        inject_all: Some(false),
        ..Default::default()
    })
    .unwrap();
    let result = engine.transform_source(
        "app.ts",
        "@d class A { constructor(n: number) {} }",
        Loader::Ts,
    );
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].starts_with("app.ts: devermind inject: A not wired"));
    assert_eq!(engine.take_warnings(), result.warnings);
    assert!(engine.take_warnings().is_empty());
}

#[test]
fn preserves_replace_edits_when_source_is_invalid() {
    let engine = Transformer::new(TransformOptions {
        replace: Some(vec![ReplacePair {
            from: "A".into(),
            to: "B".into(),
        }]),
        define: Some(vec![DefinePair {
            key: "B".into(),
            value: "1".into(),
        }]),
        ..Default::default()
    })
    .unwrap();
    let result = engine.transform_source("bad.ts", "const = A", Loader::Ts);
    assert_eq!(result.code, "const = B");
    assert!(result.changed && !result.parsed);
    assert_eq!(result.defined, 0);
}

#[test]
fn preserves_jsx_and_typescript_instead_of_transpiling() {
    let engine = Transformer::new(TransformOptions {
        define: Some(vec![DefinePair {
            key: "MODE".into(),
            value: "\"dev\"".into(),
        }]),
        ..Default::default()
    })
    .unwrap();
    for (loader, source) in [
        (Loader::Js, "export const value = MODE;"),
        (Loader::Jsx, "export const view = <div>{MODE}</div>;"),
        (Loader::Ts, "export const value: string = MODE;"),
        (
            Loader::Tsx,
            "export const view: unknown = <div>{MODE}</div>;",
        ),
    ] {
        let result = engine.transform_source("source", source, loader);
        assert!(result.parsed);
        assert_eq!(result.code, source.replace("MODE", "\"dev\""));
    }
    let result = engine.transform_source("text", "MODE", Loader::Other);
    assert_eq!(result.code, "MODE");
    assert!(!result.parsed);
}

#[test]
fn shares_one_engine_safely_between_threads() {
    let engine = std::sync::Arc::new(
        Transformer::new(TransformOptions {
            define: Some(vec![DefinePair {
                key: "MODE".into(),
                value: "1".into(),
            }]),
            ..Default::default()
        })
        .unwrap(),
    );
    std::thread::scope(|scope| {
        for _ in 0..16 {
            let engine = engine.clone();
            scope.spawn(move || {
                let result =
                    engine.transform_source("app.js", "export const mode = MODE;", Loader::Js);
                assert_eq!(result.defined, 1);
            });
        }
    });
    assert_eq!(engine.stats().files, 16);
    assert_eq!(engine.stats().defined, 16);
}
