//! Subprocess bridge to the pinned QuickJS-NG reference executable.
//!
//! Run `sh scripts/build-reference.sh` from the workspace first. Every evaluation
//! creates a fresh engine process and evaluates one synchronous global script.
//! Returned strings preserve JavaScript UTF-16 code units, including lone
//! surrogates. Objects, functions, and symbols may be used inside scripts but
//! cannot cross the result boundary.

use std::error::Error;
use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};

pub const QUICKJS_REVISION: &str = "e22ce4de4362dc5fd4794cfa3c03fc7fdba5f015";

/// A JavaScript primitive. `Number` preserves negative zero and non-finite values.
/// As with `f64`, NaN does not compare equal to itself.
#[derive(Clone, Debug, PartialEq)]
pub enum ReferenceValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    BigInt(String),
    String(Vec<u16>),
}

#[derive(Debug)]
pub enum ReferenceError {
    Io(io::Error),
    Execution {
        status: ExitStatus,
        stderr: String,
    },
    Protocol(String),
    /// External process deadline, including blocked input/output.
    Timeout {
        limit: std::time::Duration,
    },
    /// Input or combined captured output exceeded its budget.
    Capacity {
        resource: &'static str,
        limit: usize,
    },
}

impl fmt::Display for ReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "reference process I/O: {error}"),
            Self::Execution { status, stderr } => {
                write!(
                    formatter,
                    "reference process {status}: {}",
                    stderr.trim_end()
                )
            }
            Self::Protocol(message) => write!(formatter, "reference protocol: {message}"),
            Self::Timeout { limit } => write!(
                formatter,
                "reference process exceeded external deadline {limit:?}"
            ),
            Self::Capacity { resource, limit } => write!(
                formatter,
                "reference {resource} exceeds capacity {limit} bytes"
            ),
        }
    }
}

impl Error for ReferenceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ReferenceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Debug)]
pub struct ReferenceRunner {
    executable: PathBuf,
    limits: Limits,
}

impl ReferenceRunner {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            limits: Limits::default(),
        }
    }

    /// Uses `QJS_REFERENCE` when set, otherwise the workspace build location.
    pub fn from_env() -> Self {
        let executable = std::env::var_os("QJS_REFERENCE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../build/qjs-reference")
            });
        Self::new(executable)
    }

    /// Configure external deadline and bounded input/output capture.
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Sends UTF-8 source over bounded stdin with a five-second default external deadline.
    pub fn evaluate(&self, source: &str) -> Result<ReferenceValue, ReferenceError> {
        if source.len() > self.limits.max_input_bytes {
            return Err(ReferenceError::Capacity {
                resource: "input",
                limit: self.limits.max_input_bytes,
            });
        }
        let mut command = self.command();
        command.arg("-");
        decode_output(execute_bounded(
            command,
            source.as_bytes().to_vec(),
            self.limits,
        )?)
    }

    /// Read a bounded file snapshot, preserving its canonical diagnostic filename.
    /// Source bytes are streamed to the reference through stdin, never reopened by it.
    pub fn evaluate_file(
        &self,
        source: impl AsRef<Path>,
    ) -> Result<ReferenceValue, ReferenceError> {
        let source = std::fs::canonicalize(source)?;
        // Open nonblocking before checking the handle, so replacement by a FIFO cannot hang.
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(nonblocking_flag()?)
                .open(&source)?
        };
        #[cfg(not(unix))]
        let file = std::fs::File::open(&source)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reference source must be a regular file",
            )
            .into());
        }
        let mut bytes = Vec::new();
        file.take(self.limits.max_input_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > self.limits.max_input_bytes {
            return Err(ReferenceError::Capacity {
                resource: "input",
                limit: self.limits.max_input_bytes,
            });
        }
        let mut command = self.command();
        command.arg("--source-name").arg(source);
        decode_output(execute_bounded(command, bytes, self.limits)?)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        command
    }
}

