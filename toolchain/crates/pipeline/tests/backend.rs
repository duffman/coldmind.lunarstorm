//! Backend generation (spec 4c): the generated entry, and the bundled artifact
//! run for real, under bun, against a fake of the host's bridge.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use lunarstorm_model::extract;
use lunarstorm_pipeline::{backend, build, Failure};
use tempfile::TempDir;

/// # Returns
/// The starter project's directory.
fn starter() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../framework/templates/starters/desktop").canonicalize().expect("starter")
}

/// Writes a project to a temporary directory.
///
/// # Arguments
/// * `files` - project-relative paths with their text.
///
/// # Returns
/// The project directory.
fn project(files: &[(&str, &str)]) -> TempDir {
    let directory = TempDir::new().expect("temp dir");
    for (path, text) in files {
        let path = directory.path().join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }
    directory
}

/// Runs a built `backend.js` under bun with a fake `@desktop/bridge` and
/// `@desktop/api`, delivering host messages and printing what the backend did.
///
/// # Arguments
/// * `out` - the build output directory.
/// * `script` - JavaScript that uses `send(form, method, payload)` (returns the
///   dispatch result) and `writes` (the property writes so far), and prints.
///
/// # Returns
/// The harness's standard output.
fn run(out: &Path, script: &str) -> String {
    let fake = |name: &str, source: &str| {
        let directory = out.join("node_modules").join(name);
        fs::create_dir_all(&directory).expect("mkdir");
        fs::write(directory.join("package.json"), "{\"type\":\"module\",\"main\":\"index.js\"}").expect("write");
        fs::write(directory.join("index.js"), source).expect("write");
    };
    fake(
        "@desktop/bridge",
        "globalThis.writes = [];\nexport const setProperty = (...write) => void globalThis.writes.push(write.join('|'));\nexport const restored = () => ({});\nexport const serve = fn => void (globalThis.handler = fn);\n",
    );
    fake("@desktop/api", "export const native = { os: { info: async () => ({ platform: 'test' }) } };\n");
    let harness = format!(
        "await import('./backend.js');\nconst writes = globalThis.writes;\nconst send = (form, method, payload) => {{\n  const dot = method.indexOf('.');\n  return globalThis.handler({{ form, target: method.slice(0, dot), event: method.slice(dot + 1), payload, request: method.endsWith('onCloseQuery') }});\n}};\n{script}\n"
    );
    fs::write(out.join("harness.mjs"), harness).expect("write");
    let output = Command::new("bun").arg("harness.mjs").current_dir(out).output().expect("bun must be on PATH to run the backend");
    assert!(output.status.success(), "the backend failed:\n{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("utf-8")
}

#[test]
fn the_entry_binds_each_form_class_to_its_table() {
    let root = starter();
    let model = extract(&root).project.expect("valid");
    let entry = backend::entry(&model, &root);
    assert!(entry.contains("import { start } from \"@desktop/forms\";"));
    assert!(entry.contains(&format!("import * as module0 from \"{}\";", root.join("src/forms/main-form.ts").display())));
    assert!(entry.contains("create: module0[\"MainForm\"] ?? module0.default,"));
    assert!(entry.contains("application: { mainForm: \"MainForm\", connections: {} }"));
    assert!(entry.contains("\"MainForm.onCreate\":[\"formCreated\"]"));
    assert!(entry.contains("\"SaveItem.onClick\":[\"saveProject\"]"));
    assert!(entry.contains("\"NameEdit.onChange\":[\"projectChanged\"]"));
    // Every prop of the kind, literal over default.
    assert!(entry.contains("\"SaveButton\":[\"button\",{\"default\":true,\"cancel\":false,\"action\":\"\",\"caption\":\"Save\",\"align\":\"none\",\"width\":0,\"height\":0,\"enabled\":false,\"visible\":true,\"hint\":\"\",\"tabOrder\":0,\"popupMenu\":\"\"}]"));
    assert!(entry.contains("\"ProjectList\":[\"listbox\",{\"items\":[],\"itemIndex\":-1,\"caption\":\"\",\"align\":\"client\""));
    assert!(entry.contains("\"CommandBar\":[\"panel\",{\"layout\":\"row\",\"gap\":8,\"padding\":12,"));
}

#[test]
fn the_starter_backend_runs_its_handlers() {
    let out = TempDir::new().expect("temp dir");
    build(&starter(), out.path()).unwrap_or_else(|failure| panic!("build failed: {failure:?}"));
    let backend = fs::read_to_string(out.path().join("backend.js")).expect("backend.js");
    let imports: Vec<&str> = backend.lines().filter(|line| line.starts_with("import ")).collect();
    assert_eq!(imports, ["import { restored, serve, setProperty } from \"@desktop/bridge\";"], "only host modules stay imported");

    let script = "\
send('MainForm', 'MainForm.onCreate', {});\n\
console.log('create:' + writes.splice(0).join(';'));\n\
send('MainForm', 'NameEdit.onChange', { props: { text: 'Renamed' } });\n\
console.log('change:' + writes.splice(0).join(';'));\n\
console.log('close:' + JSON.stringify(send('MainForm', 'MainForm.onCloseQuery', {})));\n\
writes.splice(0);\n\
send('MainForm', 'SaveButton.onClick', { props: {} });\n\
console.log('save:' + writes.splice(0).join(';'));\n\
console.log('close:' + JSON.stringify(send('MainForm', 'MainForm.onCloseQuery', {})));\n\
send('MainForm', 'NewItem.onClick', {});\n\
console.log('new:' + writes.filter(write => write.includes('StatusBar')).pop());\n";
    let output = run(out.path(), script);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(
        lines[0],
        "create:MainForm|ProjectList|items|P-001   Desktop component library,P-002   Customer directory,P-003   Release checklist;\
MainForm|CountLabel|caption|3 projects · session storage;MainForm|ProjectList|itemIndex|0;\
MainForm|NameEdit|text|Desktop component library;MainForm|OwnerEdit|text|Alex Morgan;MainForm|StatusEdit|itemIndex|1;\
MainForm|NotesMemo|text|Define the form lifecycle, control ownership, and typed event contracts.;\
MainForm|RecordLabel|caption|P-001 · Project details"
    );
    assert_eq!(
        lines[1],
        "change:MainForm|SaveButton|enabled|true;MainForm|RevertButton|enabled|true;MainForm|SaveItem|enabled|true;\
MainForm|StatusBar|caption|Modified — save or revert before switching projects."
    );
    assert_eq!(lines[2], "close:{\"cancel\":true}", "unsaved changes keep the form open");
    assert!(lines[3].starts_with("save:MainForm|ProjectList|items|P-001   Renamed,"), "{}", lines[3]);
    assert!(lines[3].ends_with("MainForm|StatusBar|caption|P-001 saved for this session."), "{}", lines[3]);
    assert_eq!(lines[4], "close:{\"cancel\":false}");
    assert_eq!(lines[5], "new:MainForm|StatusBar|caption|P-004 created. Enter the project details.");
}

#[test]
fn actions_and_application_events_are_connected() {
    let app = "export default (<application id=\"a\" mainForm=\"Main\" onReady={form.ready} />);\n";
    let view = "export default (\n  <form name=\"Main\">\n    <actionlist name=\"Acts\"><action name=\"Save\" caption=\"Save\" onExecute={form.execute} /></actionlist>\n    <button name=\"Go\" action=\"Save\" onClick={form.clicked} />\n    <button name=\"Other\" action=\"Save\" />\n  </form>\n);\n";
    let class = "import { Form } from '@desktop/forms';\nimport { native } from '@desktop/api';\nimport { label } from '../models/label';\nexport default class Main extends Form {\n  private readonly save = this.find<any>('Save');\n  readonly clicked = (): void => { console.log(label('clicked')); };\n  readonly execute = (): void => { console.log(label('executed')); this.save.enabled = false; };\n  readonly ready = async (): Promise<void> => { console.log(label((await native.os.info()).platform)); };\n}\n";
    let model = "export const label = (text: string): string => `[${text}]`;\n";
    let directory = project(&[("src/app.tsx", app), ("src/forms/main.view.tsx", view), ("src/forms/main.ts", class), ("src/models/label.ts", model)]);
    let out = TempDir::new().expect("temp dir");
    build(directory.path(), out.path()).unwrap_or_else(|failure| panic!("build failed: {failure:?}"));

    let script = "\
send('Main', 'Go.onClick', { props: {} });\n\
console.log(writes.splice(0).join(';'));\n\
send('Main', 'Other.onClick', { props: {} });\n\
send('Main', 'Save.onExecute', {});\n\
await send('@application', '@application.onReady', {});\n";
    assert_eq!(
        run(out.path(), script),
        "[clicked]\n[executed]\nMain|Save|enabled|false;Main|Go|enabled|false;Main|Other|enabled|false\n[executed]\n[executed]\n[test]\n"
    );
}

#[test]
fn code_that_does_not_bundle_fails_the_build_and_writes_nothing() {
    let directory = project(&[
        ("src/app.tsx", "export default (<application id=\"a\" mainForm=\"Main\" />);\n"),
        ("src/forms/main.view.tsx", "export default (<form name=\"Main\" />);\n"),
        ("src/forms/main.ts", "import { Form } from '@desktop/forms';\nimport { missing } from '../models/missing';\nexport class Main extends Form { x = missing; }\n"),
    ]);
    let out = TempDir::new().expect("temp dir");
    let target = out.path().join("build");
    match build(directory.path(), &target) {
        Err(Failure::Backend(diagnostics)) => assert!(diagnostics.contains("missing"), "{diagnostics}"),
        other => panic!("expected a backend failure, got {other:?}"),
    }
    assert!(!target.exists());
}

#[test]
fn the_cli_builds_a_project_given_by_a_relative_path() {
    let out = TempDir::new().expect("temp dir");
    let framework = starter().join("../..").canonicalize().expect("templates");
    let output = Command::new(env!("CARGO_BIN_EXE_lunarstorm-build"))
        .current_dir(&framework)
        .arg("starters/desktop")
        .arg(out.path())
        .output()
        .expect("lunarstorm-build runs");
    assert!(output.status.success(), "lunarstorm-build failed:\n{}", String::from_utf8_lossy(&output.stderr));
    assert!(out.path().join("backend.js").is_file());
}
