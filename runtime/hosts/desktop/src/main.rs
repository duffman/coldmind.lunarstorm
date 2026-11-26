//! `lunarstorm-desktop <script.js>`: runs a script as a desktop application.
//!
//! The process has two threads that matter:
//! - the **main thread** runs tao's event loop and owns every native window
//!   (the OS requires windows on the main thread);
//! - the **backend thread** runs the script in the Lunarstorm Runtime
//!   ([`lunarstorm_host_desktop::Backend`]).
//!
//! They meet in two places. The backend reaches the main thread through tao's
//! `EventLoopProxy` (this file's [`Shell`] implementation sends a [`Message`]);
//! the main thread reaches the backend through `Backend::dispatch` and
//! `Settler::settle`, which post tasks to the Runtime's loop. Neither thread
//! waits for the other.
//!
//! What a script gets:
//!
//! | call                                              | does                      |
//! |---------------------------------------------------|---------------------------|
//! | `native.window.create(name, {title,width,height})`| opens a window            |
//! | `native.window.setTitle(name, title)`             | retitles it               |
//! | `native.window.close(name)`                       | closes it                 |
//! | `native.app.quit(code)`                           | ends the application      |
//!
//! and, through `serve`, the events `Window.onResize` (`{width, height}` in
//! logical pixels), `Window.onCloseQuery` (a request; answer `{cancel: true}` to
//! keep the window) and `Window.onClose`, with the window's name as `form`.
//!
//! Decisions:
//! - The application ends when its last window closes, or at `app.quit`.
//! - A close request the script does not answer within [`CLOSE_DEADLINE`] closes
//!   the window: a script that hangs must not make a window unclosable.
//! - This shell has no WebView, so an `update` envelope (a control property
//!   write) has no target and is reported on stderr rather than dropped silently.
//!
//! Exit status: the code given to `app.quit`, 0 when the last window closed,
//! 1 when the script did not load, 2 for a usage error.

use lunarstorm_host_desktop::{Backend, Program, Settler, Shell};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder};

/// The one backend instance's epoch: this shell never reloads.
const EPOCH: u64 = 1;

/// How long a script has to answer `Window.onCloseQuery`.
const CLOSE_DEADLINE: Duration = Duration::from_secs(2);

/// Longest one JS entry may run.
const BUDGET: Duration = Duration::from_secs(5);

/// Heap limit of the backend, in bytes.
const HEAP: usize = 256 << 20;

/// What the backend thread sends to the main thread.
enum Message {
    /// The script finished loading, or failed to.
    Loaded(Option<String>),
    /// The script called a native method.
    Native {
        /// The call's id, for the result.
        call: u64,
        /// The service.
        namespace: String,
        /// The method.
        method: String,
        /// The arguments as a JSON array.
        arguments: String,
    },
    /// An envelope from the script: a response or an update.
    Envelope(String),
}

/// The tao side of the `Shell` contract: everything that needs the main thread
/// is forwarded to it as a user event; logging is done where it is called.
struct TaoShell {
    /// Wakes tao's event loop with a message. Behind a lock because the contract
    /// requires `Sync` and the proxy is only `Send` on every platform.
    proxy: Mutex<EventLoopProxy<Message>>,
}

impl TaoShell {
    /// Sends a message to the main thread.
    ///
    /// # Arguments
    /// * `message` - the message; dropped when the event loop has ended.
    fn post(&self, message: Message) {
        let _ = self.proxy.lock().expect("proxy lock").send_event(message);
    }
}

impl Shell for TaoShell {
    /// Tells the main thread how the load went.
    ///
    /// # Arguments
    /// * `_epoch` - always [`EPOCH`].
    /// * `error` - the load error.
    fn loaded(&self, _epoch: u64, error: Option<&str>) {
        self.post(Message::Loaded(error.map(str::to_owned)));
    }

    /// Forwards an envelope to the main thread.
    ///
    /// # Arguments
    /// * `_epoch` - always [`EPOCH`].
    /// * `envelope` - the envelope as JSON.
    fn send(&self, _epoch: u64, envelope: &str) {
        self.post(Message::Envelope(envelope.to_owned()));
    }

    /// Forwards a native call to the main thread, where windows live.
    ///
    /// # Arguments
    /// * `_epoch` - always [`EPOCH`].
    /// * `call` - the call's id.
    /// * `namespace` - the service.
    /// * `method` - the method.
    /// * `arguments` - the arguments as a JSON array.
    fn native(&self, _epoch: u64, call: u64, namespace: &str, method: &str, arguments: &str) {
        self.post(Message::Native {
            call,
            namespace: namespace.to_owned(),
            method: method.to_owned(),
            arguments: arguments.to_owned(),
        });
    }

