//! The HostApi area `fs` of the desktop host: text files under the app's `fs`
//! roots, for the WebView (through the Runtime's dispatcher in `js/desktop.js`)
//! and for backend code alike (`lunarstorm:hostapi/fs`).
//!
//! Decisions:
//! - The module only forwards to the glue's `work('fs.<function>', args)`; the
//!   desktop native `desktopWork` runs [`run`] on a worker thread ([`Workers`])
//!   and settles the promise through the backend's loop. Every path is checked
//!   here with the host's own [`crate::within`], so the scope is enforced in
//!   Rust, never in JS.
//! - Defect fixed (review #5): the functions ran on the backend thread, so a FIFO
//!   (`open` blocks until a writer appears) or a hung network mount stopped the
//!   whole backend (test `a_fifo_in_a_root_does_not_block_the_backend`).
//!   Decision D2: worker threads, at most [`MAX_WORKERS`] per backend, behind a
//!   queue of [`MAX_QUEUED`]; one more job gets `EAGAIN`. A hung mount ties up
//!   worker threads, never the backend.
//!   Rejected: synchronous with checks only (bounds local files, not a hung mount).
//! - A relative path resolves against the first root; with no root, every path
//!   is refused.
//! - Errors cross as JSON `{code, message}` and reject with an `Error` carrying a
//!   Node-style `code`: `EINVAL` for a bad argument or a file that is not a
//!   regular file, `EACCES` outside the roots or for a hard-linked file,
//!   `ENOENT`, `ENOTDIR`, `E2BIG` over [`MAX_TEXT`], `EAGAIN` with the queue
//!   full, otherwise `EIO`.

use lunar_extension_hostapi::{Arg, Area, Function};
use lunarstorm_runtime::vm::Vm;
use lunarstorm_runtime::{Bindings, Use};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// The area's module.
pub const MODULE: &str = "lunarstorm:hostapi/fs";

/// Most bytes one `readText` returns.
///
/// Defect fixed (review #3): a file of any size was read whole into the
/// backend and the result (test `a_file_over_the_text_limit_is_refused`).
/// Decision D6: 16 MiB, read through `take(MAX_TEXT + 1)`, so a file that grows
/// while it is read still ends in `E2BIG`. A `metadata()` size check first was
/// rejected: it races a growing file.
pub const MAX_TEXT: usize = 16 << 20;

/// Most `fs` jobs one backend runs at a time (decision D2).
pub const MAX_WORKERS: usize = 8;

/// Most `fs` jobs one backend keeps waiting for a thread; one more gets
/// `EAGAIN`. Above the ui's 64 calls in flight per form, so a page never sees it
/// while the threads make progress.
pub const MAX_QUEUED: usize = 256;

/// The module: each function forwards its arguments to a worker.
const SOURCE: &str = "import { work } from 'lunarstorm:desktop';\nexport const readText = (...args) => work('fs.readText', args);\nexport const writeText = (...args) => work('fs.writeText', args);\nexport const list = (...args) => work('fs.list', args);\n";

/// A required, single argument.
///
/// # Arguments
/// * `name` - the argument's name.
///
/// # Returns
/// The spec entry.
const fn required(name: &'static str) -> Arg {
    Arg { name, optional: false, multiple: false }
}

/// The filesystem area. Its scope is not here but in the backend's `desktopWork`
/// native, which holds the app's `fs` roots, so nothing a manifest says can
/// widen it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Fs;

