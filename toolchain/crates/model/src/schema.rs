//! The element table: every tag a resource may use and where it may sit.
//!
//! Decisions:
//! - Tags are resolved from this table alone. The spec allows only lower-case
//!   element names, so no import is ever followed to resolve one.
//! - Nesting is one rule: a parent states what it accepts ([`Children`]), a child
//!   states where it belongs ([`Place`]). Non-visual components belong directly to
//!   a form, as in Delphi, and `trayicon`/`actionlist` also to the application.
//! - `document` and `permissions` carry no `name`: they are application
//!   configuration, not components a form class reaches.
//! - Every prop in a row can be written by the form class at runtime, and is in
//!   the bridge table (spec 4d). A prop marked runtime-only (`rows`, `nodes`) is
//!   data, not markup: the view resource may not set it (spec 2.4).
//! - The common props (spec 2.4, "every control accepts") apply to the visual
//!   controls. Non-visual components list exactly what they take.
//! - `document` and `permissions` props are not enumerated by the spec; the names
//!   here follow its prose ("file types the application opens"; "filesystem
//!   roots, executables, environment names").
//! - `popupmenu` is also valid under `<application>`: an application-wide
//!   `trayicon` names its menu by `popupMenu`, and that menu has to live somewhere.

/// The type of a prop value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    /// A string.
    Text,
    /// A number.
    Number,
    /// A boolean.
    Bool,
    /// An array of strings.
    Texts,
    /// One of these strings.
    Choice(&'static [&'static str]),
    /// An array of these strings.
    Choices(&'static [&'static str]),
    /// `#rrggbb` or `#rrggbbaa`.
    Color,
    /// The name of an element with this tag in the same resource; `"form"` means
    /// the name of a form of the project.
    Ref(&'static str),
    /// Rows of a `listview`: an array of string arrays. Runtime-only.
    Rows,
    /// Nodes of a `treeview`: a tree of captions. Runtime-only.
    Nodes,
}

/// One prop an element takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropSpec {
    /// The prop name.
    pub name: &'static str,
    /// Its type.
    pub ty: Type,
    /// Whether a view resource may set it; `false` for runtime-only data.
    pub declared: bool,
}

/// Builds a prop a view resource may set.
///
/// # Arguments
/// * `name` - the prop name.
/// * `ty` - its type.
///
/// # Returns
/// The prop.
const fn p(name: &'static str, ty: Type) -> PropSpec {
    PropSpec { name, ty, declared: true }
}

/// Builds a runtime-only prop.
///
/// # Arguments
/// * `name` - the prop name.
/// * `ty` - its type.
///
/// # Returns
/// The prop.
const fn runtime(name: &'static str, ty: Type) -> PropSpec {
    PropSpec { name, ty, declared: false }
}

/// The props every visual control accepts (spec 2.4).
pub const COMMON: &[PropSpec] = &[
    p("caption", Type::Text),
    p("align", Type::Choice(&["none", "top", "bottom", "left", "right", "client"])),
    p("width", Type::Number),
    p("height", Type::Number),
    p("enabled", Type::Bool),
    p("visible", Type::Bool),
    p("hint", Type::Text),
    p("tabOrder", Type::Number),
    p("popupMenu", Type::Ref("popupmenu")),
];

/// Layout props of the container controls.
const LAYOUT: &[PropSpec] =
    &[p("layout", Type::Choice(&["row", "column"])), p("gap", Type::Number), p("padding", Type::Number)];

/// Props of the single-line and multi-line text inputs.
const EDIT: &[PropSpec] =
    &[p("text", Type::Text), p("placeholder", Type::Text), p("readOnly", Type::Bool), p("password", Type::Bool)];

/// Props of `memo`: those of `edit`, plus `wordWrap`.
const MEMO: &[PropSpec] = &[
    p("text", Type::Text),
    p("placeholder", Type::Text),
    p("readOnly", Type::Bool),
    p("password", Type::Bool),
    p("wordWrap", Type::Bool),
];

/// Props of the controls that pick one of a list of strings.
const ITEMS: &[PropSpec] = &[p("items", Type::Texts), p("itemIndex", Type::Number)];

/// Props of the controls that hold a position in a range.
const RANGE: &[PropSpec] = &[p("position", Type::Number), p("min", Type::Number), p("max", Type::Number)];

