//! External cancellation/deadline boundary around the disposable Rust compiler.
//! The executable is trusted. Process isolation contains parser crashes and lets
//! the owner terminate compilation without interrupting an arena-owning thread.
use crate::{
    job_protocol as protocol,
    job_request::{self, Budgets, Mode},
};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub source_bytes: usize,
    pub artifact_bytes: usize,
    pub heap_bytes: usize,
    pub deadline: Duration,
    pub budgets: Budgets,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            source_bytes: protocol::MAX_SOURCE_BYTES,
            artifact_bytes: protocol::MAX_ARTIFACT_BYTES,
            heap_bytes: protocol::MAX_HEAP_BYTES,
            deadline: Duration::from_secs(5),
            budgets: Budgets::default(),
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Capacity(&'static str),
    Timeout,
    Cancelled,
    Io(String),
    Compilation(String),
    Unsupported(String),
    InvalidImage(String),
    Worker(String),
}
fn io(error: std::io::Error) -> Error {
    Error::Io(error.to_string())
}
fn checkpoint(start: Instant, limits: &Limits, cancelled: &AtomicBool) -> Result<(), Error> {
    if cancelled.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else if start.elapsed() >= limits.deadline {
        Err(Error::Timeout)
    } else {
        Ok(())
    }
}
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn create() -> Result<Self, Error> {
        for _ in 0..32 {
            let path = std::env::temp_dir().join(format!(
                "qjs-compiler-job-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io(e)),
            }
        }
        Err(Error::Worker(
            "compiler temporary directory collisions".into(),
        ))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Process {
    child: Child,
    reaped: bool,
}
impl Drop for Process {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            if let Ok(pid) = i32::try_from(self.child.id()) {
                // SAFETY: This child was placed in its own process group at spawn.
                // Negative PID targets only that group, never the caller's group.
                unsafe {
                    kill(-pid, 9);
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(io)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > limit {
        return Err(Error::Capacity("worker output"));
    }
    Ok(bytes)
}

/// Compile lossless UTF16 in a fresh process and independently verify its result.
/// Overrides can tighten the hard protocol limits, never increase them.
pub fn compile(
    executable: &Path,
    source: &[u16],
    filename: &str,
    limits: Limits,
    cancelled: &AtomicBool,
) -> Result<qjs_bytecode_image_verifier::image::VerifiedImage, Error> {
    compile_kind(
        executable,
        source,
        filename,
        limits,
        cancelled,
        (Mode::Script, None),
        &mut |_| {},
    )
    .and_then(Output::program)
}
/// Compile a module in the same isolated worker; installation/linking is separate.
pub fn compile_module(
    executable: &Path,
    source: &[u16],
    filename: &str,
    limits: Limits,
    cancelled: &AtomicBool,
) -> Result<qjs_bytecode_image_verifier::image::VerifiedImage, Error> {
    compile_kind(
        executable,
        source,
        filename,
        limits,
        cancelled,
        (Mode::Module, None),
        &mut |_| {},
    )
    .and_then(Output::program)
}
fn compile_kind(
    executable: &Path,
    source: &[u16],
    filename: &str,
    mut limits: Limits,
    cancelled: &AtomicBool,
    context: (Mode, Option<&qjs_bytecode_image_verifier::image::eval::VerifiedEnvLayout>),
    progress: &mut dyn FnMut(Progress),
) -> Result<Output, Error> {
    let (kind, environment) = context;
    let start = Instant::now();
    limits
        .budgets
        .validate()
        .map_err(|_| Error::Capacity("limit override"))?;
    checkpoint(start, &limits, cancelled)?;
    if limits.source_bytes > protocol::MAX_SOURCE_BYTES
        || limits.artifact_bytes > protocol::MAX_ARTIFACT_BYTES
        || limits.heap_bytes == 0
        || limits.heap_bytes > protocol::MAX_HEAP_BYTES
        || limits.deadline > Duration::from_secs(5)
    {
        return Err(Error::Capacity("limit override"));
    }
    if limits.artifact_bytes == 0 {
        return Err(Error::Capacity("worker output"));
    }
    limits.budgets.image.max_artifact_bytes = limits
        .budgets
        .image
        .max_artifact_bytes
        .min(limits.artifact_bytes);
    if source.len() > limits.source_bytes / 2 || filename.len() > protocol::MAX_FILENAME_BYTES {
        return Err(Error::Capacity("source or filename"));
    }
    let request = job_request::encode(kind, source, filename, limits.budgets, environment)
        .map_err(Error::Compilation)?;
    let request_digest: [u8; 32] = request[request.len() - 32..].try_into().unwrap();
    let directory = Directory::create()?;
    let input = directory.0.join("request");
    let output = directory.0.join("response");
    let diagnostics = directory.0.join("diagnostics");
    let phase = directory.0.join("phase");
    fs::write(&input, request).map_err(io)?;
    let error_file = File::create_new(&diagnostics).map_err(io)?;
    checkpoint(start, &limits, cancelled)?;
    let mut command = Command::new(executable);
    command
        .args([&input, &output, &phase])
        .env("QJS_COMPILER_HEAP_BYTES", limits.heap_bytes.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(error_file);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut process = Process {
        child: command.spawn().map_err(io)?,
        reaped: false,
    };
    loop {
        report_progress(progress, process.child.id(), &phase)?;
        if let Err(error) = checkpoint(start, &limits, cancelled) {
            let pid = process.child.id();
            drop(process); // kill/reap before reporting the final parser state
            report_progress(progress, pid, &phase)?;
            return Err(error);
        }
        if fs::metadata(&diagnostics).map_err(io)?.len() > protocol::MAX_DIAGNOSTIC_BYTES as u64 + 1
        {
            return Err(Error::Capacity("diagnostics"));
        }
        if let Some(status) = process.child.try_wait().map_err(io)? {
            process.reaped = true;
            checkpoint(start, &limits, cancelled)?;
            if !status.success() {
                let diagnostic = read_bounded(&diagnostics, protocol::MAX_DIAGNOSTIC_BYTES + 1)?;
                let text = String::from_utf8_lossy(&diagnostic).into_owned();
                return Err(if status.code() == Some(2) {
                    Error::Capacity("compiler heap")
                } else if status.code() == Some(3) {
                    Error::Capacity("compiler artifact")
                } else if status.code() == Some(4) {
                    Error::Capacity("compiler model")
                } else if status.code() == Some(5) {
                    Error::Unsupported(text)
                } else if status.code() == Some(6) {
                    Error::InvalidImage(text)
                } else if status.code() == Some(1) {
                    Error::Compilation(text)
                } else {
                    Error::Worker(format!("{status}: {text}"))
                });
            }
            let response = read_bounded(&output, limits.artifact_bytes.saturating_add(45))?;
            let bytes = job_request::read_response(&response, kind, &request_digest)
                .map_err(Error::InvalidImage)?;
            if bytes.len() > limits.artifact_bytes {
                return Err(Error::Capacity("worker output"));
            }
            let verified = if kind == Mode::Eval {
                Output::Eval(
                    qjs_bytecode_image_verifier::image::eval::deserialize_eval_artifact_with_limits(
                        bytes,
                        environment.unwrap(),
                        &limits.budgets.image,
                    )
                    .map_err(image_error)?,
                )
            } else {
                Output::Program(
                    qjs_bytecode_image_verifier::image::deserialize_artifact_with_limits(
                        bytes,
                        &limits.budgets.image,
                    )
                    .map_err(image_error)?,
                )
            };
            let (function, spans, profile) = match &verified {
                Output::Program(image) => {
                    if image.module().is_some() != (kind == Mode::Module)
                        || image
                            .module()
                            .is_some_and(|m| m.name != filename.encode_utf16().collect::<Vec<_>>())
                    {
                        return Err(Error::InvalidImage(
                            "worker result mode/module name mismatch".into(),
                        ));
                    }
                    (image.function(), image.spans(), image.profile())
                }
                Output::Eval(image) => (image.function(), image.spans(), image.profile()),
            };
            if profile != qjs_bytecode_image_verifier::image::COMPILER_PROFILE
                || function.filename != filename
                || function.source.as_deref() != Some(source)
                || spans.is_none_or(|spans| {
                    spans.source_unit != qjs_bytecode_image_verifier::image::SourceUnit::Utf16
                        || spans.source_len as usize != source.len()
                        || spans.source_digest != qjs_bytecode_image_verifier::image::source_digest_utf16(source)
                })
            {
                return Err(Error::InvalidImage(
                    "worker result does not match source request".into(),
                ));
            }
            checkpoint(start, &limits, cancelled)?;
            return Ok(verified);
        }
        std::thread::sleep(
            Duration::from_millis(2).min(limits.deadline.saturating_sub(start.elapsed())),
        );
    }
}

/// A parser allocator event, recorded from inside Parser::parse, not before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub pid: u32,
    pub parser_active: bool,
    pub parse_allocations: usize,
}
fn report_progress(observer: &mut dyn FnMut(Progress), pid: u32, path: &Path) -> Result<(), Error> {
    if !path.exists() {
        return Ok(());
    }
    let bytes = read_bounded(path, 1024)?;
    if bytes.contains(&b'P') {
        observer(Progress {
            pid,
            parser_active: bytes.last() == Some(&b'P'),
            parse_allocations: bytes.iter().filter(|b| **b == b'P').count(),
        });
    }
    Ok(())
}
/// Observe allocation events inside the real parser; callbacks run on the caller,
/// never inside the worker or its allocator. Cancelling still kills/reaps the job.
pub fn compile_with_progress(
    executable: &Path,
    source: &[u16],
    filename: &str,
    limits: Limits,
    cancelled: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<qjs_bytecode_image_verifier::image::VerifiedImage, Error> {
    compile_kind(
        executable,
        source,
        filename,
        limits,
        cancelled,
        (Mode::Script, None),
        progress,
    )
    .and_then(Output::program)
}

fn image_error(error: String) -> Error {
    if error.starts_with("capacity:") {
        Error::Capacity("image verification")
    } else {
        Error::InvalidImage(error)
    }
}
enum Output {
    Program(qjs_bytecode_image_verifier::image::VerifiedImage),
    Eval(qjs_bytecode_image_verifier::image::eval::VerifiedEvalImage),
}
impl Output {
    fn program(self) -> Result<qjs_bytecode_image_verifier::image::VerifiedImage, Error> {
        match self {
            Self::Program(p) => Ok(p),
            _ => Err(Error::Worker("internal output mode mismatch".into())),
        }
    }
}
/// Compile eval source using only an owned environment descriptor. The result
/// contains no live lease and does not authorize installing into any frame.
pub fn compile_eval(
    executable: &Path,
    source: &[u16],
    filename: &str,
    environment: &qjs_bytecode_image_verifier::image::eval::VerifiedEnvLayout,
    limits: Limits,
    cancelled: &AtomicBool,
) -> Result<qjs_bytecode_image_verifier::image::eval::VerifiedEvalImage, Error> {
    match compile_kind(
        executable,
        source,
        filename,
        limits,
        cancelled,
        (Mode::Eval, Some(environment)),
        &mut |_| {},
    )? {
        Output::Eval(image) => Ok(image),
        _ => Err(Error::Worker("internal eval mode mismatch".into())),
    }
}
