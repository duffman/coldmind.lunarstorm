//! devermind dev server: serves a web app in development from inside the host process.
//!
//! Requests compile on demand through `oj_compiler` (vendored from lovablelabs/oj, MIT),
//! imports resolve through `oj_resolver`, and every compiled module registers in `oj_graph`.
//! A file change walks the graph to its HMR boundaries and is pushed to pages over a WebSocket
//! speaking Vite's HMR protocol; React components hot-swap through React Refresh. The same socket
//! carries app channels (`app:<name>` custom events) between pages and the host.
//!
//! Standalone hosts hold a [`DevServer`] handle backed by its own Tokio runtime thread.
//! Embedded hosts hold [`Embedded`] and mount its router on their existing listener.

mod hot_reload;
mod html_entry;
pub mod module_pipeline;
pub mod module_urls;
mod stylesheets;

/// Compatibility names for hosts using the former public modules.
pub use module_pipeline as pipeline;
pub use module_urls as urls;
mod file_watcher;
mod watch_changes;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State as AxumState;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use oj_compiler::ImportMetaEnv;
use oj_graph::ModuleGraph;
use oj_resolver::OjResolver;
use tokio::sync::{broadcast, oneshot};

const CLIENT_JS: &str = include_str!("../assets/client.js");
const REFRESH_RUNTIME_JS: &str = include_str!("../assets/refresh-runtime.js");
const REFRESH_PREAMBLE_JS: &str = include_str!("../assets/refresh-preamble.js");

/// Path of the HMR WebSocket.
pub const HMR_PATH: &str = "/__devermind_hmr";

/// The shell's HMR runtime. When it resolves from the app root, `/@devermind/client.js` re-exports
/// it and pages get no client script: the shell owns the page's one socket.
pub const SHELL_HOT: &str = "@coldmind/devermind-shell/hot";

/// A message on an app channel: pages send it as the `custom` event `app:<channel>`.
#[derive(Debug, Clone, PartialEq)]
pub struct AppMessage {
    /// Channel name, without the `app:` prefix.
    pub channel: String,
    pub data: serde_json::Value,
}

/// Host validation runs on existing change batches before HMR compilation.
#[derive(Clone)]
struct ValidationHook(Arc<dyn Fn(&[PathBuf]) -> Result<bool, String> + Send + Sync>);
impl std::fmt::Debug for ValidationHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ValidationHook")
    }
}

/// Dev server settings.
#[derive(Debug, Clone)]
pub struct Config {
    /// App root: `index.html` and the source it imports. `/` serves `<root>/index.html`.
    pub root: PathBuf,
    /// Interface to bind; `127.0.0.1` by default.
    pub host: String,
    /// Port to bind; `0` picks a free one.
    pub port: u16,
    /// Extra compile-time constants, `(expression, replacement source)`, applied on top of
    /// `import.meta.env.*` and `process.env.NODE_ENV`.
    pub define: Vec<(String, String)>,
    /// Watch files and push HMR updates.
    pub watch: bool,
    /// String aliases from project configuration or the host.
    pub alias: Vec<(String, String)>,
    port_explicit: bool,
    host_explicit: bool,
    project_config: Option<devermind_core::config::ProjectConfig>,
    validation: Option<ValidationHook>,
}

