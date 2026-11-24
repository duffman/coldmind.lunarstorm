//! Native services that must run on the UI thread: app, window, screen, menu, tray,
//! dialog. The first argument of every `window.*` call is the form name.

use super::{form_of, App, UserEvent};
use crate::runtime::NativeCall;
use crate::native::arg_str;
use serde_json::{json, Map, Value};
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event_loop::EventLoopWindowTarget;
use tao::monitor::MonitorHandle;
use tao::window::Fullscreen;

impl App {
    /// Runs one UI-thread native call and completes it.
    ///
    /// # Arguments
    /// * `call` - the call.
    /// * `target` - event loop target.
    pub(super) fn native_ui(&mut self, call: NativeCall, target: &EventLoopWindowTarget<UserEvent>) {
        let NativeCall { ns, method, args, done } = call;
        // A call from a retired backend has no one waiting for it.
        if done.epoch != self.epoch {
            return;
        }
        let result = match ns.as_str() {
            "app" => self.app_call(&method, target),
            "window" => self.window_call(&method, &args, target),
            "screen" => screen_call(&method, target),
            "menu" => self.menu_call(&method, &args),
            "tray" => self.tray_call(&method, &args),
            "dialog" => self.dialog_call(&method, &args),
            other => Err(format!("unknown native namespace {other}")),
        };
        done.complete(result);
    }

    /// `app.*`: `quit`, `name`, `version`, `hide` (macOS).
    ///
    /// # Arguments
    /// * `method` - method name.
    /// * `target` - event loop target.
    ///
    /// # Returns
    /// The result.
    ///
    /// # Errors
    /// Unknown method.
    #[allow(unused_variables)]
    fn app_call(&mut self, method: &str, target: &EventLoopWindowTarget<UserEvent>) -> Result<Value, String> {
        match method {
            "quit" => {
                self.quit();
                Ok(Value::Null)
            }
            "name" => Ok(json!(self.config.application.name)),
            "version" => Ok(json!(self.config.application.version)),
            "hide" => {
                #[cfg(target_os = "macos")]
                {
                    use tao::platform::macos::EventLoopWindowTargetExtMacOS;
                    target.hide_application();
                }
                Ok(Value::Null)
            }
            other => Err(format!("unknown method app.{other}")),
        }
    }

    /// `window.*(form, ...)`.
    ///
    /// # Arguments
    /// * `method` - method name.
    /// * `args` - arguments; the first is the form name.
    /// * `target` - event loop target.
    ///
    /// # Returns
    /// The result.
    ///
    /// # Errors
    /// Unknown form or method, bad arguments, or a form that is not open.
    fn window_call(&mut self, method: &str, args: &[Value], target: &EventLoopWindowTarget<UserEvent>) -> Result<Value, String> {
        let form = arg_str(args, 0)?.to_owned();
        form_of(&self.config, &form)?;
        match method {
            "open" => return self.open_form(&form, target).map(|_| Value::Null),
            "close" => {
                self.request_close(&form);
                return Ok(Value::Null);
            }
            _ => {}
        }
        let window = &self.windows.get(&form).ok_or_else(|| format!("form {form} is not open"))?.window;
        let num = |n: usize| args.get(n).and_then(Value::as_f64).ok_or_else(|| format!("argument {} must be a number", n + 1));
        let flag = |n: usize| args.get(n).and_then(Value::as_bool).ok_or_else(|| format!("argument {} must be a boolean", n + 1));
        match method {
            "show" | "hide" => {
                let visible = method == "show";
                window.set_visible(visible);
                self.form_event(&form, if visible { "onShow" } else { "onHide" }, json!({}));
            }
            "focus" => window.set_focus(),
            "minimize" => window.set_minimized(true),
            "maximize" => window.set_maximized(true),
            "restore" => {
                window.set_minimized(false);
                window.set_maximized(false);
            }
            "setTitle" => window.set_title(arg_str(args, 1)?),
            "setSize" => window.set_inner_size(LogicalSize::new(num(1)?, num(2)?)),
            "setPosition" => window.set_outer_position(LogicalPosition::new(num(1)?, num(2)?)),
            "setAlwaysOnTop" => window.set_always_on_top(flag(1)?),
            "setFullscreen" => window.set_fullscreen(flag(1)?.then_some(Fullscreen::Borderless(None))),
            "center" => {
                let mut centered = self.config.form(&form).cloned().ok_or("form vanished")?;
                centered.props.insert("position".into(), json!("screenCenter"));
                super::props::place(window, &centered, None);
            }
            "bounds" => {
                let scale = window.scale_factor();
                let pos = window.outer_position().map(|p| p.to_logical::<f64>(scale)).unwrap_or(LogicalPosition::new(0.0, 0.0));
                let size = window.inner_size().to_logical::<f64>(scale);
                return Ok(json!({ "left": pos.x, "top": pos.y, "width": size.width, "height": size.height }));
            }
            "isVisible" => return Ok(json!(window.is_visible())),
            other => return Err(format!("unknown method window.{other}")),
        }
        Ok(Value::Null)
    }

