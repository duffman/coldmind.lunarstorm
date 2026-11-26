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
//! - The terminal API is the HostApi area `ui` ([`crate::terminal_ui_area`]), selected by the
//!   host's manifest (`manifest.json`). The host's own bindings keep `ops.color`
//!   and `ops.tty`, which the Runtime's console reads, and `lunarstorm:ui`, the
//!   specifier scripts used before HostApi.

use lunar_extension_hostapi::HostApi;
use lunarstorm_cliui::{report, term};
use lunarstorm_event_loop_runtime::vm::{Scope, Vm};
use lunarstorm_event_loop_runtime::{Access, Bindings, Host, Log, Manifest, Stream};
use std::io;
use std::sync::OnceLock;

use crate::terminal_ui_area::Ui;
use crate::terminal_ui_module;

/// Specifier scripts import the terminal UI module by, besides
/// `lunarstorm:hostapi/ui`.
pub const UI_NAME: &str = "lunarstorm:ui";


/// The host's manifest: the extensions and plugins every program of this host
/// gets.
///
/// Decision: it lives with the host and is embedded when the host is built; a
/// script cannot change it.
const MANIFEST: &str = include_str!("../manifest.json");

/// # Returns
/// The parsed [`MANIFEST`], parsed once per process.
///
/// # Panics
/// When `manifest.json` is not a manifest, a fault of the build that the test
/// `the_manifest_matches_the_host` catches.
fn manifest() -> &'static Manifest {
    static PARSED: OnceLock<Manifest> = OnceLock::new();
    PARSED.get_or_init(|| Manifest::parse(MANIFEST).expect("manifest.json of lunarstorm-host-command-line"))
}
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
    /// HostApi with the terminal area.
    type Extensions<V: Vm> = (HostApi<V, (Ui,)>,);

    /// # Returns
    /// A fresh set for one Runtime.
    fn extensions<V: Vm>(&self) -> Self::Extensions<V> {
        (HostApi::new((Ui,)),)
    }

    /// # Returns
    /// The host's embedded manifest.
    fn manifest(&self) -> &Manifest {
        manifest()
    }

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

/// Builds the bindings this host registers itself: the terminal facts the
/// Runtime's console reads, and the `lunarstorm:ui` specifier. The terminal API
/// is the HostApi area `ui` ([`crate::terminal_ui_area`]).
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
        // `lunarstorm:ui`, the same module as `lunarstorm:hostapi/ui`.
        //
        // Decision: a native module whose exports are the area module's own
        // objects (`terminal_ui_module::exports` builds them once and reuses them),
        // so both specifiers share one live region. It replaces a JS re-export
        // (`export * from 'lunarstorm:hostapi/ui'`).
        .native_module_with_internals(terminal_ui_module::declaration(UI_NAME), |scope, ops, live, exports| {
            terminal_ui_module::exports::<V>(scope, ops, live, exports)
        })
}

#[cfg(test)]
mod tests {
    use lunarstorm_event_loop_runtime::{check, ExtensionSet};
    use lunarstorm_engine_quickjs_adapter::QuickJs;

    use super::*;

    /// `manifest.json` parses and uses only what this host offers.
    #[test]
    fn the_manifest_matches_the_host() {
        let host = CliHost::new(Vec::new());
        let offers = ExtensionSet::<QuickJs>::offers(&host.extensions::<QuickJs>());
        assert_eq!(check(host.manifest(), &offers), Ok(()));
        assert!(host.manifest().using("hostapi").is_some_and(|using| using.has("ui")));
    }
}
