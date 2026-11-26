//! UI thread (spec 6): TAO event loop -> window manager -> WRY WebViews, plus the menus,
//! tray icons and timers of the host configuration.
//!
//! Startup order: read manifest (the caller) -> create the main form's window, menus and
//! tray -> start the Runtime -> load the backend -> mount the WebView.
//!
//! The UI thread is the only place that routes messages: WebView -> validate -> JS thread,
//! JS thread -> validate -> WebView or native setter. Every hop uses the one envelope.

mod menus;
pub mod props;
mod services;

use crate::assets::AssetStore;
use crate::changes::classify;
use crate::envelope::{hostapi_boot, validate_inbound, validate_update, Envelope, Inbound, Kind, Mirror, Reject};
use crate::runtime::{self, AppRuntime, HostMessage, NativeCall, RuntimeContext, RuntimeMessage};
use crate::manifest::{FormConfig, HostConfig, APPLICATION_ID};
use crate::native::NativeServices;
use crate::pipeline::{BuildOutput, Diagnostic};
use menus::Menus;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder, WindowId};
use tray_icon::TrayIcon;
use wry::{DragDropEvent, NewWindowResponse, WebView, WebViewBuilder};

/// Shell runtime injected into every WebView.
pub const SHELL: &str = include_str!("../shell/shell.js");

/// How long a WebView's HostApi call may wait for its `result` before the host
/// answers `ETIMEDOUT`.
///
/// Decision: one fixed deadline, long enough for file and dialog work, so a backend
/// that never answers cannot leave a page waiting forever.
const CALL_DEADLINE: Duration = Duration::from_secs(30);

/// Which runtime is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Debug runtime: devtools, dev overlay, embedded pipeline.
    Debug,
    /// Release runtime: attached archive, no dev messages.
    Release,
}

/// Everything that wakes the UI thread besides OS events.
#[derive(Debug)]
pub enum UserEvent {
    /// A message from the JS thread.
    Js(RuntimeMessage),
    /// A native call that must run on the UI thread.
    Native(NativeCall),
    /// A raw message from a form's WebView.
    Ipc {
        /// Form whose WebView sent it.
        form: String,
        /// Message text.
        body: String,
    },
    /// Files dropped on a form.
    Drop {
        /// Form.
        form: String,
        /// Paths.
        paths: Vec<PathBuf>,
    },
    /// A menu item was chosen.
    Menu(muda::MenuEvent),
    /// A tray icon was clicked.
    Tray(tray_icon::TrayIconEvent),
    /// The embedded pipeline finished a build (debug runtime).
    Build(Box<Result<BuildOutput, Vec<Diagnostic>>>),
}

/// Starts the dev machinery once the event loop exists (debug runtime).
pub type DevStarter = Box<dyn FnOnce(EventLoopProxy<UserEvent>)>;

/// One form's native window.
struct FormWindow {
    /// The WebView; declared first so it is destroyed before its window.
    webview: Option<WebView>,
    /// The window.
    window: Window,
    /// The shell has said hello; updates may be delivered.
    ready: bool,
}

/// A host request waiting for the backend.
struct Pending {
    /// Form the request is for.
    form: String,
    /// When the host stops waiting.
    deadline: Instant,
}

/// A host-owned `timer` component.
struct HostTimer {
    /// Owning form.
    owner: String,
    /// Component name.
    name: String,
    /// Period.
    every: Duration,
    /// Running.
    enabled: bool,
    /// Next due time.
    next: Instant,
}

/// The UI-thread state.
struct App {
    /// Runtime mode.
    mode: Mode,
    /// The build being run.
    build: BuildOutput,
    /// Its host configuration.
    config: Arc<HostConfig>,
    /// The same, shared with native services.
    shared: Arc<RwLock<Arc<HostConfig>>>,
    /// Frontend files.
    assets: AssetStore,
    /// Custom protocol origin.
    origin: String,
    /// Open forms.
    windows: BTreeMap<String, FormWindow>,
    /// Window id -> form.
    ids: HashMap<WindowId, String>,
    /// Owner -> modal form blocking it.
    modal: HashMap<String, String>,
    /// App runtime selected at build time (`runtime::create`); declared after `windows`
    /// so WebViews close first at shutdown.
    runtime: Box<dyn AppRuntime>,
    /// Current backend epoch.
    epoch: u64,
    /// The current epoch's backend loaded without error.
    loaded: bool,
    /// The main form has been mounted.
    started: bool,
    /// Property mirror.
    mirror: Mirror,
    /// Host requests in flight.
    pending: HashMap<u64, Pending>,
    /// WebView HostApi calls in flight: (form, call id) -> deadline. A `result` is
    /// delivered only while its call is here.
    calls: HashMap<(String, u64), Instant>,
    /// Next request id.
    next_request: u64,
    /// Menus.
    menus: Menus,
    /// Tray icons.
    trays: HashMap<String, TrayIcon>,
    /// Timers.
    timers: Vec<HostTimer>,
    /// Current compile errors (debug).
    errors: Option<Vec<Diagnostic>>,
    /// Event loop proxy.
    proxy: EventLoopProxy<UserEvent>,
    /// Quit at the end of this iteration.
    exit: bool,
}