/// A failed `fs` job: a Node-style code and the message.
type Coded = (&'static str, String);

/// The JSON a failed job settles with.
///
/// # Arguments
/// * `code` - the Node-style code.
/// * `message` - what went wrong.
///
/// # Returns
/// `{"code": .., "message": ..}` as text.
pub fn failure(code: &str, message: &str) -> String {
    json!({ "code": code, "message": format!("{code}: {message}") }).to_string()
}

/// Names an I/O error by its Node-style code.
///
/// # Arguments
/// * `error` - what the OS reported.
/// * `path` - the path, for the message.
///
/// # Returns
/// The coded error. `ENXIO` (a FIFO opened for writing with no reader) and
/// `EISDIR` are `EINVAL`: the path is not a regular file.
fn io(error: &std::io::Error, path: &Path) -> Coded {
    let code = match (error.kind(), error.raw_os_error()) {
        (std::io::ErrorKind::NotFound, _) => "ENOENT",
        (std::io::ErrorKind::PermissionDenied, _) => "EACCES",
        (_, Some(libc::ENOTDIR)) => "ENOTDIR",
        (_, Some(libc::ENXIO | libc::EISDIR)) => "EINVAL",
        _ => "EIO",
    };
    (code, format!("{error}, '{}'", path.display()))
}

/// Reads a string argument.
///
/// # Arguments
/// * `args` - the call's arguments.
/// * `index` - the argument's position.
/// * `what` - its name, for the message.
///
/// # Returns
/// The text.
///
/// # Errors
/// `EINVAL` when it is missing or not a string.
fn string<'a>(args: &'a [Value], index: usize, what: &str) -> Result<&'a str, Coded> {
    args.get(index).and_then(Value::as_str).ok_or_else(|| ("EINVAL", format!("{what} must be a string")))
}

/// Resolves and checks the path argument (argument 0).
///
/// # Arguments
/// * `args` - the call's arguments.
/// * `roots` - the area's roots, canonical.
///
/// # Returns
/// The path to operate on.
///
/// # Errors
/// `EINVAL` for a missing, empty or NUL-carrying path; `EACCES` when it lies
/// outside the roots.
fn path(args: &[Value], roots: &[PathBuf]) -> Result<PathBuf, Coded> {
    let text = string(args, 0, "path")?;
    if text.is_empty() || text.contains('\0') {
        return Err(("EINVAL", String::from("path must be a non-empty string")));
    }
    let path = match roots.first() {
        _ if Path::new(text).is_absolute() => PathBuf::from(text),
        Some(base) => base.join(text),
        None => PathBuf::from(text),
    };
    if !crate::within(roots, &path.to_string_lossy()) {
        return Err(("EACCES", format!("'{text}' is outside the app's roots")));
    }
    Ok(path)
}

/// Opens a regular file without ever blocking in `open`.
///
/// Defect fixed (review #5): `read_to_string` on a FIFO blocked in `open` until
/// a writer appeared (test `a_fifo_in_a_root_does_not_block_the_backend`).
/// Decision D3: open with `O_NONBLOCK`, then `fstat` the open handle and require
/// a regular file; `metadata()` then `open()` was rejected as racy, and its
/// `open` still blocks. Regular files ignore `O_NONBLOCK`, so it stays set.
///
/// Defect fixed (D4): a hard link inside a root to a file outside it passed
/// every path check (test `a_hard_link_inside_a_root_is_refused`). Decision: an
/// existing file with more than one link is refused, from the same `fstat`.
///
/// # Arguments
/// * `path` - the checked path.
/// * `write` - open for writing, creating the file; otherwise for reading.
///
/// # Returns
/// The open file.
///
/// # Errors
/// `EINVAL` for anything but a regular file, `EACCES` for a hard-linked file,
/// otherwise the I/O error's code.
fn open(path: &Path, write: bool) -> Result<File, Coded> {
    let mut options = OpenOptions::new();
    if write {
        options.write(true).create(true);
    } else {
        options.read(true);
    }
    let file = options.custom_flags(libc::O_NONBLOCK).open(path).map_err(|error| io(&error, path))?;
    let meta = file.metadata().map_err(|error| io(&error, path))?;
    if !meta.is_file() {
        return Err(("EINVAL", format!("'{}' is not a regular file", path.display())));
    }
    if meta.nlink() > 1 {
        return Err(("EACCES", format!("'{}' has more than one link", path.display())));
    }
    Ok(file)
}

/// `readText(path)`.
///
/// # Arguments
/// * `path` - the checked path.
///
/// # Returns
/// The text.
///
/// # Errors
/// See [`open`]; `E2BIG` over [`MAX_TEXT`]; `EIO` for text that is not UTF-8.
fn read_text(path: &Path) -> Result<Value, Coded> {
    let mut bytes = Vec::new();
    open(path, false)?.take(MAX_TEXT as u64 + 1).read_to_end(&mut bytes).map_err(|error| io(&error, path))?;
    if bytes.len() > MAX_TEXT {
        return Err(("E2BIG", format!("'{}' is larger than {MAX_TEXT} bytes", path.display())));
    }
    String::from_utf8(bytes).map(Value::String).map_err(|_| ("EIO", format!("'{}' is not UTF-8", path.display())))
}

