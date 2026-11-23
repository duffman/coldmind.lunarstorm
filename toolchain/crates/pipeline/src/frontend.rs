//! Frontend generation (spec 4b): the files served to the WebViews.
//!
//! ```text
//! frontend/index.html         the page of every form's WebView
//! frontend/main.js            mounts the form of this WebView
//! frontend/vcl.js             React, the React VCL and `mount`, prebuilt
//! frontend/vcl.css            the look of the VCL
//! frontend/forms/<Form>.js    one generated module per form
//! ```
//!
//! Decisions:
//! - A form module is plain JavaScript calling `h` (`React.createElement`) on the
//!   components of `C`, both imported from the prebuilt `vcl.js`. Nothing is
//!   transformed or bundled per project, so a view change regenerates one small
//!   file, and the host's `forms/<Form>.*` convention maps it to `dev.refresh` of
//!   that form alone (spec 6.1).
//! - Children are passed as arguments of `h`, never as an array, so React asks
//!   for no keys: the order of a view resource is static.
//! - An element without a component is non-visual (`mainmenu`, `timer`, ...):
//!   the host creates it natively from the host configuration, so it and its
//!   subtree are left out of the page.
//! - Events are not emitted into the page. A component reports every event of
//!   its kind through the bridge; the host checks them against the bridge table
//!   (spec 4d) and the backend runs the handlers.
//! - `vcl.js` is the build product of `templates/zebra.framework` (`bun run
//!   build`), embedded here so a project build needs no JavaScript toolchain.

use lunarstorm_model::{Element, Project, Value};
use serde_json::{Map, Value as Json};

use crate::host::literal;

/// The prebuilt WebView runtime: React, the React VCL and `mount`.
const VCL: &str = include_str!("../../../../templates/zebra.framework/dist/vcl.js");

/// The stylesheet of the React VCL.
const STYLE: &str = include_str!("../../../../framework/vcl/react/src/vcl.css");

/// The page of every form's WebView. The host injects its shell before any
/// script of the page runs.
const PAGE: &str = "<!doctype html>\n<html>\n  <head>\n    <meta charset=\"utf-8\" />\n    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n    <link rel=\"stylesheet\" href=\"vcl.css\" />\n  </head>\n  <body>\n    <div id=\"root\"></div>\n    <script type=\"module\" src=\"main.js\"></script>\n  </body>\n</html>\n";

/// Mounts the form of this WebView. The stamp in the query makes `dev.refresh`
/// import the regenerated module instead of the cached one.
const MAIN: &str = "import { mount } from './vcl.js';\n\nmount((name, stamp) => import(`./forms/${name}.js?t=${stamp}`));\n";

/// The element names that have a component in the React VCL (`COMPONENTS` in
/// `framework/vcl/react/src/index.ts`; a test keeps the two equal).
pub const VISUAL: &[&str] = &[
    "form", "panel", "groupbox", "scrollbox", "pagecontrol", "tabsheet", "splitter", "toolbar", "statusbar", "label",
    "image", "progressbar", "button", "edit", "memo", "checkbox", "radiogroup", "combobox", "trackbar", "listbox",
    "listview", "column", "treeview",
];

/// Writes the `h(...)` call of an element and its visual children.
///
/// # Arguments
/// * `element` - a visual element.
/// * `depth` - nesting depth, for indentation.
/// * `out` - the module text being written.
fn render(element: &Element, depth: usize, out: &mut String) {
    let mut props = Map::new();
    if !element.name.is_empty() {
        props.insert("name".into(), Json::String(element.name.clone()));
    }
    for prop in &element.props {
        // `name` is already in; a handler is backend code, not a page prop.
        if prop.name != "name" && !matches!(prop.value, Value::Handler(_)) {
            props.insert(prop.name.clone(), literal(&prop.value));
        }
    }
    let indent = "  ".repeat(depth);
    out.push_str(&format!("{indent}h(C.{}, {}", element.tag, Json::Object(props)));
    for child in element.children.iter().filter(|child| VISUAL.contains(&child.tag)) {
        out.push_str(",\n");
        render(child, depth + 1, out);
    }
    out.push(')');
}

/// Generates the module of one form.
///
/// # Arguments
/// * `root` - the form's `<form>` element.
///
/// # Returns
/// The module source: a default-exported component rendering the form.
pub fn form_module(root: &Element) -> String {
    let mut out = String::from("// Generated from the form's view resource. Do not edit.\nimport { h, C } from '../vcl.js';\n\nexport default () =>\n");
    render(root, 1, &mut out);
    out.push_str(";\n");
    out
}

/// Generates every frontend file of a project.
///
/// # Arguments
/// * `project` - the extracted model.
///
/// # Returns
/// The files as (path under `frontend/`, content), in a stable order.
pub fn generate(project: &Project) -> Vec<(String, String)> {
    let mut files = vec![
        ("index.html".to_string(), PAGE.to_string()),
        ("main.js".to_string(), MAIN.to_string()),
        ("vcl.js".to_string(), VCL.to_string()),
        ("vcl.css".to_string(), STYLE.to_string()),
    ];
    files.extend(project.forms.iter().map(|form| (format!("forms/{}.js", form.name), form_module(&form.root))));
    files
}
