//! The CLI app host: host services answered with the process's own argv, stdio,
//! environment, clock and exit, plus the terminal bindings only this kind of host
//! has.
//!
//! Decisions:
//! - Policy is "allow everything": a command-line program runs with the user's
//!   own rights, as Node does.
//! - Stdout and stderr are written with `write(2)` directly. The Runtime already
//!   batches stdout into one buffer per loop turn, so a second layer of buffering
//!   in front of the descriptor would only add a copy.
//! - The terminal natives (`color`, `tty`, `columns`, `live`) and the
//!   `lunarstorm:ui` module are registered here, not built into the Runtime: a
//!   host without a terminal has no use for them.

use lunarstorm_cliui::{report, term};
use lunarstorm_runtime::vm::{Call, Scope, Vm};
use lunarstorm_runtime::{Access, Bindings, Host, Log, Stream};
use std::io;

/// Specifier scripts import the terminal UI module by.
pub const UI_NAME: &str = "lunarstorm:ui";
/// Most bytes `getentropy(2)` hands out per call.
const ENTROPY_CHUNK: usize = 256;

/// The host services of the `lunarstorm` command.
pub struct CliHost {
    /// `process.argv`: `[runtime, script, ...args]`.
    args: Vec<String>,
    /// Script stdout goes to stderr: under `lunarstorm debug` stdout carries the
    /// Debug Adapter Protocol and nothing else may write to it.
    stdout_to_stderr: bool,
}

impl CliHost {
    /// # Arguments
    /// * `args` - `process.argv`.
    ///
    /// # Returns
    /// The host.
    pub fn new(args: Vec<String>) -> Self {
        Self { args, stdout_to_stderr: false }
    }

    /// A host whose script stdout goes to stderr, for `lunarstorm debug`.
    ///
    /// # Arguments
    /// * `args` - `process.argv`.
    ///
    /// # Returns
    /// The host.
    pub fn for_debugger(args: Vec<String>) -> Self {
        Self { args, stdout_to_stderr: true }
    }
}

/// Writes all of a buffer to a descriptor.
///
/// Decision: an interrupted write is retried and a full non-blocking pipe is
/// waited for, so output is never dropped while the reader is alive; any other
/// error (the reader went away) ends the write silently, as a closed pipe should.
///
/// # Arguments
/// * `fd` - 1 for stdout, 2 for stderr.
/// * `bytes` - the bytes to write.
fn put(fd: i32, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: `bytes` is a live slice; `write` reads at most `bytes.len()` bytes of it.
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written >= 0 {
            bytes = &bytes[written as usize..];
            continue;
        }
        match io::Error::last_os_error().kind() {
            io::ErrorKind::Interrupted => {}
            io::ErrorKind::WouldBlock => {
                let mut wait = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
                // SAFETY: `wait` is one valid `pollfd`, matching the count of 1.
                unsafe { libc::poll(&mut wait, 1, -1) };
            }
            _ => return,
        }
    }
}

impl Host for CliHost {
    /// # Returns
    /// `process.argv` as the command line gave it.
    fn args(&self) -> Vec<String> {
        self.args.clone()
    }

    /// # Returns
    /// The process environment; names or values that are not UTF-8 are converted
    /// lossily rather than dropped.
    fn environment(&self) -> Vec<(String, String)> {
        std::env::vars_os()
            .map(|(name, value)| (name.to_string_lossy().into_owned(), value.to_string_lossy().into_owned()))
            .collect()
    }

    /// # Returns
    /// The process's working directory, or `/` when it no longer exists.
    fn working_directory(&self) -> String {
        std::env::current_dir().map_or_else(|_| String::from("/"), |dir| dir.to_string_lossy().into_owned())
    }

    /// Writes to the process's stdout or stderr.
    ///
    /// # Arguments
    /// * `stream` - which stream.
    /// * `bytes` - the bytes to write.
    fn write(&self, stream: Stream, bytes: &[u8]) {
        put(if stream == Stream::Out && !self.stdout_to_stderr { 1 } else { 2 }, bytes);
    }

    /// Restores the cursor a live progress region may have hidden, then exits the
    /// process.
    ///
    /// # Arguments
    /// * `code` - the exit status.
    fn exit(&self, code: i32) -> ! {
        term::restore();
        std::process::exit(code)
    }

    /// # Arguments
    /// * `_access` - unused: the command line allows everything.
    ///
    /// # Returns
    /// `true`.
    #[inline]
    fn allow(&self, _access: Access<'_>) -> bool {
        true
    }

    /// Fills a buffer from the kernel's random source.
    ///
    /// # Arguments
    /// * `into` - the buffer to fill.
    fn entropy(&self, into: &mut [u8]) {
        for chunk in into.chunks_mut(ENTROPY_CHUNK) {
            // SAFETY: `chunk` is a live, writable slice of at most 256 bytes, the
            // most `getentropy` accepts.
            let filled = unsafe { libc::getentropy(chunk.as_mut_ptr().cast(), chunk.len()) };
            // The call cannot fail for a valid buffer of this size; if it ever
            // does, predictable "random" bytes must not reach a script.
            assert!(filled == 0, "lunarstorm: the system entropy source failed");
        }
    }

    /// Prints a Runtime record to stderr: warnings and errors as one line, an
    /// uncaught script error as a framed report.
    ///
    /// # Arguments
    /// * `record` - the record.
    fn log(&self, record: Log<'_>) {
        match record {
            Log::Warning(message) | Log::Error(message) => eprintln!("lunarstorm: {message}"),
            Log::Uncaught { in_promise, description } => {
                term::restore();
                let label = if in_promise { "Uncaught (in promise)" } else { "Uncaught" };
                eprintln!("{}", report::uncaught(label, description));
            }
        }
    }
}

/// Builds the bindings this host registers: the terminal natives and the
/// `lunarstorm:ui` module built on them.
///
/// # Returns
/// The set for `Runtime::register`. It is generic over the engine: the same
/// bindings work with any VM adapter.
pub fn bindings<V: Vm>() -> Bindings<V> {
    Bindings::<V>::new()
        // Constants: color depth and tty-ness of stdout and stderr, read once.
        .values(|scope, ops| {
            let depth = |fd| scope.int(term::depth(fd) as i32);
            scope.set(ops, "color", scope.array(&[depth(1), depth(2)])?)?;
            let tty = |fd| scope.boolean(term::is_tty(fd));
            scope.set(ops, "tty", scope.array(&[tty(1), tty(2)])?)
        })
        // The terminal width now; a resize is seen on the next redraw.
        .function("columns", |call: &Call<'_, V>| Ok(call.scope().int(i32::from(term::columns()))))
        // Enters or leaves the live region: hides or restores the cursor.
        .function("live", |call: &Call<'_, V>| {
            let scope = call.scope();
            term::live(call.args().first().and_then(|on| scope.as_bool(on)).unwrap_or(false));
            Ok(scope.undefined())
        })
        .module(UI_NAME, lunarstorm_cliui::UI_SOURCE)
}