/// `writeText(path, text)`: the checks of [`open`] run before the file is
/// truncated, so a refused file is left as it was.
///
/// # Arguments
/// * `path` - the checked path.
/// * `text` - the new content.
///
/// # Returns
/// `null`.
///
/// # Errors
/// See [`open`].
fn write_text(path: &Path, text: &str) -> Result<Value, Coded> {
    let mut file = open(path, true)?;
    file.set_len(0).and_then(|()| file.write_all(text.as_bytes())).map_err(|error| io(&error, path))?;
    Ok(Value::Null)
}

/// `list(path)`.
///
/// # Arguments
/// * `path` - the checked path.
///
/// # Returns
/// The names in the directory, sorted.
///
/// # Errors
/// `ENOTDIR` for anything but a directory, otherwise the I/O error's code.
fn list(path: &Path) -> Result<Value, Coded> {
    let entries = std::fs::read_dir(path).map_err(|error| io(&error, path))?;
    let mut names: Vec<String> = entries.filter_map(Result::ok).map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    Ok(json!(names))
}

/// Runs one `fs` job; called on a worker thread.
///
/// # Arguments
/// * `roots` - the area's roots, canonical.
/// * `function` - `readText`, `writeText` or `list`.
/// * `args` - the arguments as a JSON array.
///
/// # Returns
/// The value as JSON text.
///
/// # Errors
/// The failure as JSON `{code, message}` ([`failure`]).
pub fn run(roots: &[PathBuf], function: &str, args: &str) -> Result<String, String> {
    let outcome = serde_json::from_str::<Vec<Value>>(args).map_err(|error| ("EINVAL", format!("bad arguments: {error}"))).and_then(|args| {
        let path = path(&args, roots)?;
        match function {
            "readText" => read_text(&path),
            "writeText" => write_text(&path, string(&args, 1, "text")?),
            "list" => list(&path),
            other => Err(("EINVAL", format!("no function fs.{other}"))),
        }
    });
    outcome.map(|value| value.to_string()).map_err(|(code, message)| failure(code, &message))
}

/// One `fs` job: it delivers its own result.
type Job = Box<dyn FnOnce() + Send>;

/// The state the worker threads share.
#[derive(Default)]
struct Pool {
    /// Threads running now.
    busy: usize,
    /// Jobs waiting for a thread.
    queue: VecDeque<Job>,
}

/// The worker threads of one backend: at most [`MAX_WORKERS`] named threads,
/// which drain a queue of at most [`MAX_QUEUED`] waiting jobs.
///
/// Defect fixed: the first version refused a job outright once 8 ran, so a page
/// reading 20 files at once got `EAGAIN` for 12 of them (test
/// `a_page_cannot_queue_more_than_max_calls`, which saw `EAGAIN:15,ok:50`).
/// Decision: queue behind the 8 threads; refuse only past [`MAX_QUEUED`]. A hung
/// mount still holds at most 8 threads; the jobs queued behind them end at the
/// host's call deadline (`ETIMEDOUT`).
#[derive(Clone, Default)]
pub struct Workers {
    /// The threads' shared state.
    pool: Arc<Mutex<Pool>>,
}

/// Runs jobs on one worker thread until the queue is empty, then frees the
/// thread's slot.
///
/// # Arguments
/// * `pool` - the shared state.
/// * `first` - the job the thread was started for.
fn drain(pool: &Mutex<Pool>, first: Job) {
    let mut job = Some(first);
    while let Some(next) = job {
        // A panicking job loses its own result only, never the thread's slot.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(next));
        let mut state = pool.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        job = state.queue.pop_front();
        if job.is_none() {
            state.busy -= 1;
        }
    }
}