fn execution_error(output: Output) -> ReferenceError {
    ReferenceError::Execution {
        status: output.status,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn decode_output(output: Output) -> Result<ReferenceValue, ReferenceError> {
    if !output.status.success() {
        return Err(execution_error(output));
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| ReferenceError::Protocol("result is not UTF-8".into()))?;
    parse_value(text)
}

fn parse_value(text: &str) -> Result<ReferenceValue, ReferenceError> {
    let malformed = || ReferenceError::Protocol(format!("malformed result {text:?}"));
    let line = text.strip_suffix('\n').ok_or_else(malformed)?;
    match line {
        "undefined" => return Ok(ReferenceValue::Undefined),
        "null" => return Ok(ReferenceValue::Null),
        "boolean\ttrue" => return Ok(ReferenceValue::Boolean(true)),
        "boolean\tfalse" => return Ok(ReferenceValue::Boolean(false)),
        _ => {}
    }
    let (kind, payload) = line.split_once('\t').ok_or_else(malformed)?;
    match kind {
        "number" => {
            let value = match payload {
                "NaN" => f64::NAN,
                "Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                _ => payload.parse().map_err(|_| malformed())?,
            };
            Ok(ReferenceValue::Number(value))
        }
        "bigint" => {
            let digits = payload.strip_prefix('-').unwrap_or(payload);
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(malformed());
            }
            Ok(ReferenceValue::BigInt(payload.to_owned()))
        }
        "string" => {
            if payload.is_empty() {
                return Ok(ReferenceValue::String(Vec::new()));
            }
            let units = payload
                .split(',')
                .map(|unit| {
                    if unit.len() != 4 || !unit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                        return Err(malformed());
                    }
                    u16::from_str_radix(unit, 16).map_err(|_| malformed())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ReferenceValue::String(units))
        }
        _ => Err(malformed()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_incomplete_or_malformed_process_output() {
        for output in [
            "",
            "number\t42",
            "number\tnope\n",
            "number\t42\nextra\n",
            "bigint\t-\n",
            "string\td8000\n",
            "string\tzzzz\n",
            "boolean\t1\n",
            "object\t{}\n",
        ] {
            assert!(
                matches!(parse_value(output), Err(ReferenceError::Protocol(_))),
                "{output:?}"
            );
        }
    }
}

/// Bounded subprocess resources. Output is the combined stdout/stderr budget.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Completion must be observed before this deadline, including I/O collection.
    /// A zero budget rejects without spawning a process.
    pub timeout: std::time::Duration,
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: std::time::Duration::from_secs(5),
            max_input_bytes: 16 * 1024 * 1024,
            max_output_bytes: 16 * 1024 * 1024,
        }
    }
}
#[cfg(unix)]
fn nonblocking_flag() -> io::Result<i32> {
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        Ok(4)
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        Ok(2048)
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "linux",
        target_os = "android"
    )))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "nonblocking pipe flags unavailable on this platform",
        ))
    }
}
#[cfg(unix)]
fn set_nonblocking(fd: std::os::fd::RawFd) -> io::Result<()> {
    unsafe extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }
    let nonblock = nonblocking_flag()?;
    // SAFETY: fd is borrowed from a live owned pipe; F_GETFL/F_SETFL only update its status flags.
    unsafe {
        let flags = fcntl(fd, 3);
        if flags < 0 || fcntl(fd, 4, flags | nonblock) < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}