/// The menu item roles the native host implements.
pub const ROLES: &[&str] = &[
    "about", "quit", "undo", "redo", "cut", "copy", "paste", "selectAll", "minimize", "hide", "hideOthers",
    "showAll", "closeWindow", "fullscreen", "services",
];

/// Props of `<form>`: the native window (spec 2.3).
const FORM: &[PropSpec] = &[
    p("caption", Type::Text),
    p("width", Type::Number),
    p("height", Type::Number),
    p("minWidth", Type::Number),
    p("minHeight", Type::Number),
    p("maxWidth", Type::Number),
    p("maxHeight", Type::Number),
    p("left", Type::Number),
    p("top", Type::Number),
    p("position", Type::Choice(&["designed", "screenCenter", "ownerCenter", "default"])),
    p("borderStyle", Type::Choice(&["sizeable", "single", "dialog", "toolWindow", "none"])),
    p("borderIcons", Type::Choices(&["minimize", "maximize", "close"])),
    p("windowState", Type::Choice(&["normal", "minimized", "maximized", "fullscreen"])),
    p("formStyle", Type::Choice(&["normal", "stayOnTop", "stayOnBottom"])),
    p("visible", Type::Bool),
    p("icon", Type::Text),
    p("showInTaskbar", Type::Bool),
    p("transparent", Type::Bool),
    p("color", Type::Color),
    p("theme", Type::Choice(&["system", "light", "dark"])),
    p("owner", Type::Ref("form")),
    p("modal", Type::Bool),
    p("titleBar", Type::Choice(&["visible", "transparent", "overlay", "hidden"])),
    p("contentProtection", Type::Bool),
    p("allWorkspaces", Type::Bool),
    p("cursor", Type::Text),
    p("menu", Type::Ref("mainmenu")),
];

/// Events of `<form>` (spec 2.3).
const FORM_EVENTS: &[&str] = &[
    "onCreate", "onShow", "onHide", "onCloseQuery", "onClose", "onActivate", "onDeactivate", "onResize", "onMove",
    "onDropFiles", "onThemeChange",
];

/// Props of `<application>` (spec 2.6).
const APPLICATION: &[PropSpec] = &[
    p("id", Type::Text),
    p("name", Type::Text),
    p("version", Type::Text),
    p("icon", Type::Text),
    p("mainForm", Type::Ref("form")),
    p("singleInstance", Type::Bool),
    p("keepAlive", Type::Bool),
    p("urlSchemes", Type::Texts),
    p("activationPolicy", Type::Choice(&["regular", "accessory", "prohibited"])),
    p("theme", Type::Choice(&["system", "light", "dark"])),
];

/// What an element accepts as children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Children {
    /// A leaf: no children at all.
    None,
    /// A layout container: any control placed with [`Place::Layout`], plus the
    /// components that name this tag in [`Place::Under`].
    Controls,
    /// Exactly these tags.
    Only(&'static [&'static str]),
}

/// Where an element may sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// The root of a resource; never a child.
    Root,
    /// Inside any layout container.
    Layout,
    /// Only directly inside one of these tags.
    Under(&'static [&'static str]),
}

/// One row of the element table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kind {
    /// The element name as written in the resource.
    pub tag: &'static str,
    /// What it accepts as children.
    pub children: Children,
    /// Where it may sit.
    pub place: Place,
    /// Whether it must carry a `name` that is unique within its resource.
    pub named: bool,
    /// Whether it accepts the [`COMMON`] props.
    pub common: bool,
    /// Its own props, beyond the common ones.
    pub props: &'static [PropSpec],
    /// Its events.
    pub events: &'static [&'static str],
}

impl Kind {
    /// Finds a prop this element takes.
    ///
    /// # Arguments
    /// * `name` - the prop name.
    ///
    /// # Returns
    /// The prop, or `None` when the element has no such prop.
    pub fn prop(&self, name: &str) -> Option<&'static PropSpec> {
        let common: &'static [PropSpec] = if self.common { COMMON } else { &[] };
        self.props.iter().chain(common).find(|prop| prop.name == name)
    }

    /// Lists every prop this element takes: its own, then the common ones.
    ///
    /// # Returns
    /// The props, in table order.
    pub fn all_props(&self) -> impl Iterator<Item = &'static PropSpec> {
        let common: &'static [PropSpec] = if self.common { COMMON } else { &[] };
        self.props.iter().chain(common)
    }
}