/// Runs the native desktop process until the application quits.
///
/// # Arguments
/// * `build` - the build to run (from the pipeline or the attached archive).
/// * `mode` - debug or release.
/// * `dev` - starts the watcher and compiler threads (debug runtime).
///
/// # Returns
/// Never; the process exits when the event loop ends.
pub fn run(build: BuildOutput, mode: Mode, dev: Option<DevStarter>) -> ! {
    if build.config.application.single_instance && !single_instance(&build.config.application.id) {
        eprintln!("zebra: {} is already running", build.config.application.name);
        std::process::exit(0);
    }
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        let policy = match build.config.application.activation_policy.as_deref() {
            Some("accessory") => ActivationPolicy::Accessory,
            Some("prohibited") => ActivationPolicy::Prohibited,
            _ => ActivationPolicy::Regular,
        };
        event_loop.set_activation_policy(policy);
    }
    let proxy = event_loop.create_proxy();
    let p = proxy.clone();
    muda::MenuEvent::set_event_handler(Some(move |e| {
        let _ = p.send_event(UserEvent::Menu(e));
    }));
    let p = proxy.clone();
    tray_icon::TrayIconEvent::set_event_handler(Some(move |e| {
        let _ = p.send_event(UserEvent::Tray(e));
    }));
    let mut seed = Some(build);
    let mut dev = dev;
    let mut app: Option<App> = None;
    event_loop.run(move |event, target, flow| {
        match event {
            Event::NewEvents(StartCause::Init) => {
                let build = seed.take().expect("init runs once");
                app = Some(App::start(build, mode, proxy.clone(), target));
                if let Some(start) = dev.take() {
                    start(proxy.clone());
                }
            }
            // Dropping the state closes WebViews and windows, then joins the JS thread
            // after it has processed onClose/onQuit.
            Event::LoopDestroyed => drop(app.take()),
            other => {
                if let Some(app) = app.as_mut() {
                    app.handle(other, target);
                }
            }
        }
        if let Some(app) = app.as_mut() {
            app.tick();
            *flow = app.control_flow();
        }
    })
}

/// Holds the single-instance lock for the life of the process.
///
/// # Arguments
/// * `id` - application id.
///
/// # Returns
/// `false` when another instance holds the lock.
fn single_instance(id: &str) -> bool {
    let path = std::env::temp_dir().join(format!("{id}.zebra.lock"));
    let Ok(file) = std::fs::File::create(&path) else { return true };
    match file.try_lock() {
        Ok(()) => {
            // The lock lives as long as the file handle: until the process exits.
            Box::leak(Box::new(file));
            true
        }
        Err(std::fs::TryLockError::WouldBlock) => false,
        Err(_) => true,
    }
}

/// Origin the custom protocol is served from (WRY maps custom schemes to
/// `http://<scheme>.localhost` on Windows).
///
/// # Returns
/// The origin without a trailing slash.
fn origin() -> String {
    if cfg!(windows) { "http://zebra.localhost".into() } else { "zebra://localhost".into() }
}

impl App {
    /// Creates the main form's window, menus, tray and timers, starts the Runtime and
    /// loads the backend.
    ///
    /// # Arguments
    /// * `build` - the build.
    /// * `mode` - runtime mode.
    /// * `proxy` - event loop proxy.
    /// * `target` - event loop target.
    ///
    /// # Returns
    /// The state.
    fn start(build: BuildOutput, mode: Mode, proxy: EventLoopProxy<UserEvent>, target: &EventLoopWindowTarget<UserEvent>) -> App {
        let config = Arc::new(build.config.clone());
        let shared = Arc::new(RwLock::new(config.clone()));
        let assets = AssetStore::default();
        assets.replace(build.files.clone());
        let menus = Menus::build(&config);
        let trays = menus::trays(&config, &menus, &assets);

        let out_proxy = proxy.clone();
        let ui_proxy = proxy.clone();
        let natives = Arc::new(NativeServices::new(
            Box::new(move |call| {
                if let Err(e) = ui_proxy.send_event(UserEvent::Native(call)) {
                    // The loop is shutting down; the call cannot run.
                    let tao::event_loop::EventLoopClosed(UserEvent::Native(call)) = e else { return };
                    call.done.complete(Err("the application is quitting".into()));
                }
            }),
            shared.clone(),
        ));
        let runtime = runtime::create(RuntimeContext {
            out: Arc::new(move |m| {
                let _ = out_proxy.send_event(UserEvent::Js(m));
            }),
            natives,
        });
        if mode == Mode::Debug {
            eprintln!("zebra: app runtime: {}", runtime.name());
        }
        let mut app = App {
            mode,
            build,
            config: config.clone(),
            shared,
            assets,
            origin: origin(),
            windows: BTreeMap::new(),
            ids: HashMap::new(),
            modal: HashMap::new(),
            runtime,
            epoch: 1,
            loaded: false,
            started: false,
            mirror: Mirror::default(),
            pending: HashMap::new(),
            calls: HashMap::new(),
            next_request: 0,
            menus,
            trays,
            timers: Vec::new(),
            errors: None,
            proxy,
            exit: false,
        };
        app.reset_timers();
        let main = config.application.main_form.clone();
        if let Err(e) = app.create_window(&main, target) {
            fatal(&format!("cannot create the main window: {e}"));
        }
        app.runtime.send_blocking(HostMessage::Load {
            backend: app.build.backend.clone(),
            config,
            restore: app.mirror.snapshot(),
            epoch: app.epoch,
        });
        app
    }