    /// Prints a script error to stderr.
    ///
    /// # Arguments
    /// * `_epoch` - always [`EPOCH`].
    /// * `message` - the description.
    fn error(&self, _epoch: u64, message: &str) {
        eprintln!("lunarstorm-desktop: {message}");
    }

    /// Prints `console` output: `info` to stdout, the rest to stderr.
    ///
    /// # Arguments
    /// * `level` - the level.
    /// * `message` - the text.
    fn log(&self, level: &str, message: &str) {
        if level == "info" {
            println!("{message}");
        } else {
            eprintln!("{message}");
        }
    }

    /// Nothing to release: the windows belong to the process, not to an epoch.
    ///
    /// # Arguments
    /// * `_epoch` - the retired epoch.
    fn retired(&self, _epoch: u64) {}
}

/// The main thread's state.
struct App {
    /// The script's backend; taken when the event loop ends.
    backend: Option<Backend<TaoShell>>,
    /// Delivers native-call results.
    settler: Settler<TaoShell>,
    /// The open windows by the name the script gave them.
    windows: HashMap<String, Window>,
    /// Close requests waiting for the script: request id to window name and
    /// the time the window closes anyway.
    closing: HashMap<u64, (String, Instant)>,
    /// Id of the last close request.
    requests: u64,
}

impl App {
    /// Sends an envelope to the script.
    ///
    /// # Arguments
    /// * `envelope` - the envelope.
    fn dispatch(&self, envelope: Value) {
        if let Some(backend) = &self.backend {
            backend.dispatch(EPOCH, envelope.to_string());
        }
    }

    /// Closes a window and tells the script; ends the process with the last one.
    ///
    /// # Arguments
    /// * `name` - the window's name.
    /// * `control_flow` - set to exit when no window is left.
    ///
    /// # Returns
    /// Whether a window with that name was open.
    fn close(&mut self, name: &str, control_flow: &mut ControlFlow) -> bool {
        if self.windows.remove(name).is_none() {
            return false;
        }
        self.dispatch(json!({ "kind": "event", "windowId": name, "method": "Window.onClose", "payload": {} }));
        if self.windows.is_empty() {
            *control_flow = ControlFlow::Exit;
        }
        true
    }

    /// Runs one native call on the main thread.
    ///
    /// # Arguments
    /// * `target` - tao's window target, for creating windows.
    /// * `control_flow` - set by `app.quit` and by closing the last window.
    /// * `namespace` - the service.
    /// * `method` - the method.
    /// * `arguments` - the arguments.
    ///
    /// # Returns
    /// The result as JSON; empty for `undefined`.
    ///
    /// # Errors
    /// The message the script's promise rejects with.
    fn native(
        &mut self,
        target: &EventLoopWindowTarget<Message>,
        control_flow: &mut ControlFlow,
        namespace: &str,
        method: &str,
        arguments: &[Value],
    ) -> Result<String, String> {
        let name = arguments.first().and_then(Value::as_str);
        match (namespace, method) {
            ("window", "create") => {
                let name = name.ok_or("window.create takes the window's name")?;
                if self.windows.contains_key(name) {
                    return Err(format!("a window named '{name}' is already open"));
                }
                let options = arguments.get(1).cloned().unwrap_or(Value::Null);
                let size = |key: &str, default: f64| options.get(key).and_then(Value::as_f64).unwrap_or(default);
                let window = WindowBuilder::new()
                    .with_title(options.get("title").and_then(Value::as_str).unwrap_or(name))
                    .with_inner_size(LogicalSize::new(size("width", 800.0), size("height", 600.0)))
                    .build(target)
                    .map_err(|error| format!("cannot create the window: {error}"))?;
                self.windows.insert(name.to_owned(), window);
                Ok(String::new())
            }
            ("window", "setTitle") => {
                let name = name.ok_or("window.setTitle takes the window's name")?;
                let window = self.windows.get(name).ok_or_else(|| format!("no window named '{name}'"))?;
                window.set_title(arguments.get(1).and_then(Value::as_str).unwrap_or_default());
                Ok(String::new())
            }
            ("window", "close") => {
                let name = name.ok_or("window.close takes the window's name")?;
                if self.close(name, control_flow) {
                    Ok(String::new())
                } else {
                    Err(format!("no window named '{name}'"))
                }
            }
            ("app", "quit") => {
                let code = arguments.first().and_then(Value::as_i64).unwrap_or(0);
                *control_flow = ControlFlow::ExitWithCode(code as i32);
                Ok(String::new())
            }
            _ => Err(format!("this host has no native method {namespace}.{method}")),
        }
    }

