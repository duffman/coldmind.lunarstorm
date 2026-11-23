//! Host configuration (spec 4e) and the bridge table (spec 4d), as the
//! `zebra.json` the native host reads.
//!
//! Decisions:
//! - The format is the host's `HostConfig` (schema 1): application, forms as
//!   windows with their control tables, menus, trays, timers, actions, documents
//!   and permissions. Everything the host needs to create windows, menus and
//!   tray before any JavaScript runs is here (spec 6, "Startup order").
//! - The bridge table of a control lists every prop and every event its kind
//!   has, not only the ones the view resource sets or connects. A user edit must
//!   reach the backend's control proxy even when no handler is connected, and the
//!   form class may write any prop of the control. The host validates every
//!   message against this table (spec 6, "Security").
//! - A form's own `events`, a menu item's and a tray's list only the connected
//!   ones: for those the host decides by the list whether to involve the backend
//!   at all (for example whether a close has to wait for `onCloseQuery`).
//! - A control with a `popupMenu` gets the `onContextPopup` event in its table;
//!   the host answers it by showing the native menu.
//! - Output is deterministic: props keep source order, lists keep document
//!   order, control tables are sorted by name.

use lunarstorm_model::schema;
use lunarstorm_model::{Element, Project, Value};
use serde_json::{json, Map, Value as Json};

/// The host configuration schema this generator writes.
pub const HOST_SCHEMA: u32 = 1;

/// The owner name of application-wide components, as the host knows it.
pub const APPLICATION_OWNER: &str = "@application";

/// The event a control sends to ask the host for its popup menu.
pub const CONTEXT_POPUP: &str = "onContextPopup";

/// Converts a model value to JSON.
///
/// # Arguments
/// * `value` - the model value; never a handler, which callers filter out.
///
/// # Returns
/// The JSON value. A number without a fraction is written as an integer.
pub fn literal(value: &Value) -> Json {
    match value {
        Value::Text(text) => json!(text),
        Value::Number(number) if number.fract() == 0.0 && number.abs() < 9.0e15 => json!(*number as i64),
        Value::Number(number) => json!(number),
        Value::Bool(flag) => json!(flag),
        Value::List(items) => Json::Array(items.iter().map(literal).collect()),
        Value::Handler(_) => Json::Null,
    }
}

/// Lists the events an element connects to a handler.
///
/// # Arguments
/// * `element` - the element.
///
/// # Returns
/// The event names, in source order.
pub fn connected(element: &Element) -> Vec<&str> {
    element.props.iter().filter(|prop| matches!(prop.value, Value::Handler(_))).map(|prop| prop.name.as_str()).collect()
}

/// Collects an element's literal props into a JSON object.
///
/// # Arguments
/// * `element` - the element.
/// * `rename` - maps a model prop name to the host's field name; `None` drops it.
///
/// # Returns
/// The object, in source order.
fn object(element: &Element, rename: impl Fn(&str) -> Option<&str>) -> Map<String, Json> {
    let mut map = Map::new();
    for prop in &element.props {
        if matches!(prop.value, Value::Handler(_)) {
            continue;
        }
        if let Some(name) = rename(&prop.name) {
            map.insert(name.to_string(), literal(&prop.value));
        }
    }
    map
}

/// Builds the bridge table of one form (spec 4d): every named element under the
/// form with its kind, props and events.
///
/// # Arguments
/// * `form` - the `<form>` element.
///
/// # Returns
/// Control name to its table entry, sorted by name.
pub fn bridge_table(form: &Element) -> Map<String, Json> {
    let mut controls: Vec<(String, Json)> = Vec::new();
    form.walk(&mut |element| {
        if std::ptr::eq(element, form) || element.name.is_empty() {
            return;
        }
        let Some(kind) = schema::lookup(element.tag) else { return };
        let props: Vec<&str> = kind.all_props().map(|prop| prop.name).collect();
        let mut events: Vec<&str> = kind.events.to_vec();
        let mut entry = Map::new();
        entry.insert("type".into(), json!(element.tag));
        if let Some(Value::Text(popup)) = element.prop("popupMenu") {
            // Only a visual control asks for its menu; a tray icon's is native.
            if kind.common {
                events.push(CONTEXT_POPUP);
                entry.insert("popupMenu".into(), json!(popup));
            }
        }
        entry.insert("props".into(), json!(props));
        entry.insert("events".into(), json!(events));
        controls.push((element.name.clone(), Json::Object(entry)));
    });
    controls.sort_by(|a, b| a.0.cmp(&b.0));
    controls.into_iter().collect()
}

