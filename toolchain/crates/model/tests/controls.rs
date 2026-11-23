//! The controls of spec 2.4, the form properties of 2.3 and the application
//! properties of 2.6, as the element table has them, and the compile errors for
//! props, types, events and references.

use std::fs;

use lunarstorm_model::schema::{lookup, Type, COMMON};
use lunarstorm_model::{extract, Code, Extraction};
use tempfile::TempDir;

/// A valid form class.
const CLASS: &str = "export class Main extends Form {\n  readonly go = (): void => {};\n}\n";

/// Extracts a project whose main form has the given root props and body.
///
/// # Arguments
/// * `root` - extra props of `<form>`.
/// * `body` - the children of `<form>`; starts on line 3.
///
/// # Returns
/// The extraction.
fn form(root: &str, body: &str) -> Extraction {
    let directory = TempDir::new().expect("temp dir");
    let write = |path: &str, text: &str| {
        let path = directory.path().join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    };
    write("src/app.tsx", "export default (<application id=\"a.b\" mainForm=\"Main\" />);\n");
    write("src/forms/main.view.tsx", &format!("export default (\n  <form name=\"Main\" {root}>\n{body}\n  </form>\n);\n"));
    write("src/forms/main.ts", CLASS);
    write("src/forms/other.view.tsx", "export default (<form name=\"Other\" />);\n");
    write("src/forms/other.ts", "export class Other extends Form {}\n");
    extract(directory.path())
}

/// # Arguments
/// * `extraction` - the result.
///
/// # Returns
/// `(code, line)` per diagnostic.
fn errors(extraction: &Extraction) -> Vec<(Code, u32)> {
    extraction.diagnostics.iter().map(|diagnostic| (diagnostic.code, diagnostic.at.line)).collect()
}

/// Asserts one element's own props and events against the spec's table.
///
/// # Arguments
/// * `tag` - the element.
/// * `props` - its props beyond the common ones.
/// * `events` - its events.
/// * `common` - whether it takes the common props.
fn row(tag: &str, props: &[&str], events: &[&str], common: bool) {
    let kind = lookup(tag).unwrap_or_else(|| panic!("<{tag}> is missing from the element table"));
    let own: Vec<&str> = kind.props.iter().map(|prop| prop.name).collect();
    assert_eq!(own, props, "props of <{tag}>");
    assert_eq!(kind.events, events, "events of <{tag}>");
    assert_eq!(kind.common, common, "common props on <{tag}>");
}

#[test]
fn the_element_table_is_the_spec_table() {
    let common: Vec<&str> = COMMON.iter().map(|prop| prop.name).collect();
    assert_eq!(common, ["caption", "align", "width", "height", "enabled", "visible", "hint", "tabOrder", "popupMenu"]);

    let layout = ["layout", "gap", "padding"];
    row("panel", &layout, &[], true);
    row("groupbox", &layout, &[], true);
    row("scrollbox", &layout, &[], true);
    row("pagecontrol", &["activePage"], &["onChange"], true);
    row("tabsheet", &layout, &[], true);
    row("splitter", &[], &[], true);
    row("toolbar", &[], &[], true);
    row("statusbar", &[], &[], true);
    row("label", &[], &[], true);
    row("image", &["src", "stretch", "proportional"], &[], true);
    row("progressbar", &["position", "min", "max"], &[], true);
    row("button", &["default", "cancel", "action"], &["onClick"], true);
    row("edit", &["text", "placeholder", "readOnly", "password"], &["onChange"], true);
    row("memo", &["text", "placeholder", "readOnly", "password", "wordWrap"], &["onChange"], true);
    row("checkbox", &["checked"], &["onClick"], true);
    row("radiogroup", &["items", "itemIndex"], &["onClick"], true);
    row("combobox", &["items", "itemIndex"], &["onChange"], true);
    row("trackbar", &["position", "min", "max"], &["onChange"], true);
    row("listbox", &["items", "itemIndex"], &["onChange"], true);
    row("listview", &["itemIndex", "rows"], &["onChange", "onDblClick"], true);
    row("column", &["caption", "width", "alignment"], &[], false);
    row("treeview", &["nodes", "selected"], &["onChange", "onExpanded"], true);
    row("mainmenu", &[], &[], false);
    row("popupmenu", &[], &[], false);
    row("menuitem", &["caption", "shortcut", "checked", "enabled", "visible", "action", "role"], &["onClick"], false);
    row("actionlist", &[], &[], false);
    row("action", &["caption", "shortcut", "enabled", "checked"], &["onExecute"], false);
    row("trayicon", &["icon", "hint", "popupMenu"], &["onClick"], false);
    row("timer", &["interval", "enabled"], &["onTimer"], false);

    let form_props = [
        "caption", "width", "height", "minWidth", "minHeight", "maxWidth", "maxHeight", "left", "top", "position",
        "borderStyle", "borderIcons", "windowState", "formStyle", "visible", "icon", "showInTaskbar", "transparent",
        "color", "theme", "owner", "modal", "titleBar", "contentProtection", "allWorkspaces", "cursor", "menu",
    ];
    let form_events = [
        "onCreate", "onShow", "onHide", "onCloseQuery", "onClose", "onActivate", "onDeactivate", "onResize", "onMove",
        "onDropFiles", "onThemeChange",
    ];
    row("form", &form_props, &form_events, false);
    let application_props =
        ["id", "name", "version", "icon", "mainForm", "singleInstance", "keepAlive", "urlSchemes", "activationPolicy", "theme"];
    row("application", &application_props, &["onReady", "onActivate", "onOpenFile", "onOpenUrl", "onQuit"], false);

    assert_eq!(lookup("listview").unwrap().prop("rows").map(|prop| prop.declared), Some(false));
    assert!(matches!(lookup("form").unwrap().prop("menu").map(|prop| prop.ty), Some(Type::Ref("mainmenu"))));
}