    /// Creates a form's window (not yet mounted) and installs its main menu.
    ///
    /// # Arguments
    /// * `name` - form name.
    /// * `target` - event loop target.
    ///
    /// # Returns
    /// Nothing.
    ///
    /// # Errors
    /// Unknown form, or the OS refused the window.
    fn create_window(&mut self, name: &str, target: &EventLoopWindowTarget<UserEvent>) -> Result<(), String> {
        let form = self.config.form(name).ok_or_else(|| format!("no form named {name}"))?.clone();
        #[allow(unused_mut)]
        let mut builder = props::creation(WindowBuilder::new(), &form, &self.config);
        let owner = form.str_prop("owner").and_then(|o| self.windows.get(o));
        if let Some(owner) = owner {
            builder = with_owner(builder, &owner.window);
        }
        let window = builder.build(target).map_err(|e| e.to_string())?;
        props::live(&window, &form, None, &self.assets);
        if form.props.get("icon").is_none() {
            if let Some(icon) = self.config.application.icon.as_deref().and_then(|p| props::icon(&self.assets, p)) {
                window.set_window_icon(Some(icon));
            }
        }
        props::place(&window, &form, owner.map(|o| &o.window));
        if let Some(menu) = form.str_prop("menu") {
            self.menus.install(menu, &window);
        }
        if form.bool_prop("modal") == Some(true) {
            let owner = form.str_prop("owner").unwrap_or(&self.config.application.main_form).to_owned();
            if owner != name {
                self.modal.insert(owner, name.to_owned());
            }
        }
        self.ids.insert(window.id(), name.to_owned());
        self.windows.insert(name.to_owned(), FormWindow { webview: None, window, ready: false });
        Ok(())
    }

    /// Mounts the WebView of a created window and shows it.
    ///
    /// # Arguments
    /// * `name` - form name.
    fn mount(&mut self, name: &str) {
        let Some(form) = self.config.form(name).cloned() else { return };
        let Some(fw) = self.windows.get_mut(name) else { return };
        let hostapi = hostapi_boot(&self.config.permissions.hostapi);
        let boot = json!({ "windowId": name, "epoch": self.epoch, "mode": if self.mode == Mode::Debug { "debug" } else { "release" }, "hostapi": hostapi });
        let assets = self.assets.clone();
        let (ipc_proxy, drop_proxy) = (self.proxy.clone(), self.proxy.clone());
        let (ipc_form, drop_form) = (name.to_owned(), name.to_owned());
        let (ipc_origin, nav_origin) = (self.origin.clone(), self.origin.clone());
        let mut builder = WebViewBuilder::new()
            .with_custom_protocol("zebra".into(), move |_, request| assets.respond(request.uri().path()))
            .with_initialization_script(SHELL.replace("__ZEBRA_BOOT__", &boot.to_string()))
            .with_ipc_handler(move |request| {
                // Only pages from the asset origin may talk to the host.
                if request.uri().to_string().starts_with(&ipc_origin) {
                    let _ = ipc_proxy.send_event(UserEvent::Ipc { form: ipc_form.clone(), body: request.into_body() });
                }
            })
            .with_navigation_handler(move |url| url.starts_with(&nav_origin))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_drag_drop_handler(move |event| match event {
                DragDropEvent::Drop { paths, .. } => {
                    let _ = drop_proxy.send_event(UserEvent::Drop { form: drop_form.clone(), paths });
                    true
                }
                _ => false,
            })
            .with_devtools(self.mode == Mode::Debug && cfg!(feature = "devtools"))
            .with_url(format!("{}/index.html", self.origin));
        if form.bool_prop("transparent") == Some(true) {
            builder = builder.with_transparent(true);
        }
        if let Some(color) = form.str_prop("color").and_then(props::parse_color) {
            builder = builder.with_background_color(color);
        }
        #[cfg(target_os = "linux")]
        let webview = {
            use tao::platform::unix::WindowExtUnix;
            use wry::WebViewBuilderExtUnix;
            builder.build_gtk(fw.window.default_vbox().expect("tao windows have a vbox"))
        };
        #[cfg(not(target_os = "linux"))]
        let webview = builder.build(&fw.window);
        match webview {
            Ok(webview) => fw.webview = Some(webview),
            Err(e) => {
                eprintln!("zebra: WebView of {name}: {e}");
                return;
            }
        }
        if form.bool_prop("visible") != Some(false) {
            fw.window.set_visible(true);
            if form.str_prop("windowState") == Some("minimized") {
                fw.window.set_minimized(true);
            }
            self.form_event(name, "onShow", json!({}));
        }
    }

    /// Opens a form: creates and mounts its window, or focuses it when open.
    ///
    /// # Arguments
    /// * `name` - form name.
    /// * `target` - event loop target.
    ///
    /// # Returns
    /// Nothing.
    ///
    /// # Errors
    /// Unknown form, or the OS refused the window.
    fn open_form(&mut self, name: &str, target: &EventLoopWindowTarget<UserEvent>) -> Result<(), String> {
        if let Some(fw) = self.windows.get(name) {
            fw.window.set_visible(true);
            fw.window.set_focus();
            return Ok(());
        }
        self.create_window(name, target)?;
        self.mount(name);
        Ok(())
    }

    /// Handles one event-loop event.
    ///
    /// # Arguments
    /// * `event` - the event.
    /// * `target` - event loop target.
    fn handle(&mut self, event: Event<'_, UserEvent>, target: &EventLoopWindowTarget<UserEvent>) {
        match event {
            Event::WindowEvent { window_id, event, .. } => {
                let Some(form) = self.ids.get(&window_id).cloned() else { return };
                self.window_event(&form, event);
            }
            Event::Opened { urls } => {
                for url in urls {
                    match url.to_file_path() {
                        Ok(path) if url.scheme() == "file" => self.app_event("onOpenFile", json!({ "path": path })),
                        _ => self.app_event("onOpenUrl", json!({ "url": url.as_str() })),
                    }
                }
            }
            Event::UserEvent(user) => match user {
                UserEvent::Js(message) => self.on_js(message, target),
                UserEvent::Native(call) => self.native_ui(call, target),
                UserEvent::Ipc { form, body } => self.on_ipc(&form, &body),
                UserEvent::Drop { form, paths } => self.form_event(&form, "onDropFiles", json!({ "files": paths })),
                UserEvent::Menu(event) => self.on_menu(event.id().0.as_str()),
                UserEvent::Tray(event) => self.on_tray(event),
                UserEvent::Build(result) => self.on_build(*result, target),
            },
            _ => {}
        }
    }