    /// `menu.setEnabled|setChecked|setCaption(owner, item, value)`.
    ///
    /// # Arguments
    /// * `method` - method name.
    /// * `args` - arguments.
    ///
    /// # Returns
    /// `null`.
    ///
    /// # Errors
    /// Unknown method or bad arguments.
    fn menu_call(&mut self, method: &str, args: &[Value]) -> Result<Value, String> {
        let (owner, item) = (arg_str(args, 0)?, arg_str(args, 1)?);
        let value = args.get(2).cloned().ok_or("argument 3 is required")?;
        let key = match method {
            "setEnabled" => "enabled",
            "setChecked" => "checked",
            "setCaption" => "caption",
            other => return Err(format!("unknown method menu.{other}")),
        };
        let mut props = Map::new();
        props.insert(key.into(), value);
        self.menus.apply(owner, item, &props);
        Ok(Value::Null)
    }

    /// `tray.setHint|setVisible(name, value)`.
    ///
    /// # Arguments
    /// * `method` - method name.
    /// * `args` - arguments.
    ///
    /// # Returns
    /// `null`.
    ///
    /// # Errors
    /// Unknown method or tray, bad arguments.
    fn tray_call(&mut self, method: &str, args: &[Value]) -> Result<Value, String> {
        let name = arg_str(args, 0)?.to_owned();
        if !self.trays.contains_key(&name) {
            return Err(format!("no tray icon named {name}"));
        }
        let key = match method {
            "setHint" => "hint",
            "setVisible" => "visible",
            other => return Err(format!("unknown method tray.{other}")),
        };
        let mut props = Map::new();
        props.insert(key.into(), args.get(1).cloned().ok_or("argument 2 is required")?);
        self.apply_tray(&name, &props);
        Ok(Value::Null)
    }

