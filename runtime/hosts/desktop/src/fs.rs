//! The HostApi area `fs` of the desktop host: text files under the backend's
//! filesystem roots, for the WebView (through the Runtime's dispatcher in
//! `js/desktop.js`) and for backend code alike (`lunarstorm:hostapi/fs`).
//!
//! Decisions:
//! - The functions are natives; the module only re-exports them. Every path is
//!   checked here with the host's own [`crate::within`], so the scope is enforced
//!   in Rust, never in JS.
//! - A relative path resolves against the first root; with no root, every path
//!   is refused.
//! - Errors are JS `Error`s with a Node-style `code`: `EINVAL` for a bad argument,
//!   `EACCES` outside the roots, `ENOENT`, otherwise `EIO`.

use lunar_extension_hostapi::{Arg, Area, Function};
use lunarstorm_runtime::vm::{Call, ErrorKind, Kind, Outcome, Scope, Thrown, Vm};
use lunarstorm_runtime::{Bindings, Use};
use std::path::{Path, PathBuf};

/// The area's module.
pub const MODULE: &str = "lunarstorm:hostapi/fs";

/// The module: the natives under their API names.
const SOURCE: &str = "import { ops } from 'lunarstorm:internal';\nexport const readText = ops.hostapiFsReadText;\nexport const writeText = ops.hostapiFsWriteText;\nexport const list = ops.hostapiFsList;\n";

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

/// The filesystem area.
#[derive(Debug, Clone, Default)]
pub struct Fs {
    /// The backend's roots, canonical; the first is the base of relative paths.
    pub roots: Vec<PathBuf>,
}

/// Throws an `Error` carrying `code`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `code` - the Node-style code.
/// * `message` - what went wrong.
///
/// # Returns
/// The marker to return from the native.
fn throw<'s, V: Vm>(scope: &V::Scope<'s>, code: &str, message: &str) -> Thrown {
    match scope.error(ErrorKind::Error, &format!("{code}: {message}")) {
        Ok(error) => {
            if let Ok(value) = scope.string(code) {
                let _ = scope.set(&error, "code", value);
            }
            scope.throw(error)
        }
        Err(thrown) => thrown,
    }
}

/// Reads a string argument.
///
/// # Arguments
/// * `call` - the call.
/// * `index` - the argument's position.
/// * `what` - its name, for the message.
///
/// # Returns
/// The text.
///
/// # Errors
/// `EINVAL` when it is missing or not a string.
fn string<V: Vm>(call: &Call<'_, V>, index: usize, what: &str) -> Outcome<String> {
    let scope = call.scope();
    match call.args().get(index) {
        Some(value) if scope.kind(value) == Kind::String => scope.with_str(value, str::to_owned),
        _ => Err(throw::<V>(scope, "EINVAL", &format!("{what} must be a string"))),
    }
}

/// Resolves and checks a path argument.
///
/// # Arguments
/// * `call` - the call; argument 0 is the path.
/// * `roots` - the backend's roots.
///
/// # Returns
/// The path to operate on.
///
/// # Errors
/// `EINVAL` for a missing, empty or NUL-carrying path; `EACCES` when it lies
/// outside the roots.
fn path<V: Vm>(call: &Call<'_, V>, roots: &[PathBuf]) -> Outcome<PathBuf> {
    let text = string(call, 0, "path")?;
    if text.is_empty() || text.contains('\0') {
        return Err(throw::<V>(call.scope(), "EINVAL", "path must be a non-empty string"));
    }
    let path = match roots.first() {
        _ if Path::new(&text).is_absolute() => PathBuf::from(&text),
        Some(base) => base.join(&text),
        None => PathBuf::from(&text),
    };
    if !crate::within(roots, &path.to_string_lossy()) {
        return Err(throw::<V>(call.scope(), "EACCES", &format!("'{text}' is outside the app's roots")));
    }
    Ok(path)
}

/// Throws an I/O error with its code.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `error` - what the OS reported.
/// * `path` - the path, for the message.
///
/// # Returns
/// The marker to return from the native.
fn io<'s, V: Vm>(scope: &V::Scope<'s>, error: &std::io::Error, path: &Path) -> Thrown {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        _ => "EIO",
    };
    throw::<V>(scope, code, &format!("{error}, '{}'", path.display()))
}

impl<V: Vm> Area<V> for Fs {
    const NAME: &'static str = "fs";

    /// The functions the module exports, for the HostApi spec.
    const FUNCTIONS: &'static [Function] = &[
        Function { name: "readText", summary: "Reads a UTF-8 file under the app's roots.", args: &[required("path")] },
        Function { name: "writeText", summary: "Writes a UTF-8 file under the app's roots.", args: &[required("path"), required("text")] },
        Function { name: "list", summary: "Lists the names in a directory under the app's roots, sorted.", args: &[required("path")] },
    ];

    /// Builds the three natives and the module.
    ///
    /// # Arguments
    /// * `_using` - unused: the roots come from the host, not the manifest, so
    ///   nothing a manifest says can widen them.
    ///
    /// # Returns
    /// `ops.hostapiFsReadText`, `ops.hostapiFsWriteText`, `ops.hostapiFsList` and
    /// `lunarstorm:hostapi/fs`.
    fn bindings(&self, _using: &Use<'_>) -> Result<Bindings<V>, String> {
        let (read, write, list) = (self.roots.clone(), self.roots.clone(), self.roots.clone());
        Ok(Bindings::<V>::new()
            .function("hostapiFsReadText", move |call: &Call<'_, V>| {
                let path = path(call, &read)?;
                match std::fs::read_to_string(&path) {
                    Ok(text) => call.scope().string(&text),
                    Err(error) => Err(io::<V>(call.scope(), &error, &path)),
                }
            })
            .function("hostapiFsWriteText", move |call: &Call<'_, V>| {
                let path = path(call, &write)?;
                let text = string(call, 1, "text")?;
                match std::fs::write(&path, text) {
                    Ok(()) => Ok(call.scope().undefined()),
                    Err(error) => Err(io::<V>(call.scope(), &error, &path)),
                }
            })
            .function("hostapiFsList", move |call: &Call<'_, V>| {
                let path = path(call, &list)?;
                let scope = call.scope();
                let mut names: Vec<String> = match std::fs::read_dir(&path) {
                    Ok(entries) => entries.filter_map(Result::ok).map(|entry| entry.file_name().to_string_lossy().into_owned()).collect(),
                    Err(error) => return Err(io::<V>(scope, &error, &path)),
                };
                names.sort();
                let items = names.iter().map(|name| scope.string(name)).collect::<Outcome<Vec<_>>>()?;
                scope.array(&items)
            })
            .module(MODULE, SOURCE))
    }
}
