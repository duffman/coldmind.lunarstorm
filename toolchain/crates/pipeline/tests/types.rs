//! The generated declarations (spec 4, "Typecheck"): the starter carries the
//! current file and type-checks against it with the real `tsc`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use lunarstorm_pipeline::types;

/// # Returns
/// The repository's `framework` directory.
fn framework() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../framework").canonicalize().expect("framework")
}

/// Runs `tsc` on the starter with one file replaced.
///
/// # Arguments
/// * `replace` - `(path under the starter, old text, new text)`, or `None` to
///   check the starter as it is.
///
/// # Returns
/// Whether `tsc` passed, and its output.
fn tsc(replace: Option<(&str, &str, &str)>) -> (bool, String) {
    let starter = framework().join("templates/starters/desktop");
    let project = tempfile::TempDir::new().expect("temp dir");
    for file in ["tsconfig.json", "src/desktop.d.ts", "src/app.tsx", "src/forms/main-form.view.tsx", "src/forms/main-form.ts", "src/models/project-store.ts"] {
        let target = project.path().join(file);
        fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        let mut text = fs::read_to_string(starter.join(file)).unwrap_or_else(|error| panic!("{file}: {error}"));
        if let Some((path, old, new)) = replace.filter(|(path, ..)| *path == file) {
            assert!(text.contains(old), "{path} does not contain {old:?}");
            text = text.replacen(old, new, 1);
        }
        fs::write(target, text).expect("write");
    }
    let compiler = framework().join("zebra.framework/node_modules/.bin/tsc");
    let output = Command::new(compiler).args(["--noEmit", "-p", "."]).current_dir(project.path()).output().expect("tsc runs");
    (output.status.success(), String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr))
}

#[test]
fn the_starter_carries_the_current_declarations() {
    let carried = fs::read_to_string(framework().join("templates/starters/desktop/src/desktop.d.ts")).expect("desktop.d.ts");
    assert!(carried == types::declarations(), "run `lunarstorm-build --types framework/templates/starters/desktop`");
}

#[test]
fn the_starter_type_checks() {
    let (passed, output) = tsc(None);
    assert!(passed, "{output}");
}

#[test]
fn tsc_rejects_what_the_schema_rejects() {
    // A prop value outside the schema's choices.
    let (passed, output) = tsc(Some(("src/forms/main-form.view.tsx", "position=\"screenCenter\"", "position=\"middle\"")));
    assert!(!passed && output.contains("main-form.view.tsx"), "{output}");
    // A handler that is not a member of the form class.
    let (passed, output) = tsc(Some(("src/forms/main-form.view.tsx", "form.formCreated", "form.missing")));
    assert!(!passed && output.contains("missing"), "{output}");
    // A control property of the wrong type, written from the form class.
    let (passed, output) = tsc(Some(("src/forms/main-form.ts", "query.cancel = !this.allowNavigation();", "query.cancel = 'no';")));
    assert!(!passed && output.contains("main-form.ts"), "{output}");
}
