//! Host configuration (spec 4e): the application, every form as a window, menus, tray
//! icons, timers, actions, documents and permissions, plus each form's property and event
//! table (4d). The pipeline writes it as `zebra.json`; both runtimes read it before any
//! JavaScript runs.
//!
//! Decision: the table is part of the host configuration, not a separate file, so one
//! read gives the host everything it validates messages against.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Schema of `zebra.json` this host understands. A newer schema is refused at load.
pub const HOST_SCHEMA: u32 = 1;

/// Window id used for application-level envelopes (application events, actions).
pub const APPLICATION_ID: &str = "@application";

/// The whole host configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostConfig {
    /// Schema number; see [`HOST_SCHEMA`].
    pub schema: u32,
    /// The `<application>` element (2.6).
    pub application: Application,
    /// One entry per form (2.3); each becomes one window.
    pub forms: Vec<FormConfig>,
    /// Main and popup menus of every form and of the application.
    #[serde(default)]
    pub menus: Vec<MenuConfig>,
    /// Tray icons.
    #[serde(default)]
    pub trays: Vec<TrayConfig>,
    /// `timer` components; the host owns their clocks.
    #[serde(default)]
    pub timers: Vec<TimerConfig>,
    /// `action` components, referenced by menu items.
    #[serde(default)]
    pub actions: Vec<ActionConfig>,
    /// `<document>` elements: file types the application opens.
    #[serde(default)]
    pub documents: Vec<Value>,
    /// The `<permissions>` element plus the derived native API list.
    #[serde(default)]
    pub permissions: Permissions,
    /// Execution limits of the JS thread.
    #[serde(default)]
    pub limits: Limits,
}

/// The `<application>` element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Application {
    /// Reverse-DNS id; bundle id on macOS.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Version string.
    #[serde(default)]
    pub version: String,
    /// Project-relative icon path (PNG in the asset store).
    #[serde(default)]
    pub icon: Option<String>,
    /// Name of the form shown at startup.
    pub main_form: String,
    /// Refuse a second running instance.
    #[serde(default)]
    pub single_instance: bool,
    /// Keep running when the last window closes.
    #[serde(default)]
    pub keep_alive: bool,
    /// URL schemes the application handles.
    #[serde(default)]
    pub url_schemes: Vec<String>,
    /// macOS: `regular`, `accessory`, `prohibited`.
    #[serde(default)]
    pub activation_policy: Option<String>,
    /// Default colour scheme of every form.
    #[serde(default)]
    pub theme: Option<String>,
    /// Application events the model connected (`onReady`, `onQuit`, ...).
    #[serde(default)]
    pub events: Vec<String>,
}

/// One form: its window properties, connected form events and control table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormConfig {
    /// Form name; also the window id on the wire.
    pub name: String,
    /// Form properties of 2.3, literal values from the view resource.
    #[serde(default)]
    pub props: Map<String, Value>,
    /// Form events the view resource connected (`onCloseQuery`, `onResize`, ...).
    #[serde(default)]
    pub events: Vec<String>,
    /// Property and event table of every control in the form (4d).
    #[serde(default)]
    pub controls: BTreeMap<String, ControlEntry>,
}

/// One control's row in the shared property and event table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlEntry {
    /// Element name (`button`, `edit`, `menuitem`, ...).
    #[serde(rename = "type")]
    pub kind: String,
    /// Properties either side may carry for this control.
    #[serde(default)]
    pub props: Vec<String>,
    /// Events the WebView (or host, for non-visual controls) may raise.
    #[serde(default)]
    pub events: Vec<String>,
    /// Name of the control's `popupmenu`. The host shows it when the WebView raises
    /// `onContextPopup` for this control; that event never reaches the backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub popup_menu: Option<String>,
}

/// A `mainmenu` or `popupmenu`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuConfig {
    /// Component name.
    pub name: String,
    /// Owning form, or [`APPLICATION_ID`].
    pub owner: String,
    /// `main` or `popup`.
    pub kind: String,
    /// Top-level items.
    #[serde(default)]
    pub items: Vec<MenuItemConfig>,
}

