//! Frontend generation (spec 4b): the generated form module, and the built
//! frontend loaded for real, under bun, to read the element tree it produces.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use lunarstorm_pipeline::{build, frontend};
use tempfile::TempDir;

/// # Returns
/// The crate's dummy component theme (`mock/`).
fn theme() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("mock")
}

/// # Returns
/// The desktop starter, under the repository's `templates` directory.
fn starter() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../templates/starters/desktop").canonicalize().expect("starter")
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
fn the_built_starter_frontend_loads_and_yields_its_element_tree() {
    let starter = starter();
    let out = TempDir::new().expect("temp dir");
    build(&starter, out.path()).unwrap_or_else(|failure| panic!("build failed: {failure:?}"));
    let frontend = out.path().join("frontend");
    for file in ["index.html", "main.js", "vcl.js", "vcl.css", "forms/MainForm.js"] {
        assert!(frontend.join(file).is_file(), "{file} is missing");
    }
    assert!(fs::read_to_string(frontend.join("index.html")).expect("page").contains("<script type=\"module\" src=\"main.js\"></script>"));

    // Load the generated module against the real prebuilt runtime and list the
    // tree: component name and control name, depth-first.
    let script = "\
const { C } = await import('./vcl.js');\n\
const names = new Map(Object.entries(C).map(([tag, component]) => [component, tag]));\n\
const tree = (await import('./forms/MainForm.js')).default();\n\
const walk = (element, depth) => {\n\
  console.log(`${' '.repeat(depth)}${names.get(element.type)} ${element.props.name ?? ''}`.trimEnd());\n\
  for (const child of [element.props.children ?? []].flat()) walk(child, depth + 1);\n\
};\n\
walk(tree, 0);\n\
console.log(JSON.stringify(tree.props.children[0].props.children[0].props));\n";
    fs::write(frontend.join("harness.mjs"), script).expect("write");
    let output = Command::new("bun").arg("harness.mjs").current_dir(&frontend).output().expect("bun must be on PATH");
    assert!(output.status.success(), "the frontend failed to load:\n{}", String::from_utf8_lossy(&output.stderr));
    let text = String::from_utf8(output.stdout).expect("utf-8");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "form MainForm");
    assert_eq!(lines[1], " panel CommandBar");
    assert!(lines.contains(&"   listbox ProjectList"), "{text}");
    assert!(lines.contains(&" statusbar StatusBar"), "{text}");
    assert!(!text.contains("undefined"), "an element without a component was rendered:\n{text}");
    assert!(!text.contains("MainMenu") && !text.contains("SaveItem"), "the native menu was rendered:\n{text}");
    assert_eq!(lines.len(), 22, "21 visual elements and the props line:\n{text}");
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

#[test]
fn the_prebuilt_runtime_components_create_elements() {
    let out = TempDir::new().expect("temp dir");
    build(&starter(), out.path()).unwrap_or_else(|failure| panic!("build failed: {failure:?}"));
    let frontend = out.path().join("frontend");
    // `form` uses no hooks, so it can be called outside a renderer.
    let script = "const { C } = await import('./vcl.js');\nconst element = C.form({ name: 'Probe' });\nconsole.log(`${element.type} ${element.props['data-name']} ${element.props.className}`);\n";
    fs::write(frontend.join("probe.mjs"), script).expect("write");
    let output = Command::new("bun").arg("probe.mjs").current_dir(&frontend).output().expect("bun must be on PATH");
    assert!(output.status.success(), "a VCL component failed:\n{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "div Probe vcl-form\n");
}