#[test]
fn props_are_ones_the_element_takes() {
    assert_eq!(errors(&form("", "    <label name=\"L\" capton=\"x\" />")), [(Code::Prop, 3)]);
    assert_eq!(errors(&form("", "    <label name=\"L\" text=\"x\" />")), [(Code::Prop, 3)], "label has no text");
    assert_eq!(errors(&form("", "    <timer name=\"T\" caption=\"x\" />")), [(Code::Prop, 3)], "no common props on a timer");
    assert_eq!(errors(&form("", "    <listview name=\"V\" rows={[]} />")), [(Code::Prop, 3)], "rows are data, not markup");
    assert_eq!(errors(&form("fontSize={12}", "")), [(Code::Prop, 2)]);
    assert_eq!(errors(&form("", "    <memo name=\"M\" wordWrap hint=\"h\" tabOrder={2} align=\"client\" />")), []);
}

#[test]
fn prop_values_have_the_declared_type() {
    for prop in [
        "caption={1}",
        "width=\"10\"",
        "enabled=\"yes\"",
        "align=\"middle\"",
        "visible={0}",
        "tabOrder={true}",
    ] {
        assert_eq!(errors(&form("", &format!("    <label name=\"L\" {prop} />"))), [(Code::Type, 3)], "{prop}");
    }
    assert_eq!(errors(&form("", "    <combobox name=\"C\" items={[1, 2]} />")), [(Code::Type, 3)]);
    assert_eq!(errors(&form("", "    <combobox name=\"C\" items=\"a\" />")), [(Code::Type, 3)]);
    for prop in [
        "position=\"center\"",
        "borderIcons={['minimize', 'shrink']}",
        "borderIcons=\"close\"",
        "color=\"red\"",
        "color=\"#12345\"",
        "windowState=\"hidden\"",
        "theme=\"blue\"",
        "titleBar={true}",
    ] {
        assert_eq!(errors(&form(prop, "")), [(Code::Type, 2)], "{prop}");
    }
    let valid = "position=\"ownerCenter\" borderIcons={['minimize', 'close']} color=\"#1a2B3c80\" borderStyle=\"dialog\" formStyle=\"stayOnTop\" titleBar=\"overlay\" modal owner=\"Other\"";
    assert_eq!(errors(&form(valid, "")), []);
    let message = &form("position=\"center\"", "").diagnostics[0].message;
    assert_eq!(message, "'position' takes one of designed, screenCenter, ownerCenter, default");
}

