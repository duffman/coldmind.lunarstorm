//! Model extraction (spec 4a): reads `src/app.tsx` and every `*.view.tsx` of a
//! project from the AST into a [`Project`], and reports the compile errors of
//! the model: the parse rules, duplicate names, bad nesting, and handlers that
//! do not exist on the form class.
//!
//! Decisions:
//! - Nothing is executed and no import is followed. A view resource is paired
//!   with its form class by file name (`x.view.tsx` with `x.ts`, spec 2.7).
//! - A model with any error is not returned: generators only ever see a valid
//!   project.
//! - Application events (`onReady`, …) name handlers as `form.<member>` like every
//!   other event. The only reference form the parse rules allow is `form.<member>`,
//!   and the application names one form, `mainForm`, so they resolve against the
//!   main form's class.
//! - Paths in the model and in diagnostics are relative to the project root, so
//!   output does not depend on where the project is checked out.

mod check;
mod class;
mod model;
mod natives;
mod resource;
pub mod schema;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub use model::{Code, Diagnostic, Element, Extraction, Form, Position, Project, Prop, Value};
use resource::Report;

/// The application resource, relative to the project root (spec 2.7).
pub const APPLICATION: &str = "src/app.tsx";

/// The suffix of a view resource (spec 2.7).
pub const VIEW_SUFFIX: &str = ".view.tsx";

/// Extracts the model of the project at `root`.
///
/// # Arguments
/// * `root` - the project directory, the one holding `src/`.
///
/// # Returns
/// The project when it has no compile error; otherwise the diagnostics, ordered
/// by file and position.
pub fn extract(root: &Path) -> Extraction {
    let mut diagnostics = Vec::new();

    let application_path = PathBuf::from(APPLICATION);
    let application = load(root, &application_path, &mut diagnostics).and_then(|source| {
        resource::read(&source, "application", &mut Report::new(&application_path, &source, &mut diagnostics))
    });

    let mut views = Vec::new();
    collect_views(&root.join("src"), root, &mut views);
    views.sort();

    let mut forms = Vec::new();
    let mut classes: HashMap<String, Vec<class::Member>> = HashMap::new();
    let mut seen: HashMap<String, PathBuf> = HashMap::new();
    for view in views {
        let Some(source) = load(root, &view, &mut diagnostics) else { continue };
        let Some(tree) = resource::read(&source, "form", &mut Report::new(&view, &source, &mut diagnostics)) else {
            continue;
        };
        if tree.name.is_empty() {
            continue;
        }
        if let Some(first) = seen.get(&tree.name) {
            let message = format!("the form '{}' is already defined in {}", tree.name, first.display());
            diagnostics.push(Diagnostic { code: Code::Duplicate, file: view.clone(), at: tree.at, message });
            continue;
        }
        seen.insert(tree.name.clone(), view.clone());

        let class_path = class_file(&view);
        if let Some(class_source) = load(root, &class_path, &mut diagnostics) {
            let mut report = Report::new(&class_path, &class_source, &mut diagnostics);
            if let Some(members) = class::members(&class_source, &tree, &mut report) {
                check_handlers(&tree, &tree.name, &members, &view, &mut diagnostics);
                classes.insert(tree.name.clone(), members);
            }
        }
        forms.push(Form { name: tree.name.clone(), view, class: class_path, root: tree });
    }

    let form_names: Vec<String> = forms.iter().map(|form| form.name.clone()).collect();
    for form in &forms {
        check::tree(&form.root, &form_names, &form.view, &mut diagnostics);
    }

    if let Some(application) = &application {
        check_application(application, &form_names, &application_path, &mut diagnostics);
        match application.prop("mainForm") {
            Some(Value::Text(main)) if seen.contains_key(main) => {
                if let Some(members) = classes.get(main) {
                    check_handlers(application, main, members, &application_path, &mut diagnostics);
                }
            }
            // A `mainForm` that names no form is reported by `check_application`'s
            // reference check; only its absence is reported here.
            Some(_) => {}
            None => diagnostics.push(Diagnostic {
                code: Code::MainForm,
                file: application_path.clone(),
                at: application.at,
                message: "<application> needs mainForm, the name of the form shown at startup".to_string(),
            }),
        }
    }

    diagnostics.sort_by(|a, b| (&a.file, a.at.line, a.at.column).cmp(&(&b.file, b.at.line, b.at.column)));
    let project = match application {
        Some(application) if diagnostics.is_empty() => {
            let natives = natives::collect(root);
            Some(Project { application, forms, natives })
        }
        _ => None,
    };
    Extraction { project, diagnostics }
}

