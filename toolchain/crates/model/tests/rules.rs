//! The compile errors of the model (spec 2.2 and 4a), each on a project that is
//! valid except for the one thing under test.

use std::fs;
use std::path::Path;

use lunarstorm_model::{extract, Code, Extraction, Value};
use tempfile::TempDir;

/// A valid application resource.
const APP: &str = "export default (<application id=\"a.b\" name=\"App\" version=\"1.0.0\" mainForm=\"Main\" />);\n";

/// A valid form class with public, private and static members.
const CLASS: &str = "import { Form } from '@desktop/forms';\n\
export class Main extends Form {\n\
  readonly save = (): void => {};\n\
  open(): void {}\n\
  private readonly hidden = (): void => {};\n\
  static make(): void {}\n\
}\n";

/// Builds a view resource around a form body. The body starts on line 4.
///
/// # Arguments
/// * `body` - the children of `<form>`.
///
/// # Returns
/// The file's text.
fn view(body: &str) -> String {
    format!("declare const form: any;\nexport default (\n  <form name=\"Main\" caption=\"Main\">\n{body}\n  </form>\n);\n")
}

/// Writes a project and extracts it.
///
/// # Arguments
/// * `files` - project-relative paths with their text.
///
/// # Returns
/// The extraction.
fn run(files: &[(&str, &str)]) -> Extraction {
    let directory = TempDir::new().expect("temp dir");
    for (path, text) in files {
        let path = directory.path().join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }
    extract(directory.path())
}

/// Extracts a project that is valid except for its form body.
///
/// # Arguments
/// * `body` - the children of `<form>`.
///
/// # Returns
/// The extraction.
fn form(body: &str) -> Extraction {
    run(&[("src/app.tsx", APP), ("src/forms/main.view.tsx", &view(body)), ("src/forms/main.ts", CLASS)])
}

/// Reduces diagnostics to what the tests assert on.
///
/// # Arguments
/// * `extraction` - the result.
///
/// # Returns
/// `(code, file, line)` per diagnostic; also asserts no model was produced when
/// there is any.
fn errors(extraction: &Extraction) -> Vec<(Code, String, u32)> {
    if !extraction.diagnostics.is_empty() {
        assert!(extraction.project.is_none(), "a model with errors must not be returned");
    }
    extraction.diagnostics.iter().map(|d| (d.code, d.file.to_string_lossy().into_owned(), d.at.line)).collect()
}

/// # Arguments
/// * `code` - the expected code.
/// * `line` - the expected line in the view resource.
///
/// # Returns
/// The one expected diagnostic, in the view resource.
fn at(code: Code, line: u32) -> Vec<(Code, String, u32)> {
    vec![(code, "src/forms/main.view.tsx".to_string(), line)]
}

#[test]
fn a_valid_form_has_no_errors() {
    let extraction = form("    <button name=\"Save\" caption={`Save`} tabOrder={+1} onClick={form.save} />\n    {/* note */}");
    assert_eq!(errors(&extraction), vec![]);
    let project = extraction.project.expect("model");
    let button = &project.forms[0].root.children[0];
    assert_eq!(button.prop("caption"), Some(&Value::Text("Save".into())), "a plain template is a string");
    assert_eq!(button.prop("tabOrder"), Some(&Value::Number(1.0)));
}

#[test]
fn prop_values_are_literals_or_handlers() {
    for value in [
        "{compute()}",
        "{flag ? 'a' : 'b'}",
        "{`a${1}`}",
        "{1 + 2}",
        "{title}",
        "{{ a: 1 }}",
        "{() => {}}",
        "{other.member}",
        "{form.a.b}",
        "{form?.save}",
        "{[[1]]}",
        "{[form.save]}",
        "{[...items]}",
        "{[1, , 2]}",
        "{-x}",
        "{!true}",
        "{null}",
        "{<label name=\"X\" />}",
    ] {
        let extraction = form(&format!("    <label name=\"L\" caption={value} />"));
        assert_eq!(errors(&extraction), at(Code::Value, 4), "caption={value}");
    }
    assert_eq!(errors(&form("    <label name=\"L\" {...props} />")), at(Code::Value, 4));
    assert_eq!(errors(&form("    <label name=\"L\" a:b=\"1\" />")), at(Code::Value, 4));
}