    /// Handles a window event of a form.
    ///
    /// # Arguments
    /// * `form` - form name.
    /// * `event` - the event.
    fn window_event(&mut self, form: &str, event: WindowEvent<'_>) {
        match event {
            WindowEvent::CloseRequested => self.request_close(form),
            WindowEvent::Resized(size) => {
                let Some(fw) = self.windows.get(form) else { return };
                let size = size.to_logical::<f64>(fw.window.scale_factor());
                self.form_event(form, "onResize", json!({ "width": size.width, "height": size.height }));
            }
            WindowEvent::Moved(pos) => {
                let Some(fw) = self.windows.get(form) else { return };
                let pos = pos.to_logical::<f64>(fw.window.scale_factor());
                self.form_event(form, "onMove", json!({ "left": pos.x, "top": pos.y }));
            }
            WindowEvent::Focused(true) => {
                // A modal form keeps the focus while its owner is blocked.
                if let Some(modal) = self.modal.get(form).and_then(|m| self.windows.get(m)) {
                    modal.window.set_focus();
                    return;
                }
                #[cfg(target_os = "macos")]
                if let (Some(menu), Some(fw)) = (self.config.form(form).and_then(|f| f.str_prop("menu")), self.windows.get(form)) {
                    self.menus.install(menu, &fw.window);
                }
                self.form_event(form, "onActivate", json!({}));
            }
            WindowEvent::Focused(false) => self.form_event(form, "onDeactivate", json!({})),
            WindowEvent::ThemeChanged(theme) => {
                let theme = if theme == tao::window::Theme::Dark { "dark" } else { "light" };
                self.form_event(form, "onThemeChange", json!({ "theme": theme }));
            }
            _ => {}
        }
    }

    /// Sends a form event to the backend, if the view resource connected it.
    ///
    /// # Arguments
    /// * `form` - form name.
    /// * `event` - event name.
    /// * `payload` - event arguments.
    fn form_event(&mut self, form: &str, event: &str, payload: Value) {
        if !self.loaded || !self.config.form(form).is_some_and(|f| f.handles(event)) {
            return;
        }
        self.send_backend(Envelope::new(Kind::Event, form, self.epoch, &format!("{form}.{event}"), payload));
    }

    /// Sends an application event, if the `<application>` element connected it.
    ///
    /// # Arguments
    /// * `event` - event name.
    /// * `payload` - event arguments.
    fn app_event(&mut self, event: &str, payload: Value) {
        if !self.loaded || !self.config.application.events.iter().any(|e| e == event) {
            return;
        }
        self.send_backend(Envelope::new(Kind::Event, APPLICATION_ID, self.epoch, &format!("{APPLICATION_ID}.{event}"), payload));
    }

    /// Sends a host-raised component event (menu item, action, tray, timer).
    ///
    /// # Arguments
    /// * `owner` - owning form or `@application`.
    /// * `component` - component name.
    /// * `event` - event name.
    fn component_event(&mut self, owner: &str, component: &str, event: &str) {
        if self.loaded {
            self.send_backend(Envelope::new(Kind::Event, owner, self.epoch, &format!("{component}.{event}"), json!({})));
        }
    }

    /// Queues an envelope for the backend; a full queue is reported, never blocks.
    ///
    /// # Arguments
    /// * `envelope` - the envelope.
    ///
    /// # Returns
    /// Whether it was queued.
    fn send_backend(&mut self, envelope: Envelope) -> bool {
        match self.runtime.send(HostMessage::Envelope(envelope)) {
            Ok(()) => true,
            Err(e) => {
                self.runtime_error(&e);
                false
            }
        }
    }

    /// Delivers an envelope to a form's shell once it has said hello.
    ///
    /// # Arguments
    /// * `form` - form name.
    /// * `envelope` - the envelope.
    fn post(&self, form: &str, envelope: &Envelope) {
        let Some(FormWindow { webview: Some(webview), ready: true, .. }) = self.windows.get(form) else { return };
        if let Err(e) = webview.evaluate_script(&format!("window.__zebraReceive({})", envelope.to_json())) {
            eprintln!("zebra: deliver to {form}: {e}");
        }
    }

    /// Answers a WebView call the backend will not: a failed `result`.
    ///
    /// # Arguments
    /// * `form` - the calling form.
    /// * `id` - the call's id.
    /// * `method` - the call's method.
    /// * `code` - `EREFUSED`, `ERELOAD` or `ETIMEDOUT`.
    /// * `message` - why.
    fn fail_call(&self, form: &str, id: u64, method: &str, code: &str, message: &str) {
        let mut result = Envelope::new(Kind::Result, form, self.epoch, method, json!({ "ok": false, "error": { "code": code, "message": message } }));
        result.id = Some(id);
        self.post(form, &result);
    }

    /// Sends a dev message to every open shell (debug runtime only).
    ///
    /// # Arguments
    /// * `method` - `dev.*` or `backend.epoch`.
    /// * `payload` - data.
    fn broadcast(&self, method: &str, payload: Value) {
        if self.mode != Mode::Debug && method != "backend.epoch" {
            return;
        }
        for form in self.windows.keys() {
            self.post(form, &Envelope::new(Kind::Dev, form, self.epoch, method, payload.clone()));
        }
    }