#[test]
fn events_are_ones_the_element_fires() {
    assert_eq!(errors(&form("", "    <label name=\"L\" onClick={form.go} />")), [(Code::Event, 3)]);
    assert_eq!(errors(&form("", "    <button name=\"B\" onChange={form.go} />")), [(Code::Event, 3)]);
    assert_eq!(errors(&form("onTimer={form.go}", "")), [(Code::Event, 2)]);
    assert_eq!(errors(&form("onResize={form.go}", "    <timer name=\"T\" onTimer={form.go} />")), []);
}

#[test]
fn references_name_an_element_of_the_right_kind() {
    let menus = "    <mainmenu name=\"Bar\"><menuitem name=\"Item\" action=\"Act\" /></mainmenu>\n    <popupmenu name=\"Pop\" />\n    <actionlist name=\"Acts\"><action name=\"Act\" onExecute={form.go} /></actionlist>";
    assert_eq!(errors(&form("menu=\"Bar\"", &format!("{menus}\n    <button name=\"B\" popupMenu=\"Pop\" action=\"Act\" />"))), []);
    assert_eq!(errors(&form("menu=\"Pop\"", menus)), [(Code::Reference, 2)], "a popup menu is not a main menu");
    assert_eq!(errors(&form("menu=\"Nope\"", menus)), [(Code::Reference, 2)]);
    assert_eq!(errors(&form("", &format!("{menus}\n    <button name=\"B\" popupMenu=\"Bar\" />"))), [(Code::Reference, 6)]);
    assert_eq!(errors(&form("", &format!("{menus}\n    <button name=\"B\" action=\"Item\" />"))), [(Code::Reference, 6)]);
    assert_eq!(errors(&form("owner=\"Nope\"", "")), [(Code::Reference, 2)]);
    let pages = "    <pagecontrol name=\"Pages\" activePage=\"Two\">\n      <tabsheet name=\"One\" />\n      <tabsheet name=\"Two\" />\n    </pagecontrol>";
    assert_eq!(errors(&form("", pages)), []);
    assert_eq!(errors(&form("", &pages.replace("\"Two\">", "\"Pages\">"))), [(Code::Reference, 3)]);
}

#[test]
fn the_application_has_an_identity_and_typed_props() {
    let run = |app: &str| {
        let directory = TempDir::new().expect("temp dir");
        fs::create_dir_all(directory.path().join("src/forms")).expect("mkdir");
        fs::write(directory.path().join("src/app.tsx"), app).expect("write");
        fs::write(directory.path().join("src/forms/main.view.tsx"), "export default (<form name=\"Main\" />);\n").expect("write");
        fs::write(directory.path().join("src/forms/main.ts"), CLASS).expect("write");
        errors(&extract(directory.path()))
    };
    assert_eq!(run("export default (<application mainForm=\"Main\" />);\n"), [(Code::Prop, 1)], "id is required");
    assert_eq!(run("export default (<application id=\"a\" mainForm=\"Main\" activationPolicy=\"hidden\" />);\n"), [(Code::Type, 1)]);
    assert_eq!(run("export default (<application id=\"a\" mainForm=\"Main\" urlSchemes=\"app\" />);\n"), [(Code::Type, 1)]);
    assert_eq!(run("export default (<application id=\"a\" mainForm=\"Main\" window=\"x\" />);\n"), [(Code::Prop, 1)]);
    let full = "export default (\n  <application id=\"a\" name=\"A\" version=\"1\" mainForm=\"Main\" singleInstance keepAlive={false} urlSchemes={['app']} activationPolicy=\"regular\" theme=\"dark\" onReady={form.go}>\n    <popupmenu name=\"TrayMenu\"><menuitem name=\"Quit\" role=\"quit\" /></popupmenu>\n    <trayicon name=\"Tray\" icon=\"res/tray.png\" popupMenu=\"TrayMenu\" onClick={form.go} />\n    <document name=\"Project\" extensions={['proj']} role=\"editor\" />\n    <permissions filesystem={['$HOME/Projects']} executables={['git']} environment={['HOME']} />\n  </application>\n);\n";
    assert_eq!(run(full), []);
    assert_eq!(run(&full.replace("role=\"quit\"", "role=\"exit\"")), [(Code::Type, 3)]);
    assert_eq!(run(&full.replace("popupMenu=\"TrayMenu\"", "popupMenu=\"Tray\"")), [(Code::Reference, 4)]);
}