#[test]
fn only_model_elements_are_tags() {
    assert_eq!(errors(&form("    <Widget name=\"W\" />")), at(Code::Tag, 4));
    assert_eq!(errors(&form("    <vcl.button name=\"W\" />")), at(Code::Tag, 4));
    assert_eq!(errors(&form("    <div name=\"W\" />")), at(Code::Tag, 4));
    assert_eq!(errors(&form("    <></>")), at(Code::Nesting, 4));
}

#[test]
fn names_are_required_valid_and_unique() {
    assert_eq!(errors(&form("    <label caption=\"x\" />")), at(Code::Name, 4));
    assert_eq!(errors(&form("    <label name={1} />")), at(Code::Name, 4));
    assert_eq!(errors(&form("    <label name=\"1st\" />")), at(Code::Name, 4));
    assert_eq!(errors(&form("    <label name=\"my label\" />")), at(Code::Name, 4));
    assert_eq!(errors(&form("    <label name=\"A\" />\n    <panel name=\"P\">\n      <edit name=\"A\" />\n    </panel>")), at(Code::Duplicate, 6));
    assert_eq!(errors(&form("    <label name=\"Main\" />")), at(Code::Duplicate, 4), "the form's own name counts");
}

#[test]
fn nesting_follows_the_element_table() {
    assert_eq!(errors(&form("    <label name=\"L\">\n      <button name=\"B\" />\n    </label>")), at(Code::Nesting, 5));
    assert_eq!(errors(&form("    <tabsheet name=\"T\" />")), at(Code::Nesting, 4));
    assert_eq!(errors(&form("    <pagecontrol name=\"P\">\n      <label name=\"L\" />\n    </pagecontrol>")), at(Code::Nesting, 5));
    assert_eq!(errors(&form("    <panel name=\"P\">\n      <timer name=\"T\" />\n    </panel>")), at(Code::Nesting, 5));
    assert_eq!(errors(&form("    <toolbar name=\"T\">\n      <edit name=\"E\" />\n    </toolbar>")), at(Code::Nesting, 5));
    assert_eq!(errors(&form("    <menuitem name=\"M\" />")), at(Code::Nesting, 4));
    assert_eq!(errors(&form("    <form name=\"Inner\" />")), at(Code::Nesting, 4));
    assert_eq!(errors(&form("    <panel name=\"P\">text</panel>")), at(Code::Nesting, 4));
    assert_eq!(errors(&form("    <panel name=\"P\">{items}</panel>")), at(Code::Nesting, 4));
    let valid = "    <pagecontrol name=\"Pages\">\n      <tabsheet name=\"One\">\n        <listview name=\"Rows\">\n          <column name=\"C\" caption=\"c\" />\n        </listview>\n      </tabsheet>\n    </pagecontrol>\n    <timer name=\"T\" interval={1000} />\n    <actionlist name=\"Actions\">\n      <action name=\"A\" />\n    </actionlist>";
    assert_eq!(errors(&form(valid)), vec![]);
}

#[test]
fn events_name_handlers_and_only_events_do() {
    assert_eq!(errors(&form("    <button name=\"B\" onClick=\"save\" />")), at(Code::Event, 4));
    assert_eq!(errors(&form("    <button name=\"B\" onClick />")), at(Code::Event, 4));
    assert_eq!(errors(&form("    <button name=\"B\" caption={form.save} />")), at(Code::Event, 4));
    assert_eq!(errors(&form("    <button name=\"B\" caption=\"a\" caption=\"b\" />")), at(Code::Repeated, 4));
}