    /// Handles a message from a form's WebView.
    ///
    /// # Arguments
    /// * `form` - form whose WebView sent it.
    /// * `body` - message text.
    fn on_ipc(&mut self, form: &str, body: &str) {
        let Some(config) = self.config.form(form).cloned() else { return };
        match validate_inbound(body, &config, self.epoch, &self.config.permissions.hostapi) {
            Ok(Inbound::Hello) => {
                if let Some(fw) = self.windows.get_mut(form) {
                    fw.ready = true;
                }
                let hello = Envelope::new(Kind::Hello, form, self.epoch, "session.ready", json!({ "controls": self.mirror.form(form) }));
                self.post(form, &hello);
                if let Some(errors) = &self.errors {
                    self.post(form, &Envelope::new(Kind::Dev, form, self.epoch, "dev.error", json!({ "errors": errors })));
                }
            }
            Ok(Inbound::Event { control, event, props, envelope }) => {
                if let Some(modal) = self.modal.get(form).and_then(|m| self.windows.get(m)) {
                    modal.window.set_focus();
                    return;
                }
                self.mirror.set(form, &control, &props);
                if event == "onContextPopup" {
                    let popup = config.controls.get(&control).and_then(|c| c.popup_menu.as_deref());
                    if let (Some(popup), Some(fw)) = (popup, self.windows.get(form)) {
                        self.menus.popup(popup, &fw.window);
                    }
                    return;
                }
                if !self.loaded {
                    return;
                }
                if !self.send_backend(*envelope) {
                    self.post(form, &Envelope::new(Kind::Error, form, self.epoch, "event", json!({ "message": "backend queue is full" })));
                }
            }
            Ok(Inbound::Call { id, envelope }) => {
                let key = (form.to_owned(), id);
                if !self.loaded {
                    self.fail_call(form, id, &envelope.method, "ERELOAD", "the backend is not running");
                } else if self.calls.contains_key(&key) {
                    self.fail_call(form, id, &envelope.method, "EREFUSED", "a call with this id is in flight");
                } else {
                    let method = envelope.method.clone();
                    self.calls.insert(key.clone(), Instant::now() + CALL_DEADLINE);
                    if !self.send_backend(*envelope) {
                        self.calls.remove(&key);
                        self.fail_call(form, id, &method, "ERELOAD", "the backend is not running");
                    }
                }
            }
            Err(Reject::Call { id, method, code, reason }) => {
                eprintln!("zebra: {form}: refused call {method}: {reason}");
                self.fail_call(form, id, &method, code, &reason);
            }
            // An event from before a backend reload: dropped (6.2).
            Err(Reject::Stale(_)) => {}
            Err(reject) => {
                eprintln!("zebra: {form}: refused WebView message: {reject}");
                self.post(form, &Envelope::new(Kind::Error, form, self.epoch, "refused", json!({ "message": reject.to_string() })));
            }
        }
    }

    /// Handles a message from the JS thread.
    ///
    /// # Arguments
    /// * `message` - the message.
    /// * `target` - event loop target.
    fn on_js(&mut self, message: RuntimeMessage, target: &EventLoopWindowTarget<UserEvent>) {
        match message {
            RuntimeMessage::Loaded { epoch, error } if epoch == self.epoch => {
                self.loaded = error.is_none();
                if let Some(error) = error {
                    if self.mode == Mode::Release {
                        fatal(&format!("the application failed to start:\n{error}"));
                    }
                    self.runtime_error(&format!("backend failed to load: {error}"));
                }
                if !self.started {
                    self.started = true;
                    let main = self.config.application.main_form.clone();
                    self.form_event(&main, "onCreate", json!({}));
                    self.mount(&main);
                    self.app_event("onReady", json!({}));
                    let _ = target;
                } else {
                    // 6.2: each open form's onCreate runs again in the new Instance.
                    let open: Vec<String> = self.windows.keys().cloned().collect();
                    for form in open {
                        self.form_event(&form, "onCreate", json!({}));
                    }
                }
            }
            RuntimeMessage::Envelope(envelope) if envelope.epoch == self.epoch => match envelope.kind {
                Kind::Update => self.apply_update(&envelope),
                Kind::Response => self.on_response(&envelope),
                // Delivered once, to the form that made the call, while it waits.
                Kind::Result => {
                    let key = (envelope.window_id.clone(), envelope.id.unwrap_or_default());
                    if self.calls.remove(&key).is_some() {
                        self.post(&key.0, &envelope);
                    }
                }
                _ => {}
            },
            RuntimeMessage::Error { epoch, message } if epoch == self.epoch => self.runtime_error(&message),
            RuntimeMessage::Log { level, message } => eprintln!("[backend {level}] {message}"),
            // Anything else is from a retired epoch.
            _ => {}
        }
    }