/// Builds the row of a visual control: named, with the common props.
///
/// # Arguments
/// * `tag` - the element name.
/// * `children` - what it accepts.
/// * `place` - where it may sit.
/// * `props` - its own props.
/// * `events` - its events.
///
/// # Returns
/// The row.
const fn visual(
    tag: &'static str,
    children: Children,
    place: Place,
    props: &'static [PropSpec],
    events: &'static [&'static str],
) -> Kind {
    Kind { tag, children, place, named: true, common: true, props, events }
}

/// Builds the row of a non-visual component: named, without the common props.
///
/// # Arguments
/// * `tag` - the element name.
/// * `children` - what it accepts.
/// * `place` - where it may sit.
/// * `props` - its props.
/// * `events` - its events.
///
/// # Returns
/// The row.
const fn component(
    tag: &'static str,
    children: Children,
    place: Place,
    props: &'static [PropSpec],
    events: &'static [&'static str],
) -> Kind {
    Kind { tag, children, place, named: true, common: false, props, events }
}

/// The element table of spec 2.3 (form), 2.4 (controls) and 2.6 (application).
pub const KINDS: &[Kind] = &[
    Kind {
        tag: "application",
        children: Children::Only(&["trayicon", "actionlist", "popupmenu", "document", "permissions"]),
        place: Place::Root,
        named: false,
        common: false,
        props: APPLICATION,
        events: &["onReady", "onActivate", "onOpenFile", "onOpenUrl", "onQuit"],
    },
    component("form", Children::Controls, Place::Root, FORM, FORM_EVENTS),
    // Containers.
    visual("panel", Children::Controls, Place::Layout, LAYOUT, &[]),
    visual("groupbox", Children::Controls, Place::Layout, LAYOUT, &[]),
    visual("scrollbox", Children::Controls, Place::Layout, LAYOUT, &[]),
    visual("pagecontrol", Children::Only(&["tabsheet"]), Place::Layout, &[p("activePage", Type::Ref("tabsheet"))], &["onChange"]),
    visual("tabsheet", Children::Controls, Place::Under(&["pagecontrol"]), LAYOUT, &[]),
    visual("splitter", Children::None, Place::Layout, &[], &[]),
    visual("toolbar", Children::Only(&["button"]), Place::Layout, &[], &[]),
    visual("statusbar", Children::None, Place::Layout, &[], &[]),
    // Display.
    visual("label", Children::None, Place::Layout, &[], &[]),
    visual(
        "image",
        Children::None,
        Place::Layout,
        &[p("src", Type::Text), p("stretch", Type::Bool), p("proportional", Type::Bool)],
        &[],
    ),
    visual("progressbar", Children::None, Place::Layout, RANGE, &[]),
    // Input.
    visual(
        "button",
        Children::None,
        Place::Layout,
        &[p("default", Type::Bool), p("cancel", Type::Bool), p("action", Type::Ref("action"))],
        &["onClick"],
    ),
    visual("edit", Children::None, Place::Layout, EDIT, &["onChange"]),
    visual("memo", Children::None, Place::Layout, MEMO, &["onChange"]),
    visual("checkbox", Children::None, Place::Layout, &[p("checked", Type::Bool)], &["onClick"]),
    visual("radiogroup", Children::None, Place::Layout, ITEMS, &["onClick"]),
    visual("combobox", Children::None, Place::Layout, ITEMS, &["onChange"]),
    visual("trackbar", Children::None, Place::Layout, RANGE, &["onChange"]),
    // Lists.
    visual("listbox", Children::None, Place::Layout, ITEMS, &["onChange"]),
    visual(
        "listview",
        Children::Only(&["column"]),
        Place::Layout,
        &[p("itemIndex", Type::Number), runtime("rows", Type::Rows)],
        &["onChange", "onDblClick"],
    ),
    component(
        "column",
        Children::None,
        Place::Under(&["listview"]),
        &[p("caption", Type::Text), p("width", Type::Number), p("alignment", Type::Choice(&["left", "center", "right"]))],
        &[],
    ),
    visual(
        "treeview",
        Children::None,
        Place::Layout,
        &[runtime("nodes", Type::Nodes), runtime("selected", Type::Text)],
        &["onChange", "onExpanded"],
    ),
    // Non-visual.
    component("mainmenu", Children::Only(&["menuitem"]), Place::Under(&["form"]), &[], &[]),
    component("popupmenu", Children::Only(&["menuitem"]), Place::Under(&["form", "application"]), &[], &[]),
    component(
        "menuitem",
        Children::Only(&["menuitem"]),
        Place::Under(&["mainmenu", "popupmenu", "menuitem"]),
        &[
            p("caption", Type::Text),
            p("shortcut", Type::Text),
            p("checked", Type::Bool),
            p("enabled", Type::Bool),
            p("visible", Type::Bool),
            p("action", Type::Ref("action")),
            p("role", Type::Choice(ROLES)),
        ],
        &["onClick"],
    ),
    component("actionlist", Children::Only(&["action"]), Place::Under(&["form", "application"]), &[], &[]),
    component(
        "action",
        Children::None,
        Place::Under(&["actionlist"]),
        &[p("caption", Type::Text), p("shortcut", Type::Text), p("enabled", Type::Bool), p("checked", Type::Bool)],
        &["onExecute"],
    ),
    component(
        "trayicon",
        Children::None,
        Place::Under(&["form", "application"]),
        &[p("icon", Type::Text), p("hint", Type::Text), p("popupMenu", Type::Ref("popupmenu"))],
        &["onClick"],
    ),
    component(
        "timer",
        Children::None,
        Place::Under(&["form"]),
        &[p("interval", Type::Number), p("enabled", Type::Bool)],
        &["onTimer"],
    ),
    // Application configuration.
    Kind {
        tag: "document",
        children: Children::None,
        place: Place::Under(&["application"]),
        named: false,
        common: false,
        props: &[
            p("name", Type::Text),
            p("extensions", Type::Texts),
            p("mimeTypes", Type::Texts),
            p("role", Type::Choice(&["editor", "viewer"])),
            p("icon", Type::Text),
        ],
        events: &[],
    },
    Kind {
        tag: "permissions",
        children: Children::None,
        place: Place::Under(&["application"]),
        named: false,
        common: false,
        props: &[p("filesystem", Type::Texts), p("executables", Type::Texts), p("environment", Type::Texts)],
        events: &[],
    },
];