#[test]
fn handlers_exist_on_the_form_class_and_are_public() {
    assert_eq!(errors(&form("    <button name=\"B\" onClick={form.open} />")), vec![], "a method is a handler");
    assert_eq!(errors(&form("    <button name=\"B\" onClick={form.missing} />")), at(Code::Handler, 4));
    assert_eq!(errors(&form("    <button name=\"B\" onClick={form.hidden} />")), at(Code::Handler, 4));
    assert_eq!(errors(&form("    <button name=\"B\" onClick={form.make} />")), at(Code::Handler, 4), "a static is not on the form");
    let message = &form("    <button name=\"B\" onClick={form.missing} />").diagnostics[0].message;
    assert_eq!(message, "Main has no member 'missing'");
}

#[test]
fn every_error_in_a_file_is_reported() {
    let body = "    <label name=\"A\" caption={f()} />\n    <Widget />\n    <label name=\"A\" />\n    <button name=\"B\" onClick={form.missing} />";
    let expected = vec![
        (Code::Value, "src/forms/main.view.tsx".to_string(), 4),
        (Code::Tag, "src/forms/main.view.tsx".to_string(), 5),
        (Code::Duplicate, "src/forms/main.view.tsx".to_string(), 6),
        (Code::Handler, "src/forms/main.view.tsx".to_string(), 7),
    ];
    assert_eq!(errors(&form(body)), expected);
}

#[test]
fn a_resource_is_one_default_exported_root() {
    let files = |view: &str| run(&[("src/app.tsx", APP), ("src/forms/main.view.tsx", view), ("src/forms/main.ts", CLASS)]);
    let main_form = (Code::MainForm, "src/app.tsx".to_string(), 1);
    let view_error = |code, line| vec![main_form.clone(), (code, "src/forms/main.view.tsx".to_string(), line)];

    assert_eq!(errors(&files("export default (<panel name=\"Main\" />);\n")), view_error(Code::Root, 1));
    assert_eq!(errors(&files("export const view = 1;\n")), vec![main_form.clone(), (Code::Statement, "src/forms/main.view.tsx".to_string(), 1), (Code::Root, "src/forms/main.view.tsx".to_string(), 1)]);
    assert_eq!(errors(&files("export default function view() { return <form name=\"Main\" />; }\n")), view_error(Code::Root, 1));
    assert_eq!(errors(&files("console.log(1);\nexport default (<form name=\"Main\" />);\n")), vec![(Code::Statement, "src/forms/main.view.tsx".to_string(), 1)]);
    assert_eq!(errors(&files("const x = 1;\nexport default (<form name=\"Main\" />);\n")), vec![(Code::Statement, "src/forms/main.view.tsx".to_string(), 1)]);
    assert_eq!(errors(&files("export default (<form name=\"Main\" >);\n")), view_error(Code::Syntax, 1));
    let imports = "import type { Main } from './main';\ntype T = number;\ninterface I {}\ndeclare const form: Main;\nexport default <form name=\"Main\" />;\n";
    assert_eq!(errors(&files(imports)), vec![]);
}

#[test]
fn the_form_class_is_paired_by_file_and_name() {
    let project = |class: Option<&str>| {
        let view = view("    <button name=\"B\" onClick={form.save} />");
        let mut files = vec![("src/app.tsx", APP), ("src/forms/main.view.tsx", view.as_str())];
        if let Some(class) = class {
            files.push(("src/forms/main.ts", class));
        }
        errors(&run(&files))
    };
    let class = |code, line| vec![(code, "src/forms/main.ts".to_string(), line)];
    assert_eq!(project(None), class(Code::File, 1));
    assert_eq!(project(Some("export class Other extends Form {}\n")), class(Code::Class, 1));
    assert_eq!(project(Some("\nclass Main extends Form { save() {} }\n")), class(Code::Class, 2));
    assert_eq!(project(Some("export class Main { save() {} }\n")), class(Code::Class, 1));
    assert_eq!(project(Some("export class Main extends Form {\n")), class(Code::Syntax, 2));
    assert_eq!(project(Some("export default class Main extends Form { save() {} }\n")), vec![]);
}