    /// Applies a backend `update`: native components directly, controls via the WebView.
    ///
    /// # Arguments
    /// * `envelope` - the update.
    fn apply_update(&mut self, envelope: &Envelope) {
        let form = envelope.window_id.as_str();
        let Some(config) = self.config.form(form).cloned() else {
            return self.runtime_error(&format!("update for unknown form {form}"));
        };
        let Some(controls) = envelope.payload.get("controls").and_then(Value::as_object) else { return };
        if let Err(reject) = validate_update(&config, controls) {
            return self.runtime_error(&format!("{form}: refused backend update: {reject}"));
        }
        let mut web = Map::new();
        for (name, props) in controls {
            let Some(props) = props.as_object() else { continue };
            self.mirror.set(form, name, props);
            match config.controls[name].kind.as_str() {
                "menuitem" => self.menus.apply(form, name, props),
                "trayicon" => self.apply_tray(name, props),
                "timer" => self.apply_timer(form, name, props),
                _ => {
                    web.insert(name.clone(), Value::Object(props.clone()));
                }
            }
        }
        if !web.is_empty() {
            self.post(form, &Envelope::new(Kind::Update, form, self.epoch, "update", json!({ "controls": web })));
        }
    }

    /// Applies tray properties written by the backend.
    ///
    /// # Arguments
    /// * `name` - tray name.
    /// * `props` - `hint`, `visible`, `icon`.
    fn apply_tray(&mut self, name: &str, props: &Map<String, Value>) {
        let Some(tray) = self.trays.get(name) else { return };
        if let Some(hint) = props.get("hint").and_then(Value::as_str) {
            let _ = tray.set_tooltip(Some(hint));
        }
        if let Some(visible) = props.get("visible").and_then(Value::as_bool) {
            let _ = tray.set_visible(visible);
        }
        if let Some(path) = props.get("icon").and_then(Value::as_str) {
            let icon = props::rgba(&self.assets, path).and_then(|(d, w, h)| tray_icon::Icon::from_rgba(d, w, h).ok());
            let _ = tray.set_icon(icon);
        }
    }

    /// Applies timer properties written by the backend.
    ///
    /// # Arguments
    /// * `owner` - owning form.
    /// * `name` - timer name.
    /// * `props` - `interval`, `enabled`.
    fn apply_timer(&mut self, owner: &str, name: &str, props: &Map<String, Value>) {
        let Some(timer) = self.timers.iter_mut().find(|t| t.owner == owner && t.name == name) else { return };
        if let Some(ms) = props.get("interval").and_then(Value::as_u64) {
            timer.every = Duration::from_millis(ms.max(1));
        }
        if let Some(enabled) = props.get("enabled").and_then(Value::as_bool) {
            timer.enabled = enabled;
        }
        timer.next = Instant::now() + timer.every;
    }

    /// Rebuilds the timer list from the configuration.
    fn reset_timers(&mut self) {
        let now = Instant::now();
        self.timers = self
            .config
            .timers
            .iter()
            .map(|t| HostTimer {
                owner: t.owner.clone(),
                name: t.name.clone(),
                every: Duration::from_millis(t.interval.max(1)),
                enabled: t.enabled,
                next: now + Duration::from_millis(t.interval.max(1)),
            })
            .collect();
    }

    /// Handles a backend response.
    ///
    /// # Arguments
    /// * `envelope` - the response.
    fn on_response(&mut self, envelope: &Envelope) {
        let Some(pending) = envelope.id.and_then(|id| self.pending.remove(&id)) else { return };
        if pending.form != envelope.window_id {
            return;
        }
        if envelope.payload.get("cancel").and_then(Value::as_bool) != Some(true) {
            self.finish_close(&pending.form);
        }
    }

    /// Starts closing a form: asks `onCloseQuery` when connected, else closes.
    ///
    /// # Arguments
    /// * `form` - form name.
    fn request_close(&mut self, form: &str) {
        if self.pending.values().any(|p| p.form == form) {
            return;
        }
        let asks = self.loaded && self.config.form(form).is_some_and(|f| f.handles("onCloseQuery"));
        if !asks {
            return self.finish_close(form);
        }
        self.next_request += 1;
        let id = self.next_request;
        let mut envelope = Envelope::new(Kind::Request, form, self.epoch, &format!("{form}.onCloseQuery"), json!({}));
        envelope.id = Some(id);
        let deadline = Instant::now() + Duration::from_millis(self.config.limits.request_deadline_ms);
        if self.send_backend(envelope) {
            self.pending.insert(id, Pending { form: form.to_owned(), deadline });
        } else {
            self.finish_close(form);
        }
    }

    /// Closes a form's window for good and quits when it was the main form.
    ///
    /// # Arguments
    /// * `form` - form name.
    fn finish_close(&mut self, form: &str) {
        self.form_event(form, "onClose", json!({}));
        self.form_event(form, "onHide", json!({}));
        self.modal.retain(|_, modal| modal != form);
        if let Some(fw) = self.windows.remove(form) {
            self.ids.remove(&fw.window.id());
        }
        let main = form == self.config.application.main_form;
        if !self.config.application.keep_alive && (main || self.windows.is_empty()) {
            self.quit();
        }
    }

    /// Ends the application after `onQuit`.
    fn quit(&mut self) {
        self.app_event("onQuit", json!({}));
        self.exit = true;
    }

    /// Handles a menu click.
    ///
    /// # Arguments
    /// * `id` - menu id.
    fn on_menu(&mut self, id: &str) {
        let Some(item) = self.menus.clicked(id).cloned() else { return };
        if item.events.iter().any(|e| e == "onClick") {
            self.component_event(&item.owner, &item.name, "onClick");
        }
        if let Some(action) = item.action.as_ref().and_then(|a| self.config.actions.iter().find(|x| &x.name == a)).cloned() {
            self.component_event(&action.owner, &action.name, "onExecute");
        }
    }