/// A `menuitem`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MenuItemConfig {
    /// Component name; unique within the owner.
    pub name: String,
    /// Text; `-` is a separator.
    #[serde(default)]
    pub caption: String,
    /// Accelerator, e.g. `CmdOrCtrl+S`.
    #[serde(default)]
    pub shortcut: Option<String>,
    /// Check mark; a checkable item when present.
    #[serde(default)]
    pub checked: Option<bool>,
    /// Enabled state (default true).
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Name of an `action` this item executes.
    #[serde(default)]
    pub action: Option<String>,
    /// Predefined OS item (`quit`, `copy`, `about`, ...).
    #[serde(default)]
    pub role: Option<String>,
    /// Connected events (`onClick`).
    #[serde(default)]
    pub events: Vec<String>,
    /// Sub-items; an item with sub-items is a submenu.
    #[serde(default)]
    pub items: Vec<MenuItemConfig>,
}

/// A `trayicon`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayConfig {
    /// Component name.
    pub name: String,
    /// Owning form, or [`APPLICATION_ID`].
    pub owner: String,
    /// Asset path of the PNG icon.
    #[serde(default)]
    pub icon: Option<String>,
    /// Tooltip.
    #[serde(default)]
    pub hint: Option<String>,
    /// Name of a popup menu.
    #[serde(default)]
    pub popup_menu: Option<String>,
    /// Connected events (`onClick`).
    #[serde(default)]
    pub events: Vec<String>,
}

/// A `timer`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimerConfig {
    /// Component name.
    pub name: String,
    /// Owning form, or [`APPLICATION_ID`].
    pub owner: String,
    /// Interval in milliseconds.
    pub interval: u64,
    /// Running state (default true).
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// An `action`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActionConfig {
    /// Component name.
    pub name: String,
    /// Owning form, or [`APPLICATION_ID`].
    pub owner: String,
    /// Caption shown by items that use the action.
    #[serde(default)]
    pub caption: Option<String>,
    /// Accelerator.
    #[serde(default)]
    pub shortcut: Option<String>,
    /// Enabled state.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Check state.
    #[serde(default)]
    pub checked: Option<bool>,
}

/// The `<permissions>` element and the native methods the form classes reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    /// Filesystem roots.
    #[serde(default)]
    pub fs: Vec<String>,
    /// Executables `process` may start (file names or absolute paths).
    #[serde(default)]
    pub executables: Vec<String>,
    /// Environment variable names `process.env` may read.
    #[serde(default)]
    pub env: Vec<String>,
    /// `namespace.method` names derived from the form classes; `namespace.*` allows a namespace.
    #[serde(default)]
    pub native: Vec<String>,
    /// HostApi areas the WebView may call through the Runtime, e.g. `["fs"]`. `fs`
    /// is scoped to the `fs` roots above.
    #[serde(default)]
    pub hostapi: Vec<String>,
}

/// Execution limits of the JS thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// Wall-clock budget of one dispatched message, in milliseconds.
    #[serde(default = "default_budget")]
    pub budget_ms: u64,
    /// Heap limit in MiB.
    #[serde(default = "default_heap")]
    pub heap_mb: u64,
    /// Deadline of a host request (`onCloseQuery`), in milliseconds.
    #[serde(default = "default_deadline")]
    pub request_deadline_ms: u64,
}

impl Default for Limits {
    /// The defaults used when `zebra.json` has no `limits`.
    ///
    /// # Returns
    /// 5 s budget, 256 MiB heap, 3 s request deadline.
    fn default() -> Self {
        Limits { budget_ms: default_budget(), heap_mb: default_heap(), request_deadline_ms: default_deadline() }
    }
}

/// `true`, for serde defaults.
///
/// # Returns
/// `true`.
fn yes() -> bool {
    true
}

/// Default per-message budget.
///
/// # Returns
/// 5000 ms.
fn default_budget() -> u64 {
    5000
}

