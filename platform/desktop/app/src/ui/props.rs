//! Form properties (spec 2.3) to TAO calls: creation-time ones on the builder, live ones
//! through setters.

use crate::assets::AssetStore;
use crate::manifest::{FormConfig, HostConfig};
use serde_json::{Map, Value};
use tao::dpi::{LogicalPosition, LogicalSize, LogicalUnit, PixelUnit};
use tao::window::{CursorIcon, Fullscreen, Icon, Theme, Window, WindowBuilder, WindowSizeConstraints};

/// Default inner size when the view resource gives none.
const DEFAULT_SIZE: (f64, f64) = (800.0, 600.0);

/// Applies the creation-time properties to a builder.
///
/// # Arguments
/// * `builder` - window builder.
/// * `form` - the form.
/// * `config` - host configuration (application theme).
///
/// # Returns
/// The builder.
pub fn creation(mut builder: WindowBuilder, form: &FormConfig, config: &HostConfig) -> WindowBuilder {
    let width = form.num_prop("width").unwrap_or(DEFAULT_SIZE.0);
    let height = form.num_prop("height").unwrap_or(DEFAULT_SIZE.1);
    builder = builder
        .with_title(form.str_prop("caption").unwrap_or(&form.name))
        .with_inner_size(LogicalSize::new(width, height))
        .with_visible(false);
    if form.str_prop("position") == Some("designed") || (form.props.contains_key("left") && form.str_prop("position").is_none()) {
        let left = form.num_prop("left").unwrap_or(0.0);
        let top = form.num_prop("top").unwrap_or(0.0);
        builder = builder.with_position(LogicalPosition::new(left, top));
    }
    match form.str_prop("windowState") {
        Some("maximized") => builder = builder.with_maximized(true),
        Some("fullscreen") => builder = builder.with_fullscreen(Some(Fullscreen::Borderless(None))),
        _ => {}
    }
    if form.bool_prop("transparent") == Some(true) {
        builder = builder.with_transparent(true);
    }
    let theme = form.str_prop("theme").or(config.application.theme.as_deref());
    builder = builder.with_theme(theme.and_then(parse_theme));
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::WindowBuilderExtMacOS;
        match form.str_prop("titleBar") {
            Some("transparent") => builder = builder.with_titlebar_transparent(true).with_title_hidden(true),
            Some("overlay") => {
                builder = builder.with_titlebar_transparent(true).with_title_hidden(true).with_fullsize_content_view(true)
            }
            Some("hidden") => builder = builder.with_titlebar_hidden(true),
            _ => {}
        }
    }
    builder
}

/// Applies live properties to a window.
///
/// # Arguments
/// * `window` - the window.
/// * `form` - the form (current values).
/// * `only` - the properties to apply; `None` applies every live property present.
/// * `assets` - asset store (icons).
pub fn live(window: &Window, form: &FormConfig, only: Option<&Map<String, Value>>, assets: &AssetStore) {
    let wants = |key: &str| only.map_or(form.props.contains_key(key), |o| o.contains_key(key));
    if wants("caption") {
        window.set_title(form.str_prop("caption").unwrap_or(&form.name));
    }
    if ["minWidth", "minHeight", "maxWidth", "maxHeight"].iter().any(|k| wants(k)) {
        let unit = |key: &str| form.num_prop(key).map(|v| PixelUnit::Logical(LogicalUnit::new(v)));
        window.set_inner_size_constraints(WindowSizeConstraints {
            min_width: unit("minWidth"),
            min_height: unit("minHeight"),
            max_width: unit("maxWidth"),
            max_height: unit("maxHeight"),
        });
    }
    if wants("borderStyle") || wants("borderIcons") {
        let style = form.str_prop("borderStyle").unwrap_or("sizeable");
        let (resizable, decorations, minimizable, maximizable) = match style {
            "single" => (false, true, true, false),
            "dialog" => (false, true, false, false),
            "toolWindow" => (false, true, false, false),
            "none" => (false, false, false, false),
            _ => (true, true, true, true),
        };
        window.set_resizable(resizable);
        window.set_decorations(decorations);
        let icons: Option<Vec<&str>> =
            form.props.get("borderIcons").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect());
        let has = |icon: &str, default: bool| icons.as_ref().map_or(default, |list| list.contains(&icon));
        window.set_minimizable(has("minimize", minimizable));
        window.set_maximizable(has("maximize", maximizable));
        window.set_closable(has("close", true));
    }
    if wants("formStyle") {
        let style = form.str_prop("formStyle").unwrap_or("normal");
        window.set_always_on_top(style == "stayOnTop");
        window.set_always_on_bottom(style == "stayOnBottom");
    }
    if wants("icon") {
        window.set_window_icon(form.str_prop("icon").and_then(|p| icon(assets, p)));
    }
    if wants("showInTaskbar") {
        skip_taskbar(window, form.bool_prop("showInTaskbar") == Some(false));
    }
    if wants("color") {
        window.set_background_color(form.str_prop("color").and_then(parse_color));
    }
    if wants("theme") {
        window.set_theme(form.str_prop("theme").and_then(parse_theme));
    }
    if wants("contentProtection") {
        window.set_content_protection(form.bool_prop("contentProtection") == Some(true));
    }
    if wants("allWorkspaces") {
        window.set_visible_on_all_workspaces(form.bool_prop("allWorkspaces") == Some(true));
    }
    if wants("cursor") {
        window.set_cursor_icon(cursor(form.str_prop("cursor").unwrap_or("default")));
    }
}