    /// Handles a tray click.
    ///
    /// # Arguments
    /// * `event` - tray event.
    fn on_tray(&mut self, event: tray_icon::TrayIconEvent) {
        use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
        let TrayIconEvent::Click { id, button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event else { return };
        let Some(tray) = self.config.trays.iter().find(|t| t.name == id.0).cloned() else { return };
        if tray.events.iter().any(|e| e == "onClick") {
            self.component_event(&tray.owner, &tray.name, "onClick");
        }
    }

    /// Reports a backend or runtime error: stderr always, the dev overlay in debug.
    ///
    /// # Arguments
    /// * `message` - the error.
    fn runtime_error(&mut self, message: &str) {
        eprintln!("zebra: backend: {message}");
        self.broadcast("dev.error", json!({ "errors": [{ "message": message }] }));
    }

    /// Applies a finished dev build (6.1).
    ///
    /// # Arguments
    /// * `result` - pipeline output or diagnostics.
    /// * `target` - event loop target.
    fn on_build(&mut self, result: Result<BuildOutput, Vec<Diagnostic>>, target: &EventLoopWindowTarget<UserEvent>) {
        let new = match result {
            Err(errors) => {
                for e in &errors {
                    eprintln!("zebra: compile error: {}{}", e.file.as_deref().map(|f| format!("{f}: ")).unwrap_or_default(), e.message);
                }
                self.broadcast("dev.error", json!({ "errors": errors }));
                self.errors = Some(errors);
                return;
            }
            Ok(new) => new,
        };
        if self.errors.take().is_some() {
            self.broadcast("dev.ready", json!({}));
        }
        let plan = classify(&self.build, &new);
        if plan.is_empty() {
            return;
        }
        if plan.restart {
            self.broadcast("dev.restart", json!({}));
            restart();
            self.exit = true;
            return;
        }
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        self.assets.replace(new.files.clone());
        self.config = Arc::new(new.config.clone());
        *self.shared.write().expect("config lock") = self.config.clone();
        self.build = new;
        self.mirror.retain_existing(&self.config);

        for form in &plan.removed {
            self.modal.retain(|owner, modal| owner != form && modal != form);
            if let Some(fw) = self.windows.remove(form) {
                self.ids.remove(&fw.window.id());
            }
        }
        if plan.menus {
            self.menus = Menus::build(&self.config);
            for (name, fw) in &self.windows {
                if let Some(menu) = self.config.form(name).and_then(|f| f.str_prop("menu")) {
                    self.menus.install(menu, &fw.window);
                }
            }
        }
        if plan.trays || plan.menus {
            self.trays = menus::trays(&self.config, &self.menus, &self.assets);
        }
        if plan.timers {
            self.reset_timers();
        }
        // Fix (menu item stayed disabled after a menu edit in dev): rebuilt menus, trays and
        // timers start from the configuration, which loses what the backend wrote to them;
        // the mirrored values are applied again. Dropped alternative: asking the backend to
        // resend, which would need a new message kind and a handler in generated code.
        let mut kinds = Vec::new();
        if plan.menus {
            kinds.push("menuitem");
        }
        if plan.trays || plan.menus {
            kinds.push("trayicon");
        }
        if plan.timers {
            kinds.push("timer");
        }
        for (form, control, kind, props) in native_values(&self.config, &self.mirror, &kinds) {
            match kind.as_str() {
                "menuitem" => self.menus.apply(&form, &control, &props),
                "trayicon" => self.apply_tray(&control, &props),
                _ => self.apply_timer(&form, &control, &props),
            }
        }
        for form in &plan.recreate {
            if self.windows.remove(form).map(|fw| self.ids.remove(&fw.window.id())).is_some() {
                match self.create_window(form, target) {
                    Ok(()) => self.mount(form),
                    Err(e) => self.runtime_error(&format!("cannot recreate {form}: {e}")),
                }
            }
        }
        for (form, values) in &plan.live {
            if let (Some(fw), Some(config)) = (self.windows.get(form), self.config.form(form)) {
                props::live(&fw.window, config, Some(values), &self.assets);
                if values.contains_key("menu") {
                    if let Some(menu) = config.str_prop("menu") {
                        self.menus.install(menu, &fw.window);
                    }
                }
                if let Some(visible) = values.get("visible").and_then(Value::as_bool) {
                    fw.window.set_visible(visible);
                }
            }
        }
        if plan.backend {
            // 6.2: a new Instance; events in flight are dropped by the epoch.
            self.epoch += 1;
            self.loaded = false;
            self.pending.clear();
            // The calls' backend is gone; their callers learn it now, not at the deadline.
            for ((form, id), _) in std::mem::take(&mut self.calls) {
                self.fail_call(&form, id, "", "ERELOAD", "the backend was reloaded");
            }
            self.broadcast("backend.epoch", json!({ "epoch": self.epoch }));
            self.runtime.send_blocking(HostMessage::Load {
                backend: self.build.backend.clone(),
                config: self.config.clone(),
                restore: self.mirror.snapshot(),
                epoch: self.epoch,
            });
        }
        if plan.reload_all {
            self.broadcast("dev.reload", json!({ "stamp": stamp }));
            for fw in self.windows.values_mut() {
                fw.ready = false;
            }
            return;
        }
        if plan.css {
            self.broadcast("dev.css", json!({ "stamp": stamp }));
        }
        for form in &plan.refresh {
            self.post(form, &Envelope::new(Kind::Dev, form, self.epoch, "dev.refresh", json!({ "stamp": stamp })));
        }
    }

    /// Expires request and call deadlines and fires due timers.
    fn tick(&mut self) {
        let now = Instant::now();
        let expired: Vec<u64> = self.pending.iter().filter(|(_, p)| p.deadline <= now).map(|(id, _)| *id).collect();
        for id in expired {
            let Some(pending) = self.pending.remove(&id) else { continue };
            // Decision: a backend that does not answer onCloseQuery in time cannot keep a
            // window open forever; the request is cancelled and the form closes.
            eprintln!("zebra: {}.onCloseQuery passed its deadline; closing", pending.form);
            let mut cancel = Envelope::new(Kind::Cancel, &pending.form, self.epoch, &format!("{}.onCloseQuery", pending.form), json!({}));
            cancel.id = Some(id);
            let _ = self.runtime.send(HostMessage::Envelope(cancel));
            self.finish_close(&pending.form);
        }
        let late: Vec<(String, u64)> = self.calls.iter().filter(|(_, deadline)| **deadline <= now).map(|(key, _)| key.clone()).collect();
        for key in late {
            self.calls.remove(&key);
            self.fail_call(&key.0, key.1, "", "ETIMEDOUT", "the backend did not answer in time");
        }
        let mut due = Vec::new();
        for timer in self.timers.iter_mut().filter(|t| t.enabled && t.next <= now) {
            // A late timer fires once and keeps its period; it never bursts to catch up.
            timer.next = now + timer.every;
            due.push((timer.owner.clone(), timer.name.clone()));
        }
        for (owner, name) in due {
            if owner == APPLICATION_ID || self.windows.contains_key(&owner) {
                self.component_event(&owner, &name, "onTimer");
            }
        }
    }

    /// Wait mode until the next deadline or timer.
    ///
    /// # Returns
    /// The control flow.
    fn control_flow(&self) -> ControlFlow {
        if self.exit {
            return ControlFlow::Exit;
        }
        let next = self
            .timers
            .iter()
            .filter(|t| t.enabled)
            .map(|t| t.next)
            .chain(self.pending.values().map(|p| p.deadline))
            .chain(self.calls.values().copied())
            .min();
        match next {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        }
    }
}

/// Sets a new window's owner per OS (`owner` form property).
///
/// # Arguments
/// * `builder` - window builder.
/// * `owner` - owner window.
///
/// # Returns
/// The builder.
#[allow(unused_variables)]
fn with_owner(builder: WindowBuilder, owner: &Window) -> WindowBuilder {
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{WindowBuilderExtMacOS, WindowExtMacOS};
        return builder.with_parent_window(owner.ns_window());
    }
    #[cfg(target_os = "windows")]
    {
        use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
        return builder.with_owner_window(owner.hwnd() as _);
    }
    #[cfg(target_os = "linux")]
    {
        use tao::platform::unix::{WindowBuilderExtUnix, WindowExtUnix};
        return builder.with_transient_for(owner.gtk_window());
    }
    #[allow(unreachable_code)]
    builder
}