impl Workers {
    /// Runs a job on a worker thread: a new one while fewer than
    /// [`MAX_WORKERS`] run, otherwise after the queued jobs before it.
    ///
    /// # Arguments
    /// * `job` - the work; it delivers its own result.
    ///
    /// # Returns
    /// Whether it was taken; `false` (the queue is full, or no thread could be
    /// started) means the caller answers `EAGAIN`.
    pub fn start(&self, job: impl FnOnce() + Send + 'static) -> bool {
        let mut state = self.pool.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.busy >= MAX_WORKERS {
            if state.queue.len() >= MAX_QUEUED {
                return false;
            }
            state.queue.push_back(Box::new(job));
            return true;
        }
        state.busy += 1;
        drop(state);
        let pool = self.pool.clone();
        let spawned = std::thread::Builder::new().name("lunarstorm-fs".into()).spawn(move || drain(&pool, Box::new(job)));
        if spawned.is_err() {
            self.pool.lock().unwrap_or_else(std::sync::PoisonError::into_inner).busy -= 1;
        }
        spawned.is_ok()
    }
}

impl<V: Vm> Area<V> for Fs {
    const NAME: &'static str = "fs";

    /// The functions the module exports, for the HostApi spec.
    const FUNCTIONS: &'static [Function] = &[
        Function { name: "readText", summary: "Reads a UTF-8 file under the app's roots.", args: &[required("path")] },
        Function { name: "writeText", summary: "Writes a UTF-8 file under the app's roots.", args: &[required("path"), required("text")] },
        Function { name: "list", summary: "Lists the names in a directory under the app's roots, sorted.", args: &[required("path")] },
    ];

    /// Builds the module.
    ///
    /// # Arguments
    /// * `_using` - unused: the roots come from the host, not the manifest.
    ///
    /// # Returns
    /// `lunarstorm:hostapi/fs`.
    fn bindings(&self, _using: &Use<'_>) -> Result<Bindings<V>, String> {
        Ok(Bindings::<V>::new().module(MODULE, SOURCE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, Sender};
    use std::time::Duration;

    /// Starts a job that blocks until its channel is dropped or sent to.
    ///
    /// # Arguments
    /// * `workers` - the pool.
    ///
    /// # Returns
    /// Whether it started, and the sender that releases it.
    fn blocked(workers: &Workers) -> (bool, Sender<()>) {
        let (release, wait) = channel::<()>();
        (workers.start(move || { let _ = wait.recv(); }), release)
    }

    #[test]
    fn start_returns_while_its_job_is_still_running() {
        let workers = Workers::default();
        let (ran, seen) = channel();
        let (release, wait) = channel::<()>();
        assert!(workers.start(move || {
            let _ = wait.recv();
            let _ = ran.send(());
        }));
        assert!(seen.recv_timeout(Duration::from_millis(50)).is_err(), "the job is still blocked");
        drop(release);
        seen.recv_timeout(Duration::from_secs(5)).expect("the job finished once released");
    }

    #[test]
    fn jobs_beyond_max_workers_wait_and_beyond_the_queue_are_refused() {
        let workers = Workers::default();
        let releases: Vec<_> = (0..MAX_WORKERS + MAX_QUEUED).map(|_| blocked(&workers)).map(|(taken, release)| {
            assert!(taken, "every job up to the queue's capacity is taken");
            release
        }).collect();
        let (taken, _spare) = blocked(&workers);
        assert!(!taken, "a job past the queue must be refused");
        // A queued job runs once the threads are released.
        let (ran, seen) = channel();
        drop(releases);
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while !workers.start({ let ran = ran.clone(); move || { let _ = ran.send(()); } }) {
            assert!(std::time::Instant::now() < until, "the queue did not drain after release");
            std::thread::yield_now();
        }
        seen.recv_timeout(Duration::from_secs(5)).expect("a job taken after release ran");
    }

    #[test]
    fn failures_carry_their_code_as_json() {
        let value: Value = serde_json::from_str(&run(&[], "readText", "[1]").expect_err("refused")).expect("JSON");
        assert_eq!(value["code"], "EINVAL");
        let value: Value = serde_json::from_str(&failure("EAGAIN", "busy")).expect("JSON");
        assert_eq!(value["code"], "EAGAIN");
    }
}