#[cfg(unix)]
fn kill_group(id: u32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    if let Ok(id) = i32::try_from(id) {
        // SAFETY: This positive ID belongs to the child created with process_group(0).
        // Negative pid selects only its inherited process group; SIGKILL needs no shared pointers.
        unsafe {
            kill(-id, 9);
        }
    }
}
#[cfg(unix)]
fn capture(
    mut pipe: impl io::Read,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    used: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    capacity: std::sync::Arc<std::sync::atomic::AtomicBool>,
    limit: usize,
) -> io::Result<Vec<u8>> {
    use std::sync::atomic::Ordering;
    let mut bytes = Vec::new();
    let mut buf = [0; 8192];
    while !stop.load(Ordering::Acquire) {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let previous = used.fetch_add(n, Ordering::AcqRel);
                if previous > limit || n > limit - previous {
                    capacity.store(true, Ordering::Release);
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(2))
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(bytes)
}
#[cfg(all(test, unix))]
thread_local! {static FAIL_SPAWN:std::cell::Cell<Option<usize>>=const {std::cell::Cell::new(None)};static DELAY_POLL:std::cell::Cell<Option<std::time::Duration>>=const {std::cell::Cell::new(None)};static LAST_CHILD:std::cell::Cell<u32>=const{std::cell::Cell::new(0)};}
#[cfg(unix)]
fn spawn_worker<T: Send + 'static>(
    job: impl FnOnce() -> T + Send + 'static,
) -> io::Result<std::thread::JoinHandle<T>> {
    #[cfg(test)]
    if FAIL_SPAWN.with(|v| match v.get() {
        Some(0) => {
            v.set(None);
            true
        }
        Some(n) => {
            v.set(Some(n - 1));
            false
        }
        None => false,
    }) {
        return Err(io::Error::other("injected worker spawn failure"));
    }
    std::thread::Builder::new().spawn(job)
}
#[cfg(unix)]
struct ProcessTasks {
    child: std::process::Child,
    group_cleaned: bool,
    reaped: bool,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    writer: Option<std::thread::JoinHandle<io::Result<()>>>,
    output: Option<std::thread::JoinHandle<io::Result<Vec<u8>>>>,
    errors: Option<std::thread::JoinHandle<io::Result<Vec<u8>>>>,
}
#[cfg(unix)]
impl Drop for ProcessTasks {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if !self.group_cleaned {
            kill_group(self.child.id());
        }
        if !self.reaped {
            let _ = self.child.wait();
        }
        if let Some(h) = self.writer.take() {
            let _ = h.join();
        }
        if let Some(h) = self.output.take() {
            let _ = h.join();
        }
        if let Some(h) = self.errors.take() {
            let _ = h.join();
        }
    }
}
#[cfg(unix)]
fn execute_bounded(
    mut command: Command,
    input: Vec<u8>,
    limits: Limits,
) -> Result<Output, ReferenceError> {
    use std::{
        os::{fd::AsRawFd, unix::process::CommandExt},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        },
        thread,
        time::{Duration, Instant},
    };
    if limits.timeout.is_zero() {
        return Err(ReferenceError::Timeout {
            limit: limits.timeout,
        });
    }
    command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let child = command.spawn()?;
    #[cfg(test)]
    LAST_CHILD.with(|p| p.set(child.id()));
    let stop = Arc::new(AtomicBool::new(false));
    let mut tasks = ProcessTasks {
        child,
        group_cleaned: false,
        reaped: false,
        stop: stop.clone(),
        writer: None,
        output: None,
        errors: None,
    };
    let child = &mut tasks.child;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    for fd in [stdin.as_raw_fd(), stdout.as_raw_fd(), stderr.as_raw_fd()] {
        if let Err(e) = set_nonblocking(fd) {
            return Err(e.into());
        }
    }
    let capacity = Arc::new(AtomicBool::new(false));
    let used = Arc::new(AtomicUsize::new(0));
    let writer_stop = stop.clone();
    tasks.writer = Some(spawn_worker(move || {
        let mut at = 0;
        while at < input.len() && !writer_stop.load(Ordering::Acquire) {
            match stdin.write(&input[at..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "reference stdin closed",
                    ))
                }
                Ok(n) => at += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2))
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        drop(stdin);
        Ok(())
    })?);
    let (a, b, c) = (stop.clone(), used.clone(), capacity.clone());
    tasks.output = Some(spawn_worker(move || {
        capture(stdout, a, b, c, limits.max_output_bytes)
    })?);
    let (a, b, c) = (stop.clone(), used.clone(), capacity.clone());
    tasks.errors = Some(spawn_worker(move || {
        capture(stderr, a, b, c, limits.max_output_bytes)
    })?);
    #[cfg(test)]
    if let Some(delay) = DELAY_POLL.with(|d| d.take()) {
        thread::sleep(delay);
    }
    let child = &mut tasks.child;
    let mut status = None;
    let mut failure = None;
    let mut descendants_killed = false;
    loop {
        if started.elapsed() >= limits.timeout {
            failure = Some(ReferenceError::Timeout {
                limit: limits.timeout,
            });
            break;
        }
        if capacity.load(Ordering::Acquire) {
            failure = Some(ReferenceError::Capacity {
                resource: "output",
                limit: limits.max_output_bytes,
            });
            break;
        }
        match child.try_wait() {
            Ok(Some(s)) => {
                status = Some(s);
                tasks.reaped = true;
            }
            Ok(None) => {}
            Err(e) => {
                failure = Some(e.into());
                break;
            }
        }
        if status.is_some()
            && tasks.writer.as_ref().unwrap().is_finished()
            && tasks.output.as_ref().unwrap().is_finished()
            && tasks.errors.as_ref().unwrap().is_finished()
        {
            // Observation is conservative: completion noticed at/after the budget is late.
            if started.elapsed() >= limits.timeout {
                failure = Some(ReferenceError::Timeout {
                    limit: limits.timeout,
                });
            }
            break;
        }
        // A completed leader must not leave descendants holding pipes open.
        if status.is_some() && !descendants_killed {
            kill_group(child.id());
            descendants_killed = true;
            tasks.group_cleaned = true;
        }
        if started.elapsed() >= limits.timeout {
            failure = Some(ReferenceError::Timeout {
                limit: limits.timeout,
            });
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    if failure.is_some() {
        stop.store(true, Ordering::Release);
    }
    if status.is_none() {
        kill_group(child.id());
        tasks.group_cleaned = true;
        match child.wait() {
            Ok(s) => {
                status = Some(s);
                tasks.reaped = true;
            }
            Err(e) => {
                if failure.is_none() {
                    failure = Some(e.into())
                }
            }
        }
    } else if !descendants_killed {
        kill_group(child.id());
        tasks.group_cleaned = true;
    }
    let join = |r: thread::Result<io::Result<Vec<u8>>>| {
        r.map_err(|_| io::Error::other("reference reader thread panicked"))
            .and_then(|r| r)
    };
    let stdout = join(tasks.output.take().unwrap().join());
    let stderr = join(tasks.errors.take().unwrap().join());
    let written = tasks
        .writer
        .take()
        .unwrap()
        .join()
        .map_err(|_| io::Error::other("reference writer thread panicked"))
        .and_then(|r| r);
    if let Some(e) = failure {
        return Err(e);
    }
    if capacity.load(Ordering::Acquire) {
        return Err(ReferenceError::Capacity {
            resource: "output",
            limit: limits.max_output_bytes,
        });
    }
    if started.elapsed() >= limits.timeout {
        return Err(ReferenceError::Timeout {
            limit: limits.timeout,
        });
    }
    let result = Output {
        status: status.expect("reaped child"),
        stdout: stdout?,
        stderr: stderr?,
    };
    if !result.status.success() {
        return Err(execution_error(result));
    }
    written?;
    Ok(result)
}
#[cfg(not(unix))]
fn execute_bounded(
    _command: Command,
    _input: Vec<u8>,
    _limits: Limits,
) -> Result<Output, ReferenceError> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "reference process-tree deadlines require POSIX process groups",
    )
    .into())
}

