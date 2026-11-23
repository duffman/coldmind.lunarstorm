//! Native services registered into the Runtime (spec 6): app, window, screen, menu,
//! tray, dialog, path, clipboard, notification, os, shell, process.
//!
//! Each call runs where it may:
//!
//! | namespaces                                   | runs on                         |
//! |----------------------------------------------|---------------------------------|
//! | `app`, `window`, `screen`, `menu`, `tray`, `dialog` | UI thread (OS requirement) |
//! | `path`, `os`                                 | JS thread, inline (no blocking) |
//! | `clipboard`, `notification`, `shell`, `process` | worker pool (blocking calls) |
//!
//! Processes a backend starts are tagged with its epoch and killed when that epoch is
//! retired (6.2). Timers die with their Instance; no other service holds native
//! resources across calls.

use crate::js::{NativeCall, NativeRouter};
use crate::manifest::HostConfig;
use crossbeam_channel::{unbounded, Sender};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// Worker threads for blocking natives.
const POOL_THREADS: usize = 4;

/// Output kept from `process.exec`, per stream.
const MAX_OUTPUT: usize = 8 << 20;

/// A job for the worker pool.
type Job = Box<dyn FnOnce() + Send>;

/// Fixed-size pool for blocking native calls.
pub struct WorkerPool {
    /// Job queue.
    tx: Sender<Job>,
}

impl WorkerPool {
    /// Starts the threads.
    ///
    /// # Arguments
    /// * `threads` - number of workers.
    ///
    /// # Returns
    /// The pool.
    pub fn new(threads: usize) -> WorkerPool {
        let (tx, rx) = unbounded::<Job>();
        for i in 0..threads {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("zebra-worker-{i}"))
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        // A panicking service must not take a worker down with it.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                    }
                })
                .expect("spawn worker");
        }
        WorkerPool { tx }
    }

    /// Queues a job.
    ///
    /// # Arguments
    /// * `job` - the work.
    pub fn spawn(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.tx.send(Box::new(job));
    }
}

/// A process started by a backend, shared between its waiter and epoch release.
type Tracked = Arc<Mutex<Child>>;

/// Processes per epoch: epoch -> (pid, process).
type ProcessTable = Arc<Mutex<HashMap<u64, Vec<(u32, Tracked)>>>>;

/// The router the JS thread uses.
pub struct NativeServices {
    /// Hands UI-thread calls to the event loop.
    ui: Box<dyn Fn(NativeCall) + Send + Sync>,
    /// Blocking calls.
    pool: WorkerPool,
    /// Current host configuration (executables, environment, filesystem roots).
    config: Arc<RwLock<Arc<HostConfig>>>,
    /// Processes per epoch.
    processes: ProcessTable,
}

impl NativeServices {
    /// Creates the router.
    ///
    /// # Arguments
    /// * `ui` - forwards a call to the UI thread.
    /// * `config` - shared host configuration.
    ///
    /// # Returns
    /// The router.
    pub fn new(ui: Box<dyn Fn(NativeCall) + Send + Sync>, config: Arc<RwLock<Arc<HostConfig>>>) -> NativeServices {
        NativeServices { ui, pool: WorkerPool::new(POOL_THREADS), config, processes: Arc::default() }
    }
}

impl NativeRouter for NativeServices {
    /// Routes a permitted call.
    ///
    /// # Arguments
    /// * `call` - the call.
    fn call(&self, call: NativeCall) {
        match call.ns.as_str() {
            "app" | "window" | "screen" | "menu" | "tray" | "dialog" => (self.ui)(call),
            "path" => {
                let result = path(&call.method);
                call.done.complete(result);
            }
            "os" => {
                let config = self.config.read().expect("config lock").clone();
                let result = os(&call.method, &call.args, &config);
                call.done.complete(result);
            }
            "clipboard" | "notification" | "shell" | "process" => {
                let config = self.config.read().expect("config lock").clone();
                let processes = self.processes.clone();
                self.pool.spawn(move || {
                    let NativeCall { ns, method, args, done } = call;
                    let result = match ns.as_str() {
                        "clipboard" => clipboard(&method, &args),
                        "notification" => notification(&method, &args, &config),
                        "shell" => shell(&method, &args, &config),
                        _ => process(&method, &args, &config, done.epoch, &processes),
                    };
                    done.complete(result);
                });
            }
            other => call.done.complete(Err(format!("unknown native namespace {other}"))),
        }
    }

