//! Host configuration and bridge table generation (spec 4e, 4d), against the
//! starter and against a project that uses every native component.

use std::fs;
use std::path::Path;

use lunarstorm_pipeline::{build, host, Failure};
use serde_json::{json, Value};
use tempfile::TempDir;

/// Builds a project and reads its host configuration back from disk.
///
/// # Arguments
/// * `project` - the project directory.
///
/// # Returns
/// The output directory and the parsed `zebra.json`.
fn config(project: &Path) -> (TempDir, Value) {
    let out = TempDir::new().expect("temp dir");
    build(project, out.path()).unwrap_or_else(|failure| panic!("build failed: {failure:?}"));
    let text = fs::read_to_string(out.path().join("zebra.json")).expect("zebra.json");
    (out, serde_json::from_str(&text).expect("valid JSON"))
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

#[test]
fn native_components_documents_and_permissions_are_configured() {
    let app = "export default (\n  <application id=\"a.b\" mainForm=\"Main\" singleInstance urlSchemes={['app']} activationPolicy=\"accessory\" onReady={form.go}>\n    <popupmenu name=\"TrayMenu\"><menuitem name=\"Quit\" role=\"quit\" /></popupmenu>\n    <trayicon name=\"Tray\" icon=\"res/tray.png\" hint=\"App\" popupMenu=\"TrayMenu\" onClick={form.go} />\n    <actionlist name=\"AppActions\"><action name=\"About\" caption=\"About\" /></actionlist>\n    <document name=\"Project\" extensions={['proj']} role=\"editor\" />\n    <permissions filesystem={['$HOME/Projects']} executables={['git']} environment={['HOME']} />\n  </application>\n);\n";
    let view = "export default (\n  <form name=\"Main\" caption=\"Main\" onResize={form.go}>\n    <popupmenu name=\"Pop\"><menuitem name=\"Copy\" role=\"copy\" /></popupmenu>\n    <actionlist name=\"Acts\"><action name=\"Save\" caption=\"Save\" shortcut=\"CmdOrCtrl+S\" enabled={false} onExecute={form.go} /></actionlist>\n    <timer name=\"Clock\" interval={250} enabled={false} onTimer={form.go} />\n    <timer name=\"Tick\" />\n    <trayicon name=\"FormTray\" />\n    <edit name=\"Field\" popupMenu=\"Pop\" />\n    <button name=\"Go\" action=\"Save\" width={12.5} />\n  </form>\n);\n";
    let class = "import { native } from '@desktop/api';\nexport class Main extends Form {\n  readonly go = async (): Promise<void> => { await native.os.info(); };\n}\n";
    let directory = project(&[("src/app.tsx", app), ("src/forms/main.view.tsx", view), ("src/forms/main.ts", class)]);
    let (out, config) = config(directory.path());

    assert_eq!(
        config["application"],
        json!({
            "id": "a.b", "mainForm": "Main", "singleInstance": true, "urlSchemes": ["app"],
            "activationPolicy": "accessory", "name": "a.b", "events": ["onReady"],
        }),
        "the identity stands in for a missing name"
    );
    assert_eq!(
        config["menus"],
        json!([
            { "name": "TrayMenu", "owner": "@application", "kind": "popup", "items": [{ "name": "Quit", "role": "quit" }] },
            { "name": "Pop", "owner": "Main", "kind": "popup", "items": [{ "name": "Copy", "role": "copy" }] },
        ])
    );
    assert_eq!(
        config["trays"],
        json!([
            { "name": "Tray", "owner": "@application", "icon": "res/tray.png", "hint": "App", "popupMenu": "TrayMenu", "events": ["onClick"] },
            { "name": "FormTray", "owner": "Main", "events": [] },
        ])
    );
    assert_eq!(
        config["timers"],
        json!([
            { "name": "Clock", "owner": "Main", "interval": 250, "enabled": false },
            { "name": "Tick", "owner": "Main", "interval": 1000, "enabled": true },
        ])
    );
    assert_eq!(
        config["actions"],
        json!([
            { "name": "About", "owner": "@application", "caption": "About" },
            { "name": "Save", "owner": "Main", "caption": "Save", "shortcut": "CmdOrCtrl+S", "enabled": false },
        ])
    );
    assert_eq!(config["documents"], json!([{ "name": "Project", "extensions": ["proj"], "role": "editor" }]));
    assert_eq!(
        config["permissions"],
        json!({ "fs": ["$HOME/Projects"], "executables": ["git"], "env": ["HOME"], "native": ["os.info"] })
    );

    let controls = &config["forms"][0]["controls"];
    assert_eq!(controls["Field"]["popupMenu"], "Pop");
    assert_eq!(controls["Field"]["events"], json!(["onChange", "onContextPopup"]));
    assert_eq!(controls["FormTray"].get("popupMenu"), None);
    assert_eq!(controls["Clock"], json!({ "type": "timer", "props": ["interval", "enabled"], "events": ["onTimer"] }));
    assert_eq!(config["forms"][0]["events"], json!(["onResize"]));
    assert!(!out.path().join("package.json").exists(), "a project without package.json gets none");
}

#[test]
fn numbers_keep_their_fraction_and_integers_stay_integers() {
    use lunarstorm_model::Value as Model;
    assert_eq!(host::literal(&Model::Number(960.0)).to_string(), "960");
    assert_eq!(host::literal(&Model::Number(-1.0)).to_string(), "-1");
    assert_eq!(host::literal(&Model::Number(12.5)).to_string(), "12.5");
    assert_eq!(host::literal(&Model::List(vec![Model::Text("a".into()), Model::Bool(true)])), json!(["a", true]));
}

#[test]
fn a_project_with_errors_writes_nothing() {
    let directory = project(&[
        ("src/app.tsx", "export default (<application id=\"a\" mainForm=\"Main\" />);\n"),
        ("src/forms/main.view.tsx", "export default (<form name=\"Main\"><label name=\"L\" caption={f()} /></form>);\n"),
        ("src/forms/main.ts", "export class Main extends Form {}\n"),
    ]);
    let out = TempDir::new().expect("temp dir");
    let target = out.path().join("build");
    match build(directory.path(), &target) {
        Err(Failure::Model(diagnostics)) => assert_eq!(diagnostics.len(), 1),
        other => panic!("expected model errors, got {other:?}"),
    }
    assert!(!target.exists(), "nothing is written for a project with errors");
}