impl Config {
    /// Defaults for `root`: `127.0.0.1:5173`, watching, no extra defines.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            host: "127.0.0.1".into(),
            port: 5173,
            define: Vec::new(),
            watch: true,
            alias: Vec::new(),
            port_explicit: false,
            host_explicit: false,
            project_config: None,
            validation: None,
        }
    }

    /// Explicit overrides, including values equal to defaults, take precedence over Vite.
    pub fn set_port(&mut self, port: u16) {
        self.port = port;
        self.port_explicit = true;
    }
    pub fn set_host(&mut self, host: String) {
        self.host = host;
        self.host_explicit = true;
    }

    /// Inject native host configuration and disable compatibility discovery.
    pub fn with_project_config(mut self, config: devermind_core::config::ProjectConfig) -> Self {
        self.project_config = Some(config);
        self
    }

    /// Validate changed host files using the existing watcher; `false` skips unrelated batches.
    pub fn with_validation(
        mut self,
        validate: impl Fn(&[PathBuf]) -> Result<bool, String> + Send + Sync + 'static,
    ) -> Self {
        self.validation = Some(ValidationHook(Arc::new(validate)));
        self
    }

    fn with_vite(mut self) -> anyhow::Result<Self> {
        let config = match self.project_config.take() {
            Some(config) => Some(config),
            None => devermind_core::config::load(&self.root, "serve", "development")?,
        };
        if let Some(vite) = config {
            if let Some(root) = vite.root {
                self.root = root;
            }
            if !self.port_explicit && self.port == 5173 {
                if let Some(port) = vite.server_port {
                    self.port = port;
                }
            }
            if !self.host_explicit && self.host == "127.0.0.1" {
                if let Some(host) = vite.server_host {
                    self.host = host;
                }
            }
            let mut define = vite.define;
            define.extend(self.define);
            self.define = define;
            let mut alias = self.alias;
            alias.extend(vite.alias);
            self.alias = alias;
        }
        Ok(self)
    }
}

/// Shared server state.
pub struct State {
    /// Canonical app root.
    pub root: PathBuf,
    /// Directories `/@fs/` and root-relative URLs may read: the root and the enclosing workspace.
    pub allow: Vec<PathBuf>,
    pub resolver: OjResolver,
    pub graph: Mutex<ModuleGraph>,
    pub(crate) cache: Mutex<HashMap<String, module_pipeline::Cached>>,
    pub(crate) cjs_files: Mutex<HashMap<PathBuf, bool>>,
    /// Exact external files reached through imports, not their containing directories.
    resolved_files: Mutex<HashMap<String, PathBuf>>,
    pub env: Arc<ImportMetaEnv>,
    pub(crate) frames: broadcast::Sender<hot_reload::Frame>,
    pub(crate) buffered: Mutex<Option<String>>,
    pub(crate) messages: broadcast::Sender<AppMessage>,
    pub(crate) next_client: std::sync::atomic::AtomicU64,
    client_js: String,
    validation: Option<ValidationHook>,
    validation_failed: std::sync::atomic::AtomicBool,
}