/// Checks the application resource against the element table. A `mainForm` that
/// names no form is reported as [`Code::MainForm`] rather than as a plain
/// reference error, because nothing can start without it.
///
/// # Arguments
/// * `application` - the `<application>` element.
/// * `forms` - the names of every form of the project.
/// * `file` - the application resource.
/// * `diagnostics` - receives the errors.
fn check_application(application: &Element, forms: &[String], file: &Path, diagnostics: &mut Vec<Diagnostic>) {
    let first = diagnostics.len();
    check::tree(application, forms, file, diagnostics);
    let main = application.props.iter().find(|prop| prop.name == "mainForm").map(|prop| prop.at);
    for diagnostic in &mut diagnostics[first..] {
        if diagnostic.code == Code::Reference && Some(diagnostic.at) == main {
            diagnostic.code = Code::MainForm;
            diagnostic.at = application.at;
        }
    }
    if application.prop("id").is_none() {
        let message = "<application> needs id, the application's identity".to_string();
        diagnostics.push(Diagnostic { code: Code::Prop, file: file.to_path_buf(), at: application.at, message });
    }
}

/// Reads a project file, reporting a [`Code::File`] error when it cannot be read.
///
/// # Arguments
/// * `root` - the project directory.
/// * `file` - the file, relative to `root`.
/// * `diagnostics` - receives the error.
///
/// # Returns
/// The file's text, or `None` after reporting.
fn load(root: &Path, file: &Path, diagnostics: &mut Vec<Diagnostic>) -> Option<String> {
    match fs::read_to_string(root.join(file)) {
        Ok(source) => Some(source),
        Err(error) => {
            let message = format!("cannot read {}: {error}", file.display());
            diagnostics.push(Diagnostic { code: Code::File, file: file.to_path_buf(), at: Position { line: 1, column: 1 }, message });
            None
        }
    }
}

/// Collects every view resource under a directory, recursively.
///
/// # Arguments
/// * `directory` - where to look.
/// * `root` - the project directory, to make the paths relative.
/// * `into` - receives the paths, relative to `root`.
fn collect_views(directory: &Path, root: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            // `node_modules` under `src` is never the project's own code.
            if name != "node_modules" {
                collect_views(&path, root, into);
            }
        } else if name.ends_with(VIEW_SUFFIX) {
            if let Ok(relative) = path.strip_prefix(root) {
                into.push(relative.to_path_buf());
            }
        }
    }
}

/// Names the form class file paired with a view resource: `x.view.tsx` with `x.ts`.
///
/// # Arguments
/// * `view` - the view resource path.
///
/// # Returns
/// The form class path.
fn class_file(view: &Path) -> PathBuf {
    let name = view.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = name.strip_suffix(VIEW_SUFFIX).unwrap_or(&name);
    view.with_file_name(format!("{stem}.ts"))
}

/// Checks that every handler a resource names exists on the form class and is
/// public (spec 4a).
///
/// # Arguments
/// * `tree` - the resource's root element.
/// * `form` - the form class's name, for messages.
/// * `members` - the class's members.
/// * `file` - the resource, for the diagnostics.
/// * `diagnostics` - receives [`Code::Handler`] errors.
fn check_handlers(tree: &Element, form: &str, members: &[class::Member], file: &Path, diagnostics: &mut Vec<Diagnostic>) {
    tree.walk(&mut |element| {
        for prop in &element.props {
            let Value::Handler(handler) = &prop.value else { continue };
            let message = match members.iter().find(|member| &member.name == handler) {
                Some(member) if member.public => continue,
                Some(_) => format!("'{handler}' is not public on {form}; a handler is a public member"),
                None => format!("{form} has no member '{handler}'"),
            };
            diagnostics.push(Diagnostic { code: Code::Handler, file: file.to_path_buf(), at: prop.at, message });
        }
    });
}