/// Names the type a form class uses for an element (spec 2.5: `Button`, `Edit`,
/// `ListBox`, …): the Delphi spelling of the tag.
///
/// # Arguments
/// * `tag` - the element name.
///
/// # Returns
/// The type name, or `None` for a tag without one (`application`, `form`,
/// `document`, `permissions`).
pub fn class_name(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "panel" => "Panel",
        "groupbox" => "GroupBox",
        "scrollbox" => "ScrollBox",
        "pagecontrol" => "PageControl",
        "tabsheet" => "TabSheet",
        "splitter" => "Splitter",
        "toolbar" => "ToolBar",
        "statusbar" => "StatusBar",
        "label" => "Label",
        "image" => "Image",
        "progressbar" => "ProgressBar",
        "button" => "Button",
        "edit" => "Edit",
        "memo" => "Memo",
        "checkbox" => "CheckBox",
        "radiogroup" => "RadioGroup",
        "combobox" => "ComboBox",
        "trackbar" => "TrackBar",
        "listbox" => "ListBox",
        "listview" => "ListView",
        "column" => "Column",
        "treeview" => "TreeView",
        "mainmenu" => "MainMenu",
        "popupmenu" => "PopupMenu",
        "menuitem" => "MenuItem",
        "actionlist" => "ActionList",
        "action" => "Action",
        "trayicon" => "TrayIcon",
        "timer" => "Timer",
        _ => return None,
    })
}

/// Looks a tag up in the element table.
///
/// # Arguments
/// * `tag` - the element name as written.
///
/// # Returns
/// Its row, or `None` for a tag the model does not have.
pub fn lookup(tag: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|kind| kind.tag == tag)
}

/// Decides whether `child` may sit directly inside `parent`.
///
/// # Arguments
/// * `parent` - the containing element's row.
/// * `child` - the contained element's row.
///
/// # Returns
/// `true` when the nesting is valid.
pub fn accepts(parent: &Kind, child: &Kind) -> bool {
    match parent.children {
        Children::None => false,
        Children::Only(tags) => tags.contains(&child.tag),
        Children::Controls => match child.place {
            Place::Root => false,
            Place::Layout => true,
            Place::Under(tags) => tags.contains(&parent.tag),
        },
    }
}
