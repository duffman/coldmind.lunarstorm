//! Emission of `new` expressions: ordinary construction and argument order, checked
//! against the vendored engine.

use qjs_bytecode_emitter::compile_script;
#[test]
fn ordinary_new_expressions_have_verified_final_call_operands() {
    for source in [
        "function Point(x){this.x=x;}new Point(42).x;",
        "function C(){return {answer:42};}(new C()).answer;",
        "function C(){this.x=42;return 1;}(new C()).x;",
    ] {
        compile_script(source, "new.js").unwrap_or_else(|e| panic!("{source}: {e}"));
    }
}

#[test]
fn ordinary_construction_and_arguments_execute_in_parser_disabled_vm() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vendor = root.join("../vendor/quickjs-ng");
    let directory =
        std::env::temp_dir().join(format!("qjs-source-constructors-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    let executable = directory.join("probe");
    let output = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()))
        .args(["-std=c11", "-D_GNU_SOURCE", "-DQJS_DISABLE_PARSER", "-I"])
        .arg(&vendor)
        .arg(root.join("tools/module_probe.c"))
        .args([
            vendor.join("dtoa.c"),
            vendor.join("libregexp.c"),
            vendor.join("libunicode.c"),
        ])
        .args(["-lm", "-pthread", "-o"])
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for (source, expected) in [
        ("function f(){return this===this;}f.call(1);", Some("true")),
        (
            "function f(){'use strict';return this;}f.call(42);",
            Some("42"),
        ),
        (
            "let object={answer:42,f(){return this.answer;}};object.f();",
            Some("42"),
        ),
        (
            "function f(){return function(){return this===this;};}f.call({}).call(1);",
            Some("true"),
        ),
        ("function f(x){return arguments[0]+2;}f(40);", Some("42")),
        ("function f(x){arguments[0]=42;return x;}f(40);", Some("42")),
        (
            "function f(x){'use strict';arguments[0]=42;return x+2;}f(40);",
            Some("42"),
        ),
        ("function f(x){x=42;return arguments[0];}f(40);", Some("42")),
        (
            "function f(x){'use strict';x=42;return arguments[0]+2;}f(40);",
            Some("42"),
        ),
        (
            "function f(){return arguments===arguments;}f();",
            Some("true"),
        ),
        (
            "function f(x){var arguments;return arguments[0];}f(42);",
            Some("42"),
        ),
        ("function f(arguments){return arguments;}f(42);", Some("42")),
        (
            "function f(){let arguments=42;return arguments;}f();",
            Some("42"),
        ),
        (
            "let arguments=0;function f(x){return arguments[0]+2;}f(40);",
            Some("42"),
        ),
        (
            "function f(){arguments=42;return arguments;}f();",
            Some("42"),
        ),
        (
            "function f(x,x){arguments[0]=99;return x+arguments[1];}f(1,21);",
            Some("42"),
        ),
        (
            "function f(x,x){x=42;return arguments[0]+arguments[1];}f(0,1);",
            Some("42"),
        ),
        (
            "function f(x,x){arguments[1]=42;return x;}f(1,2);",
            Some("42"),
        ),
        (
            "function f(){function arguments(){return 42;}return arguments();}f();",
            Some("42"),
        ),
        (
            "function f(x){{let arguments=0;}return arguments[0];}f(42);",
            Some("42"),
        ),
        (
            "function f(x){let outer=arguments;return function(y){return arguments!==outer&&arguments[0]===y;};}f(1)(42);",
            Some("true"),
        ),
        ("function Point(x){this.x=x;}new Point(42).x;", Some("42")),
        (
            "function C(){return {answer:42};}(new C()).answer;",
            Some("42"),
        ),
        ("function C(){this.x=42;return 1;}(new C()).x;", Some("42")),
        (
            "let x=0;function C(value){this.x=value;}let o=new C(x=42);o.x;",
            Some("42"),
        ),
        ("let o={method(){return 42;}};new o.method();", None),
    ] {
        let image = compile_script(source, "constructors.js").unwrap();
        assert!(image.bytes().len() < 4096);
        let mut child = Command::new(&executable)
            .arg("script")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(image.bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        if let Some(expected) = expected {
            assert!(
                output.status.success(),
                "{source}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), expected);
        } else {
            assert_eq!(output.status.code(), Some(3));
            assert!(String::from_utf8_lossy(&output.stderr).starts_with("TypeError:"));
        }
    }
}
