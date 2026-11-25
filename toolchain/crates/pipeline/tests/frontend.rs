//! Frontend generation (spec 4b): the generated form module, and the built
//! frontend loaded for real, under bun, to read the element tree it produces.

use std::fs;
use std::path::{Path, PathBuf};

use lunarstorm_pipeline::{build, frontend};
use tempfile::TempDir;

/// # Returns
/// The crate's dummy component theme (`mock/`).
fn theme() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("mock")
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
fn the_visual_tags_are_the_components_of_the_vcl() {
    let index = fs::read_to_string(theme().join("index.ts")).expect("index.ts");
    let table = index.split("export const COMPONENTS = {").nth(1).expect("COMPONENTS").split('}').next().expect("end");
    let components: Vec<&str> = table.lines().filter_map(|line| line.trim().split(':').next()).filter(|name| !name.is_empty()).collect();
    assert_eq!(components, frontend::VISUAL);
}

#[test]
fn a_form_module_renders_visual_elements_with_their_literal_props() {
    let app = "export default (<application id=\"a\" mainForm=\"Main\" />);\n";
    let view = "export default (\n  <form name=\"Main\" caption=\"Title\" onCreate={form.created}>\n    <mainmenu name=\"Menu\"><menuitem name=\"Quit\" caption=\"Quit\" /></mainmenu>\n    <timer name=\"Tick\" interval={500} />\n    <panel name=\"Top\" align=\"top\" layout=\"row\" gap={8}>\n      <button name=\"Go\" caption={'Say \"go\"'} default onClick={form.go} />\n      <combobox name=\"Pick\" items={['a', 'b']} itemIndex={1} enabled={false} />\n    </panel>\n    <listview name=\"Table\" align=\"client\"><column name=\"NameColumn\" caption=\"Name\" width={120} /></listview>\n  </form>\n);\n";
    let class = "import { Form } from '@desktop/forms';\nexport class Main extends Form { readonly created = (): void => {}; readonly go = (): void => {}; }\n";
    let directory = project(&[("src/app.tsx", app), ("src/forms/main.view.tsx", view), ("src/forms/main.ts", class)]);
    let out = TempDir::new().expect("temp dir");
    build(directory.path(), out.path()).unwrap_or_else(|failure| panic!("build failed: {failure:?}"));
    assert_eq!(
        fs::read_to_string(out.path().join("frontend/forms/Main.js")).expect("module"),
        "// Generated from the form's view resource. Do not edit.\n\
import { h, C } from '../vcl.js';\n\
\n\
export default () =>\n\
\x20 h(C.form, {\"name\":\"Main\",\"caption\":\"Title\"},\n\
\x20   h(C.panel, {\"name\":\"Top\",\"align\":\"top\",\"layout\":\"row\",\"gap\":8},\n\
\x20     h(C.button, {\"name\":\"Go\",\"caption\":\"Say \\\"go\\\"\",\"default\":true}),\n\
\x20     h(C.combobox, {\"name\":\"Pick\",\"items\":[\"a\",\"b\"],\"itemIndex\":1,\"enabled\":false})),\n\
\x20   h(C.listview, {\"name\":\"Table\",\"align\":\"client\"},\n\
\x20     h(C.column, {\"name\":\"NameColumn\",\"caption\":\"Name\",\"width\":120})));\n"
    );
}

#[test]
fn a_rebuild_drops_the_module_of_a_removed_form() {
    let app = "export default (<application id=\"a\" mainForm=\"Main\" />);\n";
    let class = |name: &str| format!("import {{ Form }} from '@desktop/forms';\nexport class {name} extends Form {{}}\n");
    let directory = project(&[
        ("src/app.tsx", app),
        ("src/forms/main.view.tsx", "export default (<form name=\"Main\" />);\n"),
        ("src/forms/main.ts", &class("Main")),
        ("src/forms/about.view.tsx", "export default (<form name=\"About\" />);\n"),
        ("src/forms/about.ts", &class("About")),
    ]);
    let out = TempDir::new().expect("temp dir");
    build(directory.path(), out.path()).unwrap_or_else(|failure| panic!("build failed: {failure:?}"));
    assert!(out.path().join("frontend/forms/About.js").is_file());
    fs::remove_file(directory.path().join("src/forms/about.view.tsx")).expect("remove");
    fs::remove_file(directory.path().join("src/forms/about.ts")).expect("remove");
    build(directory.path(), out.path()).unwrap_or_else(|failure| panic!("build failed: {failure:?}"));
    assert!(!out.path().join("frontend/forms/About.js").exists());
    assert!(out.path().join("frontend/forms/Main.js").is_file());
}