/// Starts a fresh copy of this process with the same arguments (`dev.restart`).
fn restart() {
    let Ok(exe) = std::env::current_exe() else { return };
    if let Err(e) = std::process::Command::new(exe).args(std::env::args_os().skip(1)).spawn() {
        eprintln!("zebra: restart failed: {e}");
    }
}

/// Shows a fatal error and exits.
///
/// # Arguments
/// * `message` - the error.
///
/// # Returns
/// Never.
fn fatal(message: &str) -> ! {
    eprintln!("zebra: {message}");
    let _ = rfd::MessageDialog::new().set_level(rfd::MessageLevel::Error).set_title("Error").set_description(message).show();
    std::process::exit(1)
}

/// Whether a form name is valid for a native window call.
///
/// # Arguments
/// * `config` - host configuration.
/// * `name` - form name.
///
/// # Returns
/// The form.
///
/// # Errors
/// No such form.
fn form_of<'a>(config: &'a HostConfig, name: &str) -> Result<&'a FormConfig, String> {
    config.form(name).ok_or_else(|| format!("no form named {name}"))
}

/// Host-owned component values the backend wrote, to apply again after those components
/// are rebuilt from the configuration.
///
/// # Arguments
/// * `config` - host configuration (after the rebuild).
/// * `mirror` - property mirror.
/// * `kinds` - component types rebuilt (`menuitem`, `trayicon`, `timer`).
///
/// # Returns
/// `(form, control, kind, props)` for every mirrored value of those types.
fn native_values(config: &HostConfig, mirror: &Mirror, kinds: &[&str]) -> Vec<(String, String, String, Map<String, Value>)> {
    let mut out = Vec::new();
    for form in &config.forms {
        for (control, props) in mirror.form(&form.name) {
            let Some(entry) = form.controls.get(&control).filter(|e| kinds.contains(&e.kind.as_str())) else { continue };
            if let Value::Object(props) = props {
                out.push((form.name.clone(), control, entry.kind.clone(), props));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilt_native_components_get_backend_values_back() {
        let config = crate::pipeline::tests::sample_build().config;
        let mut mirror = Mirror::default();
        let set = |m: &mut Mirror, c: &str, v: Value| m.set("MainForm", c, v.as_object().unwrap());
        set(&mut mirror, "SaveItem", json!({ "enabled": true }));
        set(&mut mirror, "ClockTimer", json!({ "enabled": false }));
        set(&mut mirror, "NameEdit", json!({ "text": "web control, not native" }));
        let menus = native_values(&config, &mirror, &["menuitem"]);
        assert_eq!(
            menus,
            vec![("MainForm".into(), "SaveItem".into(), "menuitem".into(), json!({ "enabled": true }).as_object().unwrap().clone())]
        );
        let timers = native_values(&config, &mirror, &["timer"]);
        assert_eq!(timers.len(), 1);
        assert_eq!(timers[0].1, "ClockTimer");
    }
}