    /// `dialog.openFile(options)`, `dialog.saveFile(options)`, `dialog.message(options)`.
    ///
    /// Options: `title`, `defaultPath`, `filters: [{name, extensions}]`, `multiple`,
    /// `directory`, `defaultName`; for `message`: `text`, `level` (`info`, `warning`,
    /// `error`), `buttons` (`ok`, `okCancel`, `yesNo`), `form` (parent window).
    ///
    /// Decision: dialogs are modal and run on the UI thread; the event loop waits for
    /// them, which is what a modal dialog means.
    ///
    /// # Arguments
    /// * `method` - method name.
    /// * `args` - arguments.
    ///
    /// # Returns
    /// Paths (or `null` when cancelled), or the button result.
    ///
    /// # Errors
    /// Unknown method.
    fn dialog_call(&mut self, method: &str, args: &[Value]) -> Result<Value, String> {
        let options = args.first().cloned().unwrap_or(json!({}));
        let text = |key: &str| options.get(key).and_then(Value::as_str).map(str::to_owned);
        let parent = text("form").and_then(|f| self.windows.get(&f)).map(|fw| &fw.window);
        match method {
            "openFile" | "saveFile" => {
                let mut dialog = rfd::FileDialog::new();
                if let Some(title) = text("title") {
                    dialog = dialog.set_title(title);
                }
                if let Some(path) = text("defaultPath") {
                    dialog = dialog.set_directory(path);
                }
                if let Some(name) = text("defaultName") {
                    dialog = dialog.set_file_name(name);
                }
                for filter in options.get("filters").and_then(Value::as_array).into_iter().flatten() {
                    let name = filter.get("name").and_then(Value::as_str).unwrap_or("Files");
                    let exts: Vec<&str> =
                        filter.get("extensions").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
                    dialog = dialog.add_filter(name, &exts);
                }
                if let Some(parent) = parent {
                    dialog = dialog.set_parent(parent);
                }
                let path = |p: std::path::PathBuf| Value::String(p.to_string_lossy().into_owned());
                let flag = |k: &str| options.get(k).and_then(Value::as_bool) == Some(true);
                Ok(match (method, flag("directory"), flag("multiple")) {
                    ("saveFile", _, _) => dialog.save_file().map(path).unwrap_or(Value::Null),
                    (_, true, true) => dialog.pick_folders().map(|v| Value::Array(v.into_iter().map(path).collect())).unwrap_or(Value::Null),
                    (_, true, false) => dialog.pick_folder().map(path).unwrap_or(Value::Null),
                    (_, false, true) => dialog.pick_files().map(|v| Value::Array(v.into_iter().map(path).collect())).unwrap_or(Value::Null),
                    _ => dialog.pick_file().map(path).unwrap_or(Value::Null),
                })
            }
            "message" => {
                let level = match text("level").as_deref() {
                    Some("warning") => rfd::MessageLevel::Warning,
                    Some("error") => rfd::MessageLevel::Error,
                    _ => rfd::MessageLevel::Info,
                };
                let buttons = match text("buttons").as_deref() {
                    Some("okCancel") => rfd::MessageButtons::OkCancel,
                    Some("yesNo") => rfd::MessageButtons::YesNo,
                    _ => rfd::MessageButtons::Ok,
                };
                let mut dialog = rfd::MessageDialog::new()
                    .set_level(level)
                    .set_title(text("title").unwrap_or_else(|| self.config.application.name.clone()))
                    .set_description(text("text").unwrap_or_default())
                    .set_buttons(buttons);
                if let Some(parent) = parent {
                    dialog = dialog.set_parent(parent);
                }
                Ok(match dialog.show() {
                    rfd::MessageDialogResult::Ok => json!("ok"),
                    rfd::MessageDialogResult::Cancel => json!("cancel"),
                    rfd::MessageDialogResult::Yes => json!("yes"),
                    rfd::MessageDialogResult::No => json!("no"),
                    rfd::MessageDialogResult::Custom(label) => json!(label),
                })
            }
            other => Err(format!("unknown method dialog.{other}")),
        }
    }
}

/// `screen.all()` / `screen.primary()`.
///
/// # Arguments
/// * `method` - method name.
/// * `target` - event loop target.
///
/// # Returns
/// Monitor descriptions in logical pixels.
///
/// # Errors
/// Unknown method.
fn screen_call(method: &str, target: &EventLoopWindowTarget<UserEvent>) -> Result<Value, String> {
    let describe = |m: MonitorHandle| {
        let scale = m.scale_factor();
        let pos = m.position().to_logical::<f64>(scale);
        let size = m.size().to_logical::<f64>(scale);
        json!({ "name": m.name(), "left": pos.x, "top": pos.y, "width": size.width, "height": size.height, "scale": scale })
    };
    match method {
        "all" => Ok(Value::Array(target.available_monitors().map(describe).collect())),
        "primary" => Ok(target.primary_monitor().map(describe).unwrap_or(Value::Null)),
        other => Err(format!("unknown method screen.{other}")),
    }
}