    /// Acts on a message from the backend thread.
    ///
    /// # Arguments
    /// * `message` - the message.
    /// * `target` - tao's window target.
    /// * `control_flow` - the loop's control flow.
    fn message(&mut self, message: Message, target: &EventLoopWindowTarget<Message>, control_flow: &mut ControlFlow) {
        match message {
            Message::Loaded(None) => {}
            Message::Loaded(Some(error)) => {
                eprintln!("lunarstorm-desktop: the script did not load: {error}");
                *control_flow = ControlFlow::ExitWithCode(1);
            }
            Message::Native { call, namespace, method, arguments } => {
                let result = match serde_json::from_str::<Vec<Value>>(&arguments) {
                    Ok(arguments) => self.native(target, control_flow, &namespace, &method, &arguments),
                    Err(error) => Err(format!("bad arguments: {error}")),
                };
                self.settler.settle(EPOCH, call, result);
            }
            Message::Envelope(text) => {
                let envelope: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                if envelope["kind"] == "response" {
                    // The answer to a close request: the window closes unless the
                    // script cancelled. A late answer finds no entry and is ignored.
                    if let Some((name, _)) = envelope["id"].as_u64().and_then(|id| self.closing.remove(&id)) {
                        if envelope["payload"]["cancel"] != true {
                            self.close(&name, control_flow);
                        }
                    }
                } else {
                    eprintln!("lunarstorm-desktop: this host has no WebView to apply: {text}");
                }
            }
        }
    }

    /// Acts on a window event from the OS.
    ///
    /// # Arguments
    /// * `name` - the window's name.
    /// * `event` - the event.
    fn window(&mut self, name: String, event: WindowEvent<'_>) {
        match event {
            WindowEvent::CloseRequested => {
                self.requests += 1;
                let id = self.requests;
                self.dispatch(json!({ "kind": "request", "id": id, "windowId": name, "method": "Window.onCloseQuery", "payload": {} }));
                self.closing.insert(id, (name, Instant::now() + CLOSE_DEADLINE));
            }
            WindowEvent::Resized(size) => {
                let scale = self.windows.get(&name).map_or(1.0, Window::scale_factor);
                let size: LogicalSize<f64> = size.to_logical(scale);
                let payload = json!({ "width": size.width.round(), "height": size.height.round() });
                self.dispatch(json!({ "kind": "event", "windowId": name, "method": "Window.onResize", "payload": payload }));
            }
            _ => {}
        }
    }

    /// Closes the windows whose close request the script did not answer in time,
    /// and tells tao when to wake for the next deadline.
    ///
    /// # Arguments
    /// * `control_flow` - set to wait for the next deadline, or for an event.
    fn deadlines(&mut self, control_flow: &mut ControlFlow) {
        let now = Instant::now();
        let overdue: Vec<u64> = self.closing.iter().filter(|(_, (_, at))| *at <= now).map(|(id, _)| *id).collect();
        for id in overdue {
            if let Some((name, _)) = self.closing.remove(&id) {
                self.close(&name, control_flow);
            }
        }
        // An exit already decided stays decided.
        if matches!(*control_flow, ControlFlow::ExitWithCode(_)) {
            return;
        }
        *control_flow = match self.closing.values().map(|(_, at)| *at).min() {
            Some(next) => ControlFlow::WaitUntil(next),
            None => ControlFlow::Wait,
        };
    }
}

/// Loads the script and runs tao's event loop on the main thread.
fn main() {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: lunarstorm-desktop <script.js>");
        std::process::exit(2);
    };
    let path = Path::new(&path);
    let code = match std::fs::read_to_string(path) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("lunarstorm-desktop: cannot read {}: {error}", path.display());
            std::process::exit(2);
        }
    };
    // The script may read and import files beside itself, and nothing else.
    let roots = path.canonicalize().ok().and_then(|file| file.parent().map(Path::to_path_buf)).into_iter().collect();

    let event_loop = EventLoopBuilder::<Message>::with_user_event().build();
    let shell = Arc::new(TaoShell { proxy: Mutex::new(event_loop.create_proxy()) });
    let backend = Backend::spawn(shell);
    backend.load(Program {
        epoch: EPOCH,
        code: Arc::from(code),
        restore: String::from("{}"),
        memory: Some(HEAP),
        budget: BUDGET,
        roots,
        environment: Vec::new(),
        name: path.display().to_string(),
        // No WebView, so no `call`s to answer.
        areas: Vec::new(),
    });
    let mut app = App {
        settler: backend.settler(),
        backend: Some(backend),
        windows: HashMap::new(),
        closing: HashMap::new(),
        requests: 0,
    };

    event_loop.run(move |event, target, control_flow| {
        match event {
            Event::UserEvent(message) => app.message(message, target, control_flow),
            Event::WindowEvent { window_id, event, .. } => {
                let name = app.windows.iter().find(|(_, window)| window.id() == window_id).map(|(name, _)| name.clone());
                if let Some(name) = name {
                    app.window(name, event);
                }
            }
            // Stops the backend thread before the process exits.
            Event::LoopDestroyed => drop(app.backend.take()),
            _ => {}
        }
        app.deadlines(control_flow);
    });
}