#[cfg(all(test, unix))]
mod spawn_failure_tests {
    use super::*;
    #[test]
    fn late_completion_observed_after_scheduler_delay_is_timeout() {
        DELAY_POLL.with(|v| v.set(Some(std::time::Duration::from_millis(200))));
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 'number\\t42\\n'"]);
        let result = execute_bounded(
            command,
            Vec::new(),
            Limits {
                timeout: std::time::Duration::from_millis(20),
                ..Limits::default()
            },
        );
        assert!(
            matches!(result, Err(ReferenceError::Timeout { .. })),
            "late observed completion must time out: {result:?}"
        );
    }
    #[test]
    fn every_worker_spawn_failure_reaps_child_and_joins_started_workers() {
        for stage in 0..3 {
            FAIL_SPAWN.with(|v| v.set(Some(stage)));
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "sleep 20"]);
            let started = std::time::Instant::now();
            let error = execute_bounded(command, vec![b'x'; 2 * 1024 * 1024], Limits::default())
                .unwrap_err();
            assert!(error.to_string().contains("injected worker spawn failure"));
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
            unsafe extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            let pid = LAST_CHILD.with(|v| v.get());
            // SAFETY: Signal zero only queries the just-owned child PID; no signal is delivered.
            assert_eq!(
                unsafe { kill(pid as i32, 0) },
                -1,
                "child must be reaped at spawn stage {stage}"
            );
        }
    }
}
