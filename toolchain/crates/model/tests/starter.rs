//! Extraction against the starter (spec 7, step 1): the starter is the project
//! every part of the pipeline is tested against.

use std::path::{Path, PathBuf};

use lunarstorm_model::{extract, Element, Value};

/// # Returns
/// The starter project's directory.
fn starter() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../platform/desktop/templates/app")
}

/// Finds an element by name anywhere under `root`.
///
/// # Arguments
/// * `root` - where to search.
/// * `name` - the element's `name`.
///
/// # Returns
/// The element; panics when there is none, which fails the test.
fn find<'a>(root: &'a Element, name: &str) -> &'a Element {
    let mut found = None;
    root.walk(&mut |element| {
        if element.name == name {
            found = Some(element);
        }
    });
    found.unwrap_or_else(|| panic!("no element named {name}"))
}

#[test]
fn the_starter_extracts_without_errors() {
    let extraction = extract(&starter());
    assert_eq!(extraction.diagnostics, vec![], "the starter must be a valid model");
    let project = extraction.project.expect("a valid project has a model");

    assert_eq!(project.application.tag, "application");
    assert_eq!(project.application.prop("id"), Some(&Value::Text("dev.lunarstorm.starter".into())));
    assert_eq!(project.application.prop("name"), Some(&Value::Text("Project Register".into())));
    assert_eq!(project.application.prop("mainForm"), Some(&Value::Text("MainForm".into())));

    assert_eq!(project.forms.len(), 1);
    let form = &project.forms[0];
    assert_eq!(form.name, "MainForm");
    assert_eq!(form.view, Path::new("src/forms/main-form.view.tsx"));
    assert_eq!(form.class, Path::new("src/forms/main-form.ts"));
}

#[test]
fn the_form_root_carries_the_window_properties() {
    let project = extract(&starter()).project.expect("valid");
    let root = &project.forms[0].root;
    assert_eq!(root.tag, "form");
    assert_eq!(root.name, "MainForm");
    assert_eq!(root.prop("caption"), Some(&Value::Text("Project Register".into())));
    assert_eq!(root.prop("width"), Some(&Value::Number(960.0)));
    assert_eq!(root.prop("height"), Some(&Value::Number(650.0)));
    assert_eq!(root.prop("position"), Some(&Value::Text("screenCenter".into())));
    assert_eq!(root.prop("menu"), Some(&Value::Text("MainMenu".into())));
    assert_eq!(root.prop("onCreate"), Some(&Value::Handler("formCreated".into())));
    assert_eq!(root.prop("onCloseQuery"), Some(&Value::Handler("formClosing".into())));
    assert_eq!(root.prop("name"), None, "the name is the element's name, not a prop");
    let top: Vec<&str> = root.children.iter().map(|child| child.name.as_str()).collect();
    assert_eq!(top, ["MainMenu", "CommandBar", "Workspace", "StatusBar"]);
}

#[test]
fn controls_keep_their_literals_and_connections() {
    let project = extract(&starter()).project.expect("valid");
    let root = &project.forms[0].root;

    let mut count = 0;
    root.walk(&mut |_| count += 1);
    assert_eq!(count, 35, "the form, 14 menu elements and 20 controls");

    let save = find(root, "SaveButton");
    assert_eq!(save.tag, "button");
    assert_eq!(save.prop("default"), Some(&Value::Bool(true)), "a bare prop is true");
    assert_eq!(save.prop("enabled"), Some(&Value::Bool(false)));
    assert_eq!(save.prop("onClick"), Some(&Value::Handler("saveProject".into())));

    let list = find(root, "ProjectList");
    assert_eq!(list.prop("items"), Some(&Value::List(vec![])));
    assert_eq!(list.prop("itemIndex"), Some(&Value::Number(-1.0)), "a signed number is a literal");

    let status = find(root, "StatusEdit");
    let items = ["Planned", "In progress", "Complete"].map(|item| Value::Text(item.into())).to_vec();
    assert_eq!(status.prop("items"), Some(&Value::List(items)));

    let pane = find(root, "ProjectPane");
    assert_eq!(pane.prop("width"), Some(&Value::Number(250.0)));
    let names: Vec<&str> = pane.children.iter().map(|child| child.name.as_str()).collect();
    assert_eq!(names, ["ProjectsLabel", "ProjectList", "CountLabel"]);

    let file = find(root, "FileMenu");
    assert_eq!(file.children.len(), 4, "submenu items nest under their menu item");
    assert_eq!(find(root, "QuitItem").prop("role"), Some(&Value::Text("quit".into())));
    assert_eq!(save.at.line, 42, "positions point into the view resource");
}

#[test]
fn native_references_are_collected_from_the_backend_code() {
    use std::fs;
    let directory = tempfile::TempDir::new().expect("temp dir");
    let write = |path: &str, text: &str| {
        let path = directory.path().join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    };
    write("src/app.tsx", "export default (<application id=\"a\" mainForm=\"Main\" />);\n");
    write("src/forms/main.view.tsx", "export default (<form name=\"Main\" />);\n");
    write(
        "src/forms/main.ts",
        "import { native as n } from '@desktop/api';\nexport class Main extends Form {\n  async go() { await n.os.info(); n.clipboard.writeText('x'); n.os.info(); }\n}\n",
    );
    write("src/models/files.ts", "import { native } from '@desktop/api';\nexport const pick = () => native.dialog.open({});\n");
    write("src/models/other.ts", "const native = { fake: { call() {} } };\nnative.fake.call();\n");
    write("src/models/files.test.ts", "import { native } from '@desktop/api';\nnative.shell.open('x');\n");
    let project = extract(directory.path()).project.expect("valid");
    assert_eq!(project.natives, ["clipboard.writeText", "dialog.open", "os.info"]);
    assert_eq!(extract(&starter()).project.expect("valid").natives, Vec::<String>::new(), "the starter calls no native");
}
