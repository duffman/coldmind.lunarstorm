//! M0: one entry compiled in production mode runs in Bun.

use std::fs;
use std::path::Path;
use std::process::Command;

use devermind_core::{build_file, compile_file, Mode};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/app")
        .join(name)
}

#[test]
fn production_output_has_no_types_or_jsx() {
    let out = compile_file(&fixture("main.tsx"), &Mode::Production.options()).unwrap();
    assert!(!out.code.contains("interface Props"), "{}", out.code);
    assert!(!out.code.contains("<Hello"), "{}", out.code);
    assert!(out.code.contains("react/jsx-runtime"), "{}", out.code);
    assert!(
        out.imports.iter().any(|spec| spec == "./greet.ts"),
        "{:?}",
        out.imports
    );
    assert!(out.hot_accept.is_none());
}

#[test]
fn built_entry_runs_in_bun() {
    let dir = tempfile::tempdir().unwrap();
    build_file(&fixture("main.tsx"), &dir.path().join("main.js")).unwrap();
    build_file(&fixture("greet.ts"), &dir.path().join("greet.ts")).unwrap();
    assert!(dir.path().join("main.js.map").exists());

    // Minimal automatic-runtime shim so the output runs without installing React.
    let runtime = dir.path().join("node_modules/react");
    fs::create_dir_all(&runtime).unwrap();
    fs::write(
        runtime.join("package.json"),
        r#"{"name":"react","type":"module","exports":{"./jsx-runtime":"./jsx-runtime.js"}}"#,
    )
    .unwrap();
    fs::write(runtime.join("jsx-runtime.js"), "export const jsx = (type, props) => ({ type, props }); export const jsxs = jsx; export const Fragment = Symbol();").unwrap();

    let run = Command::new("bun")
        .arg("main.js")
        .current_dir(dir.path())
        .output()
        .expect("bun on PATH");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "{}{}",
        stdout,
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        stdout.trim(),
        r#"{"type":"function","props":{"name":"devermind"}}"#
    );
}

#[test]
fn parse_errors_name_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.ts");
    fs::write(&bad, "const = ;").unwrap();
    let error = compile_file(&bad, &Mode::Production.options())
        .unwrap_err()
        .to_string();
    assert!(error.contains("bad.ts"), "{error}");
}