/// Builds one menu item and its submenu.
///
/// # Arguments
/// * `item` - the `<menuitem>` element.
///
/// # Returns
/// The host's menu item.
fn menu_item(item: &Element) -> Json {
    let mut map = Map::new();
    map.insert("name".into(), json!(item.name));
    let fields = object(item, |name| matches!(name, "caption" | "shortcut" | "checked" | "enabled" | "action" | "role").then_some(name));
    map.extend(fields);
    let events = connected(item);
    if !events.is_empty() {
        map.insert("events".into(), json!(events));
    }
    let items: Vec<Json> = item.children.iter().filter(|child| child.tag == "menuitem").map(menu_item).collect();
    if !items.is_empty() {
        map.insert("items".into(), Json::Array(items));
    }
    Json::Object(map)
}

/// The native components of one owner (a form or the application), split by kind.
#[derive(Default)]
struct Natives {
    /// Main and popup menus.
    menus: Vec<Json>,
    /// Tray icons.
    trays: Vec<Json>,
    /// Timers.
    timers: Vec<Json>,
    /// Actions.
    actions: Vec<Json>,
}

impl Natives {
    /// Adds the non-visual components declared directly under an owner.
    ///
    /// # Arguments
    /// * `owner` - the form's name, or [`APPLICATION_OWNER`].
    /// * `root` - the `<form>` or `<application>` element.
    fn add(&mut self, owner: &str, root: &Element) {
        for child in &root.children {
            match child.tag {
                "mainmenu" | "popupmenu" => {
                    let kind = if child.tag == "mainmenu" { "main" } else { "popup" };
                    let items: Vec<Json> = child.children.iter().map(menu_item).collect();
                    self.menus.push(json!({ "name": child.name, "owner": owner, "kind": kind, "items": items }));
                }
                "trayicon" => {
                    let mut map = Map::new();
                    map.insert("name".into(), json!(child.name));
                    map.insert("owner".into(), json!(owner));
                    map.extend(object(child, |name| matches!(name, "icon" | "hint" | "popupMenu").then_some(name)));
                    map.insert("events".into(), json!(connected(child)));
                    self.trays.push(Json::Object(map));
                }
                "timer" => {
                    // Delphi's defaults: a timer is enabled and fires every second.
                    let interval = match child.prop("interval") {
                        Some(Value::Number(interval)) if *interval >= 1.0 => *interval as u64,
                        _ => 1000,
                    };
                    let enabled = !matches!(child.prop("enabled"), Some(Value::Bool(false)));
                    self.timers.push(json!({ "name": child.name, "owner": owner, "interval": interval, "enabled": enabled }));
                }
                "actionlist" => {
                    for action in &child.children {
                        let mut map = Map::new();
                        map.insert("name".into(), json!(action.name));
                        map.insert("owner".into(), json!(owner));
                        map.extend(object(action, |name| matches!(name, "caption" | "shortcut" | "enabled" | "checked").then_some(name)));
                        self.actions.push(Json::Object(map));
                    }
                }
                _ => {}
            }
        }
    }
}

/// Generates the host configuration of a project.
///
/// # Arguments
/// * `project` - the extracted model.
///
/// # Returns
/// The `zebra.json` document.
pub fn host_config(project: &Project) -> Json {
    let app = &project.application;
    let mut application = object(app, |name| match name {
        "id" | "name" | "version" | "icon" | "mainForm" | "singleInstance" | "keepAlive" | "urlSchemes"
        | "activationPolicy" | "theme" => Some(name),
        _ => None,
    });
    // The host requires a display name; the identity stands in for a missing one.
    if !application.contains_key("name") {
        let id = application.get("id").cloned().unwrap_or_else(|| json!(""));
        application.insert("name".into(), id);
    }
    application.insert("events".into(), json!(connected(app)));

    let mut natives = Natives::default();
    natives.add(APPLICATION_OWNER, app);
    let mut forms = Vec::new();
    for form in &project.forms {
        natives.add(&form.name, &form.root);
        forms.push(json!({
            "name": form.name,
            "props": object(&form.root, |name| Some(name)),
            "events": connected(&form.root),
            "controls": bridge_table(&form.root),
        }));
    }

    let documents: Vec<Json> =
        app.children.iter().filter(|child| child.tag == "document").map(|child| Json::Object(object(child, |name| Some(name)))).collect();
    let mut permissions = Map::new();
    if let Some(element) = app.children.iter().find(|child| child.tag == "permissions") {
        permissions.extend(object(element, |name| match name {
            "filesystem" => Some("fs"),
            "executables" => Some("executables"),
            "environment" => Some("env"),
            _ => None,
        }));
    }
    permissions.insert("native".into(), json!(project.natives));

    json!({
        "schema": HOST_SCHEMA,
        "application": application,
        "forms": forms,
        "menus": natives.menus,
        "trays": natives.trays,
        "timers": natives.timers,
        "actions": natives.actions,
        "documents": documents,
        "permissions": permissions,
    })
}