/// Places a new window per `position` (`screenCenter`, `ownerCenter`).
///
/// # Arguments
/// * `window` - the new window.
/// * `form` - the form.
/// * `owner` - the owner's window, if any.
pub fn place(window: &Window, form: &FormConfig, owner: Option<&Window>) {
    let size = window.outer_size();
    let area = match (form.str_prop("position"), owner) {
        (Some("ownerCenter"), Some(owner)) => owner.outer_position().ok().map(|p| (p.x, p.y, owner.outer_size())),
        (Some("screenCenter"), _) | (Some("ownerCenter"), None) => window
            .current_monitor()
            .or_else(|| window.primary_monitor())
            .map(|m| (m.position().x, m.position().y, m.size())),
        _ => None,
    };
    if let Some((x, y, outer)) = area {
        let left = x + (outer.width as i32 - size.width as i32) / 2;
        let top = y + (outer.height as i32 - size.height as i32) / 2;
        window.set_outer_position(tao::dpi::PhysicalPosition::new(left, top.max(y)));
    }
}

/// Hides a window from the taskbar where the OS supports it.
///
/// # Arguments
/// * `window` - the window.
/// * `skip` - hide from the taskbar.
#[allow(unused_variables)]
fn skip_taskbar(window: &Window, skip: bool) {
    #[cfg(target_os = "windows")]
    {
        use tao::platform::windows::WindowExtWindows;
        let _ = window.set_skip_taskbar(skip);
    }
    #[cfg(target_os = "linux")]
    {
        use tao::platform::unix::WindowExtUnix;
        let _ = window.set_skip_taskbar(skip);
    }
    // macOS has no per-window taskbar entry; the Dock entry follows `activationPolicy`.
}

/// Parses `system`, `light`, `dark`.
///
/// # Arguments
/// * `value` - theme name.
///
/// # Returns
/// The theme; `None` follows the system.
pub fn parse_theme(value: &str) -> Option<Theme> {
    match value {
        "light" => Some(Theme::Light),
        "dark" => Some(Theme::Dark),
        _ => None,
    }
}

/// Parses `#rrggbb` or `#rrggbbaa`.
///
/// # Arguments
/// * `value` - colour text.
///
/// # Returns
/// The colour, or `None` when malformed.
pub fn parse_color(value: &str) -> Option<(u8, u8, u8, u8)> {
    let hex = value.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        6 => Some((byte(0)?, byte(2)?, byte(4)?, 255)),
        8 => Some((byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => None,
    }
}

/// Maps a CSS cursor name.
///
/// # Arguments
/// * `name` - CSS cursor name.
///
/// # Returns
/// The TAO cursor; unknown names give the default.
fn cursor(name: &str) -> CursorIcon {
    match name {
        "pointer" => CursorIcon::Hand,
        "text" => CursorIcon::Text,
        "wait" => CursorIcon::Wait,
        "progress" => CursorIcon::Progress,
        "crosshair" => CursorIcon::Crosshair,
        "move" => CursorIcon::Move,
        "help" => CursorIcon::Help,
        "not-allowed" => CursorIcon::NotAllowed,
        "grab" => CursorIcon::Grab,
        "grabbing" => CursorIcon::Grabbing,
        "ew-resize" | "col-resize" => CursorIcon::EwResize,
        "ns-resize" | "row-resize" => CursorIcon::NsResize,
        "nesw-resize" => CursorIcon::NeswResize,
        "nwse-resize" => CursorIcon::NwseResize,
        _ => CursorIcon::Default,
    }
}

/// Decodes a PNG asset to RGBA.
///
/// # Arguments
/// * `assets` - asset store.
/// * `path` - asset path.
///
/// # Returns
/// `(rgba, width, height)`, or `None` when missing or not an image.
pub fn rgba(assets: &AssetStore, path: &str) -> Option<(Vec<u8>, u32, u32)> {
    let bytes = assets.get(path.trim_start_matches('/'))?;
    let image = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (w, h) = image.dimensions();
    Some((image.into_raw(), w, h))
}

/// Loads a window icon.
///
/// # Arguments
/// * `assets` - asset store.
/// * `path` - asset path.
///
/// # Returns
/// The icon, if it decodes.
pub fn icon(assets: &AssetStore, path: &str) -> Option<Icon> {
    let (data, w, h) = rgba(assets, path)?;
    Icon::from_rgba(data, w, h).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_and_themes() {
        assert_eq!(parse_color("#112233"), Some((0x11, 0x22, 0x33, 255)));
        assert_eq!(parse_color("#11223344"), Some((0x11, 0x22, 0x33, 0x44)));
        assert_eq!(parse_color("112233"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_theme("dark"), Some(Theme::Dark));
        assert_eq!(parse_theme("system"), None);
    }
}