impl State {
    fn new(config: &Config) -> anyhow::Result<Self> {
        let root = config
            .root
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("root {}: {e}", config.root.display()))?;
        let workspace = workspace_root(&root);
        let mut allow = vec![root.clone()];
        if workspace != root {
            allow.push(workspace);
        }
        let mut define = vec![
            (
                "import.meta.env".to_string(),
                serde_json::json!({
                    "MODE": "development", "DEV": true, "PROD": false,
                    "SSR": false, "BASE_URL": "/"
                })
                .to_string(),
            ),
            (
                "import.meta.env.MODE".to_string(),
                "\"development\"".to_string(),
            ),
            ("import.meta.env.DEV".to_string(), "true".to_string()),
            ("import.meta.env.PROD".to_string(), "false".to_string()),
            ("import.meta.env.SSR".to_string(), "false".to_string()),
            ("import.meta.env.BASE_URL".to_string(), "\"/\"".to_string()),
            (
                "process.env.NODE_ENV".to_string(),
                "\"development\"".to_string(),
            ),
        ];
        define.extend(config.define.iter().cloned());
        let client_js = CLIENT_JS
            .replace("__HMR_PROTOCOL__", "null")
            .replace("__HMR_HOSTNAME__", "null")
            .replace("__HMR_PORT__", "null")
            .replace(
                "__HMR_PATH__",
                &serde_json::Value::from(HMR_PATH).to_string(),
            )
            .replace("__HMR_ENABLE_OVERLAY__", "true")
            .replace("__WS_TOKEN__", "\"\"")
            .replace(
                "__FORWARD_CONSOLE__",
                r#"{"enabled":false,"unhandledErrors":false,"logLevels":[]}"#,
            );
        Ok(Self {
            resolver: OjResolver::with_options(
                &root,
                &["browser", "import", "module", "development", "default"].map(String::from),
                &config.alias,
                &[],
            ),
            root,
            allow,
            graph: Mutex::new(ModuleGraph::new()),
            cache: Mutex::new(HashMap::new()),
            cjs_files: Mutex::new(HashMap::new()),
            resolved_files: Mutex::new(HashMap::new()),
            env: Arc::new(ImportMetaEnv::new(define, Vec::new())),
            frames: broadcast::channel(256).0,
            buffered: Mutex::new(None),
            messages: broadcast::channel(256).0,
            next_client: std::sync::atomic::AtomicU64::new(1),
            client_js,
            validation: config.validation.clone(),
            validation_failed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Resolves imports with an app-root peer fallback only for external sources.
    ///
    /// Each successful resolution records one canonical file for later HTTP access;
    /// resolving a linked package never grants access to its adjacent directory.
    /// Explicitly ignored resolver results are not retried as peer dependencies.
    pub(crate) fn resolve_import(&self, dir: &Path, spec: &str) -> Option<PathBuf> {
        let resolved = match self.resolver.resolve(dir, spec) {
            Ok(file) => file,
            Err(error)
                if !error.ignored && !dir.starts_with(&self.root) && module_urls::is_bare(spec) =>
            {
                self.resolver.resolve(&self.root, spec).ok()?
            }
            Err(_) => return None,
        };
        let canonical = resolved.canonicalize().ok()?;
        self.resolved_files
            .lock()
            .unwrap()
            .insert(module_urls::url_of(&self.root, &resolved), canonical);
        Some(resolved)
    }

    /// URL of the shell's HMR runtime when the app depends on `@coldmind/devermind-shell`.
    /// Registers the resolved file before emitting its URL, so linked packages outside
    /// the workspace can load this runtime without granting access to their directory.
    pub fn shell_hot_url(&self) -> Option<String> {
        self.resolve_import(&self.root, SHELL_HOT)
            .map(|file| module_urls::url_of(&self.root, &file))
    }

    /// `/@devermind/client.js`: the shell's runtime re-exported (one module instance, one socket),
    /// else the built-in client.
    fn client_module(&self) -> String {
        match self.shell_hot_url() {
            Some(url) => format!("export * from {};\n", serde_json::Value::from(url)),
            None => self.client_js.clone(),
        }
    }
}

/// A running dev server. Dropping it (or [`DevServer::stop`]) shuts it down.
pub struct DevServer {
    addr: SocketAddr,
    state: Arc<State>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl DevServer {
    /// Binds and starts serving; returns once the socket is listening.
    pub fn start(config: Config) -> anyhow::Result<Self> {
        let config = config.with_vite()?;
        let state = Arc::new(State::new(&config)?);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("devermind")
            .thread_stack_size(oj_compiler::COMPILE_STACK_SIZE)
            .build()?;
        // Bound with std so `start` works from inside another async runtime (tests, the napi host).
        let listener = std::net::TcpListener::bind((config.host.as_str(), config.port))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let watcher = if config.watch {
            let dir = state
                .allow
                .last()
                .cloned()
                .unwrap_or_else(|| state.root.clone());
            Some(file_watcher::start(state.clone(), &dir)?)
        } else {
            None
        };
        let (shutdown, stopped) = oneshot::channel::<()>();
        let app = router(state.clone());
        let thread = std::thread::Builder::new()
            .name("devermind-server".into())
            .spawn(move || {
                let _watcher = watcher;
                runtime.block_on(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        return;
                    };
                    // Dev WebSockets can stay open forever; stop must not wait for them.
                    tokio::select! {
                        _ = async { let _ = axum::serve(listener, app).await; } => {},
                        _ = stopped => {},
                    }
                });
                runtime.shutdown_background();
            })?;
        Ok(Self {
            addr,
            state,
            shutdown: Some(shutdown),
            thread: Some(thread),
        })
    }

    /// Bound address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://host:port/`.
    pub fn url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    /// Shared state (module graph, resolver), for hosts and tests.
    pub fn state(&self) -> &Arc<State> {
        &self.state
    }

    /// Treats `files` as changed, exactly as the watcher does.
    pub fn invalidate(&self, files: &[PathBuf]) {
        hot_reload::on_change(&self.state, files);
    }

    /// Sends `data` on the app channel `channel` to every connected page. Not held for pages that
    /// connect later.
    pub fn publish(&self, channel: &str, data: serde_json::Value) {
        hot_reload::publish(&self.state, channel, data);
    }

    /// App channel messages pages send, from now on.
    pub fn messages(&self) -> broadcast::Receiver<AppMessage> {
        self.state.messages.subscribe()
    }

    /// Stops serving and waits for the server thread.
    pub fn stop(mut self) {
        self.shutdown_now();
    }

    fn shutdown_now(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for DevServer {
    fn drop(&mut self) {
        self.shutdown_now();
    }
}

/// The dev server without a listener, for hosts that serve its router from their own socket and
/// runtime (the Tauri plugin mounts it on the app gateway). Dropping it stops the watcher.
pub struct Embedded {
    state: Arc<State>,
    _watcher: Option<Box<dyn notify::Watcher + Send>>,
}

impl Embedded {
    /// Prepares state and, when `config.watch`, starts the file watcher. `host`/`port` are unused.
    pub fn start(config: &Config) -> anyhow::Result<Self> {
        let config = config.clone().with_vite()?;
        let state = Arc::new(State::new(&config)?);
        let watcher = if config.watch {
            let dir = state
                .allow
                .last()
                .cloned()
                .unwrap_or_else(|| state.root.clone());
            Some(file_watcher::start(state.clone(), &dir)?)
        } else {
            None
        };
        Ok(Self {
            state,
            _watcher: watcher,
        })
    }

    /// Serves pages, modules, the HMR socket and the devermind runtime assets.
    pub fn router(&self) -> Router {
        router(self.state.clone())
    }

    /// Shared state (module graph, resolver).
    pub fn state(&self) -> &Arc<State> {
        &self.state
    }

    /// Treats `files` as changed, exactly as the watcher does.
    pub fn invalidate(&self, files: &[PathBuf]) {
        hot_reload::on_change(&self.state, files);
    }

    /// Sends `data` on the app channel `channel` to every connected page.
    pub fn publish(&self, channel: &str, data: serde_json::Value) {
        hot_reload::publish(&self.state, channel, data);
    }

    /// App channel messages pages send, from now on.
    pub fn messages(&self) -> broadcast::Receiver<AppMessage> {
        self.state.messages.subscribe()
    }
}

fn router(state: Arc<State>) -> Router {
    Router::new()
        .route(
            "/@devermind/client.js",
            get(|AxumState(s): AxumState<Arc<State>>| async move { js(s.client_module()) }),
        )
        .route(
            "/@devermind/refresh-runtime.js",
            get(|| async { js(REFRESH_RUNTIME_JS.to_string()) }),
        )
        .route(
            "/@devermind/refresh-preamble.js",
            get(|| async { js(REFRESH_PREAMBLE_JS.to_string()) }),
        )
        .route(
            HMR_PATH,
            get(
                |AxumState(s): AxumState<Arc<State>>,
                 ws: Result<
                    WebSocketUpgrade,
                    axum::extract::ws::rejection::WebSocketUpgradeRejection,
                >| async move {
                    // A plain GET is the client's restart ping: any answer means the server is up.
                    match ws {
                        Ok(ws) => ws
                            .on_upgrade(move |socket| hot_reload::socket(s, socket))
                            .into_response(),
                        Err(_) => (StatusCode::OK, "devermind").into_response(),
                    }
                },
            ),
        )
        .fallback(serve)
        .with_state(state)
}

/// Every other GET: modules, pages, static files, SPA fallback.
async fn serve(AxumState(state): AxumState<Arc<State>>, request: Request<Body>) -> Response {
    let uri = request.uri().clone();
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or("").to_string();
    let worker = state.clone();
    // A dedicated thread with the compiler's stack: an embedding host's blocking pool may use the
    // default stack size, which deep module ASTs overflow.
    let (tx, rx) = oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("devermind-compile".into())
        .stack_size(oj_compiler::COMPILE_STACK_SIZE)
        .spawn(move || {
            let _ = tx.send(respond(&worker, &path, &query));
        });
    if let Err(e) = spawned {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    match rx.await {
        Ok(response) => response,
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Serves project/workspace files or exact imports previously authorized by resolution.
/// Canonical targets are rechecked before external access, preventing a replaced
/// symlink from turning a registered dependency into a directory-wide file grant.
fn respond(state: &State, path: &str, query: &str) -> Response {
    // `public/` is served at `/` as is, where `build` copies it; `?import` stays a module request.
    let public = state.root.join("public");
    if !module_urls::has_flag(query, "import") {
        if let Some(file) = module_urls::file_of(&public, std::slice::from_ref(&public), path)
            .filter(|f| f.is_file())
        {
            return match std::fs::read(&file) {
                Ok(bytes) => with_type(bytes, module_urls::mime(&module_urls::ext(path))),
                Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
            };
        }
    }
    let registered = || {
        let files = state.resolved_files.lock().unwrap();
        let file = files.get(path)?;
        // Do not follow a file replaced by a symlink to a different target.
        (file.canonicalize().ok()?.as_path() == file.as_path()).then(|| file.clone())
    };
    let Some(mut file) = module_urls::file_of(&state.root, &state.allow, path).or_else(registered)
    else {
        return (
            StatusCode::FORBIDDEN,
            format!("{path} is outside the served directories"),
        )
            .into_response();
    };
    if file.is_dir() {
        file = file.join("index.html");
    }
    if !file.is_file() {
        // SPA fallback: an extensionless path is a client route.
        if !module_urls::ext(path).is_empty() {
            return (StatusCode::NOT_FOUND, format!("{path} not found")).into_response();
        }
        file = state.root.join("index.html");
    }
    if module_urls::ext(&file.to_string_lossy()) == "html" {
        return match std::fs::read_to_string(&file) {
            Ok(text) => with_type(
                html_entry::transform(&text, state.shell_hot_url().is_some()),
                "text/html; charset=utf-8",
            ),
            Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
        };
    }
    if module_pipeline::is_module_request(path, query) {
        return match module_pipeline::load(state, path, query, &file) {
            Ok(module) => with_type(module.code, module.content_type),
            Err(message) => {
                hot_reload::send_error(state, &message);
                (StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
            }
        };
    }
    match std::fs::read(&file) {
        Ok(bytes) => with_type(bytes, module_urls::mime(&module_urls::ext(path))),
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

fn js(body: String) -> Response {
    with_type(body, "text/javascript")
}

fn with_type(body: impl Into<Body>, content_type: &'static str) -> Response {
    let mut response = Response::new(body.into());
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// The nearest ancestor whose `package.json` declares `workspaces`, else `root`.
fn workspace_root(root: &Path) -> PathBuf {
    root.ancestors()
        .find(|dir| {
            std::fs::read_to_string(dir.join("package.json"))
                .is_ok_and(|pkg| pkg.contains("\"workspaces\""))
        })
        .unwrap_or(root)
        .to_path_buf()
}

#[cfg(test)]
mod vite_tests {
    use super::*;
    #[test]
    fn explicit_default_host_and_port_override_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("vite.config.js"),
            "export default {server:{host:true,port:6123},define:{__KEY__:'1'}}",
        )
        .unwrap();
        let mut config = Config::new(dir.path());
        config.set_host("127.0.0.1".into());
        config.set_port(5173);
        config.define.push(("__KEY__".into(), "2".into()));
        let config = config.with_vite().unwrap();
        assert_eq!(config.port, 5173);
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.define.last(), Some(&("__KEY__".into(), "2".into())));
    }
}