/// Default heap limit.
///
/// # Returns
/// 256 MiB.
fn default_heap() -> u64 {
    256
}

/// Default request deadline.
///
/// # Returns
/// 3000 ms.
fn default_deadline() -> u64 {
    3000
}

/// Form properties read only when the window is created (2.3: size, position and
/// `windowState` are read once; the others have no setter after creation). A change to
/// one of these recreates the window in dev (6.1).
pub const CREATION_PROPS: &[&str] = &[
    "width", "height", "left", "top", "position", "windowState", "transparent", "owner", "titleBar",
];

impl HostConfig {
    /// Parses and checks `zebra.json`.
    ///
    /// # Arguments
    /// * `bytes` - the file contents.
    ///
    /// # Returns
    /// The configuration.
    ///
    /// # Errors
    /// Invalid JSON, a newer schema, a missing main form, a duplicate form name, or a
    /// `permissions.hostapi` area this host does not offer.
    pub fn parse(bytes: &[u8]) -> Result<HostConfig, String> {
        let config: HostConfig = serde_json::from_slice(bytes).map_err(|e| format!("zebra.json: {e}"))?;
        if config.schema > HOST_SCHEMA {
            return Err(format!("zebra.json: schema {} is newer than this host ({HOST_SCHEMA})", config.schema));
        }
        let mut names = std::collections::BTreeSet::new();
        for form in &config.forms {
            if !names.insert(form.name.as_str()) || form.name == APPLICATION_ID {
                return Err(format!("zebra.json: duplicate or reserved form name {}", form.name));
            }
        }
        if config.form(&config.application.main_form).is_none() {
            return Err(format!("zebra.json: main form {} is not a form", config.application.main_form));
        }
        // Decision: an unknown area is an error at load, not a call refused later, so
        // a typo in the permission list cannot pass unnoticed.
        if let Some(area) = config.permissions.hostapi.iter().find(|area| !crate::envelope::is_area(area)) {
            return Err(format!("zebra.json: permissions.hostapi: no area {area}"));
        }
        Ok(config)
    }

    /// Looks up a form by name.
    ///
    /// # Arguments
    /// * `name` - form name.
    ///
    /// # Returns
    /// The form, if present.
    pub fn form(&self, name: &str) -> Option<&FormConfig> {
        self.forms.iter().find(|f| f.name == name)
    }

    /// Whether the backend may call `namespace.method`.
    ///
    /// # Arguments
    /// * `ns` - service namespace.
    /// * `method` - method name.
    ///
    /// # Returns
    /// `true` when listed exactly or by `namespace.*`.
    pub fn allows_native(&self, ns: &str, method: &str) -> bool {
        let exact = format!("{ns}.{method}");
        let wildcard = format!("{ns}.*");
        self.permissions.native.iter().any(|p| *p == exact || *p == wildcard)
    }
}

impl FormConfig {
    /// A string form property.
    ///
    /// # Arguments
    /// * `key` - property name.
    ///
    /// # Returns
    /// The value when it is a string.
    pub fn str_prop(&self, key: &str) -> Option<&str> {
        self.props.get(key).and_then(Value::as_str)
    }

    /// A numeric form property.
    ///
    /// # Arguments
    /// * `key` - property name.
    ///
    /// # Returns
    /// The value when it is a number.
    pub fn num_prop(&self, key: &str) -> Option<f64> {
        self.props.get(key).and_then(Value::as_f64)
    }

    /// A boolean form property.
    ///
    /// # Arguments
    /// * `key` - property name.
    ///
    /// # Returns
    /// The value when it is a boolean.
    pub fn bool_prop(&self, key: &str) -> Option<bool> {
        self.props.get(key).and_then(Value::as_bool)
    }

    /// Whether the view resource connected a form event.
    ///
    /// # Arguments
    /// * `event` - event name, e.g. `onCloseQuery`.
    ///
    /// # Returns
    /// `true` when connected.
    pub fn handles(&self, event: &str) -> bool {
        self.events.iter().any(|e| e == event)
    }
}
