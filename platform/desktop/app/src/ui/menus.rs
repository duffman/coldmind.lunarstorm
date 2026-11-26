//! Menus (muda) and tray icons (tray-icon), built from the host configuration before any
//! JavaScript runs.
//!
//! Menu ids are `<owner>.<menu>.<item>`, so a click maps back to the owning form and the
//! item's connected events without a lookup table on the native side.

use crate::assets::AssetStore;
use crate::manifest::{ActionConfig, HostConfig, MenuItemConfig};
use muda::{AboutMetadata, CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use serde_json::{Map, Value};
use std::collections::HashMap;
use tao::window::Window;
use tray_icon::{TrayIcon, TrayIconBuilder};

/// A menu item the host can update later.
#[derive(Clone)]
enum Handle {
    /// Plain item.
    Normal(MenuItem),
    /// Checkable item.
    Check(CheckMenuItem),
    /// Submenu.
    Sub(Submenu),
}

/// What a click on an item does.
#[derive(Clone)]
pub struct ItemRef {
    /// Owning form or `@application`.
    pub owner: String,
    /// Item name.
    pub name: String,
    /// Connected events of the item.
    pub events: Vec<String>,
    /// Action the item executes, if any.
    pub action: Option<String>,
    /// Native handle.
    handle: Option<Handle>,
}

/// Every menu and its items.
#[derive(Default)]
pub struct Menus {
    /// Menu name -> menu.
    menus: HashMap<String, Menu>,
    /// Menu id -> item.
    items: HashMap<String, ItemRef>,
    /// (owner, item name) -> menu id.
    names: HashMap<(String, String), String>,
}

impl Menus {
    /// Builds every menu of the configuration.
    ///
    /// # Arguments
    /// * `config` - host configuration.
    ///
    /// # Returns
    /// The menus.
    pub fn build(config: &HostConfig) -> Menus {
        let mut menus = Menus::default();
        for menu in &config.menus {
            let root = Menu::with_id(format!("{}.{}", menu.owner, menu.name));
            for item in &menu.items {
                let built = menus.item(config, &menu.owner, &menu.name, item);
                if let Err(e) = root.append(built.as_ref()) {
                    eprintln!("zebra: menu {}: {e}", menu.name);
                }
            }
            menus.menus.insert(menu.name.clone(), root);
        }
        menus
    }

    /// Builds one item (recursively for submenus) and records it.
    ///
    /// # Arguments
    /// * `config` - host configuration (actions, application identity).
    /// * `owner` - owning form.
    /// * `menu` - menu name.
    /// * `cfg` - the item.
    ///
    /// # Returns
    /// The native item.
    fn item(&mut self, config: &HostConfig, owner: &str, menu: &str, cfg: &MenuItemConfig) -> Box<dyn IsMenuItem> {
        let id = format!("{owner}.{menu}.{}", cfg.name);
        let action: Option<&ActionConfig> = cfg.action.as_ref().and_then(|a| config.actions.iter().find(|x| &x.name == a));
        let caption = if cfg.caption.is_empty() { action.and_then(|a| a.caption.clone()).unwrap_or_default() } else { cfg.caption.clone() };
        let enabled = cfg.enabled.or(action.and_then(|a| a.enabled)).unwrap_or(true);
        let checked = cfg.checked.or(action.and_then(|a| a.checked));
        let accel = cfg.shortcut.as_ref().or(action.and_then(|a| a.shortcut.as_ref())).and_then(|s| match s.parse() {
            Ok(a) => Some(a),
            Err(e) => {
                eprintln!("zebra: shortcut {s} of {}: {e}", cfg.name);
                None
            }
        });
        let mut record = |handle: Option<Handle>| {
            self.names.insert((owner.to_owned(), cfg.name.clone()), id.clone());
            self.items.insert(
                id.clone(),
                ItemRef { owner: owner.to_owned(), name: cfg.name.clone(), events: cfg.events.clone(), action: cfg.action.clone(), handle },
            );
        };
        if caption == "-" {
            return Box::new(PredefinedMenuItem::separator());
        }
        if let Some(role) = &cfg.role {
            let text = (!caption.is_empty()).then_some(caption.as_str());
            let about = AboutMetadata {
                name: Some(config.application.name.clone()),
                version: Some(config.application.version.clone()),
                ..Default::default()
            };
            return Box::new(match role.as_str() {
                "about" => PredefinedMenuItem::about(text, Some(about)),
                "quit" => PredefinedMenuItem::quit(text),
                "copy" => PredefinedMenuItem::copy(text),
                "cut" => PredefinedMenuItem::cut(text),
                "paste" => PredefinedMenuItem::paste(text),
                "selectAll" => PredefinedMenuItem::select_all(text),
                "undo" => PredefinedMenuItem::undo(text),
                "redo" => PredefinedMenuItem::redo(text),
                "minimize" => PredefinedMenuItem::minimize(text),
                "hide" => PredefinedMenuItem::hide(text),
                "hideOthers" => PredefinedMenuItem::hide_others(text),
                "showAll" => PredefinedMenuItem::show_all(text),
                "closeWindow" => PredefinedMenuItem::close_window(text),
                "fullscreen" => PredefinedMenuItem::fullscreen(text),
                "services" => PredefinedMenuItem::services(text),
                other => {
                    eprintln!("zebra: unknown menu role {other}; item {} left out", cfg.name);
                    PredefinedMenuItem::separator()
                }
            });
        }
        if !cfg.items.is_empty() {
            let sub = Submenu::with_id(id.clone(), &caption, enabled);
            for child in &cfg.items {
                let built = self.item(config, owner, menu, child);
                if let Err(e) = sub.append(built.as_ref()) {
                    eprintln!("zebra: menu item {}: {e}", child.name);
                }
            }
            self.names.insert((owner.to_owned(), cfg.name.clone()), id.clone());
            self.items.insert(
                id.clone(),
                ItemRef { owner: owner.to_owned(), name: cfg.name.clone(), events: vec![], action: None, handle: Some(Handle::Sub(sub.clone())) },
            );
            return Box::new(sub);
        }
        match checked {
            Some(checked) => {
                let item = CheckMenuItem::with_id(id.clone(), &caption, enabled, checked, accel);
                record(Some(Handle::Check(item.clone())));
                Box::new(item)
            }
            None => {
                let item = MenuItem::with_id(id.clone(), &caption, enabled, accel);
                record(Some(Handle::Normal(item.clone())));
                Box::new(item)
            }
        }
    }

    /// Looks up a clicked item.
    ///
    /// # Arguments
    /// * `id` - menu id from the event.
    ///
    /// # Returns
    /// The item.
    pub fn clicked(&self, id: &str) -> Option<&ItemRef> {
        self.items.get(id)
    }

    /// Applies property values written by the backend to a menu item.
    ///
    /// # Arguments
    /// * `owner` - owning form.
    /// * `name` - item name.
    /// * `props` - `caption`, `enabled`, `checked`.
    pub fn apply(&self, owner: &str, name: &str, props: &Map<String, Value>) {
        let Some(item) = self.names.get(&(owner.to_owned(), name.to_owned())).and_then(|id| self.items.get(id)) else { return };
        let caption = props.get("caption").and_then(Value::as_str);
        let enabled = props.get("enabled").and_then(Value::as_bool);
        let checked = props.get("checked").and_then(Value::as_bool);
        // Each handle type has its own setters; apply what was written.
        match &item.handle {
            Some(Handle::Normal(i)) => {
                if let Some(c) = caption {
                    i.set_text(c);
                }
                if let Some(e) = enabled {
                    i.set_enabled(e);
                }
            }
            Some(Handle::Check(i)) => {
                if let Some(c) = caption {
                    i.set_text(c);
                }
                if let Some(e) = enabled {
                    i.set_enabled(e);
                }
                if let Some(c) = checked {
                    i.set_checked(c);
                }
            }
            Some(Handle::Sub(i)) => {
                if let Some(c) = caption {
                    i.set_text(c);
                }
                if let Some(e) = enabled {
                    i.set_enabled(e);
                }
            }
            None => {}
        }
    }

    /// A menu by name.
    ///
    /// # Arguments
    /// * `name` - menu name.
    ///
    /// # Returns
    /// The menu.
    pub fn get(&self, name: &str) -> Option<&Menu> {
        self.menus.get(name)
    }

    /// Installs a main menu for a window: the app menu bar on macOS, the window's menu
    /// bar elsewhere.
    ///
    /// # Arguments
    /// * `name` - menu name.
    /// * `window` - the form's window.
    #[allow(unused_variables)]
    pub fn install(&self, name: &str, window: &Window) {
        let Some(menu) = self.menus.get(name) else {
            eprintln!("zebra: menu {name} does not exist");
            return;
        };
        #[cfg(target_os = "macos")]
        menu.init_for_nsapp();
        #[cfg(target_os = "windows")]
        {
            use tao::platform::windows::WindowExtWindows;
            // SAFETY: the HWND belongs to a live window owned by this thread.
            if let Err(e) = unsafe { menu.init_for_hwnd(window.hwnd()) } {
                eprintln!("zebra: menu {name}: {e}");
            }
        }
        #[cfg(target_os = "linux")]
        {
            use tao::platform::unix::WindowExtUnix;
            if let Err(e) = menu.init_for_gtk_window(window.gtk_window(), window.default_vbox()) {
                eprintln!("zebra: menu {name}: {e}");
            }
        }
    }

    /// Shows a popup menu at the cursor over a window.
    ///
    /// # Arguments
    /// * `name` - popup menu name.
    /// * `window` - the window.
    #[allow(unused_variables)]
    pub fn popup(&self, name: &str, window: &Window) {
        use muda::ContextMenu;
        let Some(menu) = self.menus.get(name) else { return };
        #[cfg(target_os = "macos")]
        {
            use tao::platform::macos::WindowExtMacOS;
            // SAFETY: the NSView belongs to a live window on this (main) thread.
            unsafe { menu.show_context_menu_for_nsview(window.ns_view() as _, None) };
        }
        #[cfg(target_os = "windows")]
        {
            use tao::platform::windows::WindowExtWindows;
            // SAFETY: the HWND belongs to a live window owned by this thread.
            unsafe { menu.show_context_menu_for_hwnd(window.hwnd(), None) };
        }
        #[cfg(target_os = "linux")]
        {
            use tao::platform::unix::WindowExtUnix;
            menu.show_context_menu_for_gtk_window(window.gtk_window().as_ref(), None);
        }
    }
}

/// Builds the tray icons.
///
/// # Arguments
/// * `config` - host configuration.
/// * `menus` - built menus (popup menus).
/// * `assets` - asset store (icons).
///
/// # Returns
/// Tray name -> icon.
pub fn trays(config: &HostConfig, menus: &Menus, assets: &AssetStore) -> HashMap<String, TrayIcon> {
    let mut out = HashMap::new();
    for tray in &config.trays {
        let mut builder = TrayIconBuilder::new().with_id(tray.name.clone()).with_menu_on_left_click(false);
        if let Some(hint) = &tray.hint {
            builder = builder.with_tooltip(hint);
        }
        let icon_path = tray.icon.as_deref().or(config.application.icon.as_deref());
        match icon_path.and_then(|p| super::props::rgba(assets, p)).and_then(|(d, w, h)| tray_icon::Icon::from_rgba(d, w, h).ok()) {
            Some(icon) => builder = builder.with_icon(icon),
            None => builder = builder.with_title(&config.application.name),
        }
        if let Some(menu) = tray.popup_menu.as_ref().and_then(|m| menus.get(m)) {
            builder = builder.with_menu(Box::new(menu.clone()));
        }
        match builder.build() {
            Ok(icon) => {
                out.insert(tray.name.clone(), icon);
            }
            Err(e) => eprintln!("zebra: tray {}: {e}", tray.name),
        }
    }
    out
}