#[test]
fn the_application_names_an_existing_main_form() {
    let project = |app: &str| {
        let view = view("");
        errors(&run(&[("src/app.tsx", app), ("src/forms/main.view.tsx", view.as_str()), ("src/forms/main.ts", CLASS)]))
    };
    let app = |code| vec![(code, "src/app.tsx".to_string(), 1)];
    assert_eq!(project("export default (<application id=\"a\" name=\"A\" mainForm=\"Nope\" />);\n"), app(Code::MainForm));
    assert_eq!(project("export default (<application id=\"a\" name=\"A\" />);\n"), app(Code::MainForm));
    assert_eq!(project("export default (<form name=\"Main\" />);\n"), app(Code::Root));
    assert_eq!(project("export default (<application id=\"a\" mainForm=\"Main\" onReady={form.save} />);\n"), vec![]);
    assert_eq!(project("export default (<application id=\"a\" mainForm=\"Main\" onReady={form.nope} />);\n"), app(Code::Handler));
    assert_eq!(project("export default (<application id=\"a\" mainForm=\"Main\"><panel name=\"P\" /></application>);\n"), app(Code::Nesting));
    let children = "export default (<application id=\"a\" mainForm=\"Main\"><trayicon name=\"Tray\" /><permissions /><document /></application>);\n";
    assert_eq!(project(children), vec![]);

    let missing = errors(&run(&[("src/forms/main.view.tsx", view("").as_str()), ("src/forms/main.ts", CLASS)]));
    assert_eq!(missing, app(Code::File));
}

#[test]
fn form_names_are_unique_across_the_project() {
    let view = view("");
    let extraction = run(&[
        ("src/app.tsx", APP),
        ("src/forms/a.view.tsx", view.as_str()),
        ("src/forms/a.ts", CLASS),
        ("src/forms/b.view.tsx", view.as_str()),
        ("src/forms/b.ts", CLASS),
    ]);
    assert_eq!(errors(&extraction), vec![(Code::Duplicate, "src/forms/b.view.tsx".to_string(), 3)]);
    assert!(Path::new("src/forms/a.view.tsx").is_relative());
}

#[test]
fn find_is_resolved_against_the_view_at_compile_time() {
    let body = "    <button name=\"Go\" caption=\"Go\" />\n    <listbox name=\"List\" />";
    let class = |finds: &str| format!("import {{ Form }} from '@desktop/forms';\nexport class Main extends Form {{\n{finds}\n}}\n");
    let check = |finds: &str| {
        errors(&run(&[("src/app.tsx", APP), ("src/forms/main.view.tsx", &view(body)), ("src/forms/main.ts", &class(finds))]))
    };
    let file = "src/forms/main.ts".to_string();

    // Typed, untyped and deliberately widened finds of existing controls.
    assert_eq!(check("  go = this.find<Button>('Go');\n  list = this.find('List');\n  any = this.find<any>('Go');"), []);
    // An unknown name, also inside a method.
    assert_eq!(check("  run(): void {\n    this.find<Button>('Gone');\n  }"), [(Code::Find, file.clone(), 4)]);
    // The type of another kind.
    assert_eq!(check("  go = this.find<ListBox>('Go');"), [(Code::Find, file.clone(), 3)]);
    // A name that cannot be resolved.
    assert_eq!(check("  go = this.find<Button>(name());"), [(Code::Find, file.clone(), 3)]);
    // The form itself is not a control.
    assert_eq!(check("  self = this.find<Form>('Main');"), [(Code::Find, file, 3)]);
}