    /// Kills every process the retired backend started.
    ///
    /// # Arguments
    /// * `epoch` - retired epoch.
    fn release_epoch(&self, epoch: u64) {
        let tracked = self.processes.lock().expect("process table").remove(&epoch).unwrap_or_default();
        for (_, child) in tracked {
            let mut child = child.lock().expect("child lock");
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The `n`-th argument as a string.
///
/// # Arguments
/// * `args` - call arguments.
/// * `n` - index.
///
/// # Returns
/// The string.
///
/// # Errors
/// Missing or not a string.
pub fn arg_str(args: &[Value], n: usize) -> Result<&str, String> {
    args.get(n).and_then(Value::as_str).ok_or_else(|| format!("argument {} must be a string", n + 1))
}

/// `path.<name>()`: well-known directories.
///
/// # Arguments
/// * `method` - directory name.
///
/// # Returns
/// The path as a string, or `null` when the OS has none.
///
/// # Errors
/// Unknown method.
fn path(method: &str) -> Result<Value, String> {
    let dir = match method {
        "home" => dirs::home_dir(),
        "documents" => dirs::document_dir(),
        "downloads" => dirs::download_dir(),
        "desktop" => dirs::desktop_dir(),
        "pictures" => dirs::picture_dir(),
        "music" => dirs::audio_dir(),
        "videos" => dirs::video_dir(),
        "config" => dirs::config_dir(),
        "data" => dirs::data_dir(),
        "cache" => dirs::cache_dir(),
        "temp" => Some(std::env::temp_dir()),
        "executable" => std::env::current_exe().ok(),
        other => return Err(format!("unknown method path.{other}")),
    };
    Ok(dir.map(|d| Value::String(d.to_string_lossy().into_owned())).unwrap_or(Value::Null))
}

/// `os.info()` and `os.env(name)`.
///
/// # Arguments
/// * `method` - method name.
/// * `args` - arguments.
/// * `config` - host configuration (environment permissions).
///
/// # Returns
/// The result.
///
/// # Errors
/// Unknown method, or an environment name not in `<permissions>`.
fn os(method: &str, args: &[Value], config: &HostConfig) -> Result<Value, String> {
    match method {
        "info" => Ok(json!({
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "family": std::env::consts::FAMILY,
            "locale": std::env::var("LANG").ok(),
        })),
        "env" => {
            let name = arg_str(args, 0)?;
            if !config.permissions.env.iter().any(|n| n == name) {
                return Err(format!("environment variable {name} is not permitted"));
            }
            Ok(std::env::var(name).map(Value::String).unwrap_or(Value::Null))
        }
        other => Err(format!("unknown method os.{other}")),
    }
}

/// `clipboard.readText()` / `clipboard.writeText(text)`.
///
/// # Arguments
/// * `method` - method name.
/// * `args` - arguments.
///
/// # Returns
/// The text read, or `null`.
///
/// # Errors
/// Clipboard unavailable, or unknown method.
fn clipboard(method: &str, args: &[Value]) -> Result<Value, String> {
    let mut board = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    match method {
        "readText" => Ok(board.get_text().map(Value::String).unwrap_or(Value::Null)),
        "writeText" => board.set_text(arg_str(args, 0)?.to_owned()).map(|_| Value::Null).map_err(|e| e.to_string()),
        other => Err(format!("unknown method clipboard.{other}")),
    }
}

/// `notification.show({title, body})`.
///
/// # Arguments
/// * `method` - method name.
/// * `args` - arguments.
/// * `config` - host configuration (application name).
///
/// # Returns
/// `null`.
///
/// # Errors
/// The OS refused, or unknown method.
fn notification(method: &str, args: &[Value], config: &HostConfig) -> Result<Value, String> {
    if method != "show" {
        return Err(format!("unknown method notification.{method}"));
    }
    let options = args.first().cloned().unwrap_or(Value::Null);
    let title = options.get("title").and_then(Value::as_str).unwrap_or(&config.application.name);
    let body = options.get("body").and_then(Value::as_str).unwrap_or("");
    notify_rust::Notification::new()
        .appname(&config.application.name)
        .summary(title)
        .body(body)
        .show()
        .map(|_| Value::Null)
        .map_err(|e| e.to_string())
}

/// Whether a path lies under one of the `<permissions>` filesystem roots.
///
/// # Arguments
/// * `path` - candidate path.
/// * `config` - host configuration.
///
/// # Returns
/// `true` when inside a root (after resolving `..` and symlinks).
fn under_fs_root(path: &Path, config: &HostConfig) -> bool {
    let Ok(real) = std::fs::canonicalize(path) else { return false };
    config.permissions.fs.iter().any(|root| {
        let root = root.strip_prefix("~/").map(|r| dirs::home_dir().unwrap_or_default().join(r)).unwrap_or_else(|| root.into());
        std::fs::canonicalize(root).is_ok_and(|root| real.starts_with(root))
    })
}

/// `shell.openUrl(url)` / `shell.openPath(path)`.
///
/// # Arguments
/// * `method` - method name.
/// * `args` - arguments.
/// * `config` - host configuration (filesystem roots).
///
/// # Returns
/// `null`.
///
/// # Errors
/// A scheme other than http, https or mailto; a path outside the roots; unknown method.
fn shell(method: &str, args: &[Value], config: &HostConfig) -> Result<Value, String> {
    let target = arg_str(args, 0)?;
    match method {
        "openUrl" => {
            let lower = target.to_ascii_lowercase();
            if !(lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")) {
                return Err("shell.openUrl only opens http, https and mailto URLs".into());
            }
            open::that_detached(target).map(|_| Value::Null).map_err(|e| e.to_string())
        }
        "openPath" => {
            if !under_fs_root(Path::new(target), config) {
                return Err(format!("{target} is outside the permitted filesystem roots"));
            }
            open::that_detached(target).map(|_| Value::Null).map_err(|e| e.to_string())
        }
        other => Err(format!("unknown method shell.{other}")),
    }
}

/// Whether `<permissions>` lists an executable, by exact path or file name.
///
/// # Arguments
/// * `program` - program as given by the backend.
/// * `config` - host configuration.
///
/// # Returns
/// `true` when permitted.
fn executable_allowed(program: &str, config: &HostConfig) -> bool {
    config.permissions.executables.iter().any(|allowed| {
        allowed == program || (!program.contains(['/', '\\']) && Path::new(allowed).file_name().is_some_and(|n| n == program))
    })
}

/// `process.exec(program, args, {cwd})`, `process.spawn(...)`, `process.kill(pid)`.
///
/// `exec` waits and returns `{code, stdout, stderr}`; `spawn` returns `{pid}` and the
/// process runs until it exits, is killed, or its epoch is retired.
///
/// # Arguments
/// * `method` - method name.
/// * `args` - arguments.
/// * `config` - host configuration (executables).
/// * `epoch` - caller epoch.
/// * `table` - processes per epoch.
///
/// # Returns
/// The result.
///
/// # Errors
/// Not permitted, failed to start, or unknown method.
fn process(
    method: &str,
    args: &[Value],
    config: &HostConfig,
    epoch: u64,
    table: &ProcessTable,
) -> Result<Value, String> {
    if method == "kill" {
        let pid = args.first().and_then(Value::as_u64).ok_or("argument 1 must be a pid")? as u32;
        let found = table.lock().expect("process table").get(&epoch).and_then(|list| list.iter().find(|(p, _)| *p == pid).cloned());
        // Only a process this epoch started can be killed through the API.
        let (_, child) = found.ok_or_else(|| format!("process {pid} was not started by this backend"))?;
        let killed = child.lock().expect("child lock").kill();
        return killed.map(|_| Value::Null).map_err(|e| e.to_string());
    }
    if method != "exec" && method != "spawn" {
        return Err(format!("unknown method process.{method}"));
    }
    let program = arg_str(args, 0)?;
    if !executable_allowed(program, config) {
        return Err(format!("executable {program} is not permitted"));
    }
    let argv: Vec<String> = match args.get(1) {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(items)) => items.iter().map(|v| v.as_str().map(str::to_owned).ok_or("arguments must be strings")).collect::<Result<_, _>>()?,
        Some(_) => return Err("argument 2 must be an array of strings".into()),
    };
    let mut command = Command::new(program);
    command.args(&argv).stdin(Stdio::null());
    if let Some(cwd) = args.get(2).and_then(|o| o.get("cwd")).and_then(Value::as_str) {
        command.current_dir(cwd);
    }
    let exec = method == "exec";
    if exec {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let mut child = command.spawn().map_err(|e| format!("{program}: {e}"))?;
    let pid = child.id();
    let readers = exec.then(|| (reader(child.stdout.take()), reader(child.stderr.take())));
    let child: Tracked = Arc::new(Mutex::new(child));
    table.lock().expect("process table").entry(epoch).or_default().push((pid, child.clone()));
    let Some((out, err)) = readers else {
        return Ok(json!({ "pid": pid }));
    };
    // Decision: the waiter polls `try_wait` instead of blocking in `wait`, so epoch release
    // can take the lock and kill the child while it runs.
    let status = loop {
        if let Some(status) = child.lock().expect("child lock").try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if let Some(list) = table.lock().expect("process table").get_mut(&epoch) {
        list.retain(|(p, _)| *p != pid);
    }
    let text = |r: std::thread::JoinHandle<Vec<u8>>| String::from_utf8_lossy(&r.join().unwrap_or_default()).into_owned();
    Ok(json!({ "code": status.code(), "stdout": text(out), "stderr": text(err) }))
}

/// Reads a child stream on its own thread, up to [`MAX_OUTPUT`] bytes.
///
/// # Arguments
/// * `stream` - the pipe.
///
/// # Returns
/// The reader thread.
fn reader<R: Read + Send + 'static>(stream: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = stream {
            let _ = s.take(MAX_OUTPUT as u64).read_to_end(&mut buf);
        }
        buf
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::js::tests::config_with;

    #[test]
    fn executables_need_a_permission() {
        let config = config_with(&[], &["/bin/echo"]);
        assert!(executable_allowed("/bin/echo", &config));
        assert!(executable_allowed("echo", &config));
        assert!(!executable_allowed("/usr/bin/echo", &config));
        assert!(!executable_allowed("sh", &config));
    }

    #[cfg(unix)]
    #[test]
    fn exec_runs_and_spawned_processes_die_with_their_epoch() {
        let config = config_with(&[], &["/bin/echo", "/bin/sleep"]);
        let table = Arc::default();
        let out = process("exec", &[json!("/bin/echo"), json!(["hi"])], &config, 1, &table).unwrap();
        assert_eq!(out["stdout"], "hi\n");
        assert!(process("exec", &[json!("/bin/sh")], &config, 1, &table).unwrap_err().contains("not permitted"));

        let services = NativeServices::new(Box::new(|_| {}), Arc::new(RwLock::new(Arc::new(config.clone()))));
        let spawned = process("spawn", &[json!("/bin/sleep"), json!(["30"])], &config, 7, &services.processes).unwrap();
        let pid = spawned["pid"].as_u64().unwrap() as u32;
        let child = services.processes.lock().unwrap()[&7][0].1.clone();
        services.release_epoch(7);
        let status = child.lock().unwrap().try_wait().unwrap();
        assert!(status.is_some(), "process {pid} still runs after its epoch was retired");
    }

    #[test]
    fn env_and_paths() {
        let mut config = config_with(&[], &[]);
        config.permissions.env.push("HOME".into());
        assert!(os("env", &[json!("HOME")], &config).is_ok());
        assert!(os("env", &[json!("PATH")], &config).is_err());
        assert!(path("temp").unwrap().is_string());
        assert!(path("nope").is_err());
    }

    #[test]
    fn shell_refuses_other_schemes() {
        let config = config_with(&[], &[]);
        assert!(shell("openUrl", &[json!("file:///etc/passwd")], &config).is_err());
        assert!(shell("openPath", &[json!("/etc")], &config).is_err());
    }
}
