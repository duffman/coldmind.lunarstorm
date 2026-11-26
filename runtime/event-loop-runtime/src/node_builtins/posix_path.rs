//! `node:path`: POSIX path helpers.
//!
//! Pure string functions, ported from the JS glue. Only `/` and `.` are looked
//! at, so byte positions in UTF-8 match the JS string positions they replace.

use super::{f, p};
use crate::host_bindings_registry::declared;
use crate::host_services_trait::Host;
use crate::reactor_io_state::Shared;
use lunarstorm_engine_contract::{Call, Field, FnDecl, Item, Kind, ModuleDecl, Outcome, Scope, Ty, Vm};
use std::rc::Rc;

/// A path argument.
const PATH: lunarstorm_engine_contract::Param = p("p", Ty::String, "a path");
/// A path segment of `join` and `resolve`.
const SEGMENT: lunarstorm_engine_contract::Param = p("parts", Ty::String, "path segments");

const IS_ABSOLUTE: FnDecl = f("isAbsolute", &[PATH], None, Ty::Bool, "Whether the path starts at the root.");
const NORMALIZE: FnDecl = f("normalize", &[PATH], None, Ty::String, "Resolves `.` and `..` segments and repeated slashes.");
const JOIN: FnDecl = f("join", &[], Some(SEGMENT), Ty::String, "Joins segments and normalizes the result.");
const RESOLVE: FnDecl = f("resolve", &[], Some(SEGMENT), Ty::String, "Resolves segments right to left into an absolute path.");
const DIRNAME: FnDecl = f("dirname", &[PATH], None, Ty::String, "The directory part of a path.");
const BASENAME: FnDecl = f(
    "basename",
    &[PATH, p("ext", Ty::Optional(&Ty::String), "suffix to strip")],
    None,
    Ty::String,
    "The last segment of a path.",
);
const EXTNAME: FnDecl = f("extname", &[PATH], None, Ty::String, "The extension including its dot, or ''.");

/// The constants, in the module's key order.
const VALUES: [Field; 2] = [
    Field { name: "sep", ty: Ty::String, about: "The segment separator, '/'." },
    Field { name: "delimiter", ty: Ty::String, about: "The PATH list delimiter, ':'." },
];

/// # Returns
/// The declaration of `node:path`, in the module's key order.
pub(crate) fn declare() -> ModuleDecl {
    let functions = [IS_ABSOLUTE, NORMALIZE, JOIN, RESOLVE, DIRNAME, BASENAME, EXTNAME];
    let items = VALUES.iter().copied().map(Item::Value).chain(functions.into_iter().map(Item::Function)).collect();
    ModuleDecl { specifier: "node:path", items, about: "POSIX path helpers." }
}

/// Resolves `.` and `..` segments and repeated slashes.
///
/// # Arguments
/// * `p` - a path.
///
/// # Returns
/// The normalized path; `.` (or `./`) for an empty relative result.
pub(crate) fn normalize(p: &str) -> String {
    let absolute = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part != ".." {
            out.push(part);
        } else if out.last().is_some_and(|last| *last != "..") {
            out.pop();
        } else if !absolute {
            out.push("..");
        }
    }
    let mut joined = out.join("/");
    if absolute {
        if !joined.is_empty() && p.ends_with('/') {
            joined.push('/');
        }
        return format!("/{joined}");
    }
    // Fix (node_compat_fixes `path_normalize_keeps_trailing_slash_of_dot`): the glue gave
    // `.` for `./` and `a/../`, dropping the trailing slash; Node gives `./`.
    // Decision: an empty relative result becomes `.` first, and the trailing slash
    // of the input is kept on it as on any other result. `normalize("")` and
    // `normalize("a/..")` stay `.` (no trailing slash in the input).
    if joined.is_empty() {
        joined.push('.');
    }
    if p.ends_with('/') {
        joined.push('/');
    }
    joined
}

/// Joins segments, skipping empty ones, and normalizes the result.
///
/// # Arguments
/// * `parts` - the segments.
///
/// # Returns
/// The path; `.` when every segment is empty.
fn join(parts: &[String]) -> String {
    let joined = parts.iter().filter(|part| !part.is_empty()).map(String::as_str).collect::<Vec<_>>().join("/");
    normalize(if joined.is_empty() { "." } else { &joined })
}

/// Resolves segments right to left into an absolute path.
///
/// # Arguments
/// * `cwd` - the host's working directory, used when no segment is absolute.
/// * `parts` - the segments.
///
/// # Returns
/// The absolute path, without a trailing slash except for the root.
fn resolve(cwd: &str, parts: &[String]) -> String {
    let mut joined = String::new();
    for part in parts.iter().rev() {
        if joined.starts_with('/') {
            break;
        }
        if !part.is_empty() {
            joined = format!("{part}/{joined}");
        }
    }
    if !joined.starts_with('/') {
        joined = format!("{cwd}/{joined}");
    }
    let mut out = normalize(&joined);
    if out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

/// # Arguments
/// * `p` - a path.
///
/// # Returns
/// The directory part: `/` for the root, `.` for a bare name.
fn dirname(p: &str) -> String {
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        return String::from(if p.starts_with('/') { "/" } else { "." });
    }
    match trimmed.rfind('/') {
        None => String::from("."),
        Some(0) => String::from("/"),
        Some(at) => trimmed[..at].to_owned(),
    }
}

/// # Arguments
/// * `p` - a path.
/// * `ext` - a suffix to strip, unless it is the whole name.
///
/// # Returns
/// The last segment, trailing slashes ignored.
fn basename<'a>(p: &'a str, ext: Option<&str>) -> &'a str {
    let trimmed = p.trim_end_matches('/');
    let base = &trimmed[trimmed.rfind('/').map_or(0, |at| at + 1)..];
    match ext {
        Some(ext) if !ext.is_empty() && base.ends_with(ext) && base != ext => &base[..base.len() - ext.len()],
        _ => base,
    }
}

/// # Arguments
/// * `p` - a path.
///
/// # Returns
/// The extension of the last segment including its dot, or `""`.
fn extname(p: &str) -> &str {
    let base = basename(p, None);
    // Fix (node_compat_fixes `path_extname_of_dot_dot_is_empty`): the glue gave `.` for
    // `..`, reading its second dot as an extension; Node gives `""`. Decision: the
    // parent segment `..` has no extension; every other name keeps the glue's
    // rule (a dot at index 0 starts no extension), so `...` and `a.` stay `.`.
    if base == ".." {
        return "";
    }
    match base.rfind('.') {
        None | Some(0) => "",
        Some(at) => &base[at..],
    }
}

/// Reads every argument as text; the declaration has checked they are strings.
///
/// # Arguments
/// * `call` - the native call.
///
/// # Returns
/// The texts.
///
/// # Errors
/// A `TypeError` for an unpaired surrogate.
fn texts<V: Vm>(call: &Call<'_, V>) -> Outcome<Vec<String>> {
    call.args().iter().map(|value| call.scope().with_str(value, str::to_owned)).collect()
}

/// Builds the `node:path` object.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state; `resolve` reads the host's working directory.
///
/// # Returns
/// The module object, its keys in [`declare`]'s order.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when a value cannot be created.
pub(crate) fn install<'s, V: Vm, H: Host>(scope: &V::Scope<'s>, shared: &Rc<Shared<H>>) -> Outcome<V::Value<'s>> {
    let path = scope.object()?;
    scope.set(&path, "sep", scope.string("/")?)?;
    scope.set(&path, "delimiter", scope.string(":")?)?;

    let is_absolute = declared(scope, IS_ABSOLUTE.name, IS_ABSOLUTE.sig, |call: &Call<'_, V>| {
        let absolute = call.scope().with_str(&call.args()[0], |p| p.starts_with('/'))?;
        Ok(call.scope().boolean(absolute))
    })?;
    scope.set(&path, IS_ABSOLUTE.name, is_absolute)?;
    let normalized = declared(scope, NORMALIZE.name, NORMALIZE.sig, |call: &Call<'_, V>| {
        let out = call.scope().with_str(&call.args()[0], normalize)?;
        call.scope().string(&out)
    })?;
    scope.set(&path, NORMALIZE.name, normalized)?;
    let joined = declared(scope, JOIN.name, JOIN.sig, |call: &Call<'_, V>| call.scope().string(&join(&texts(call)?)))?;
    scope.set(&path, JOIN.name, joined)?;
    let host = shared.clone();
    let resolved = declared(scope, RESOLVE.name, RESOLVE.sig, move |call: &Call<'_, V>| {
        call.scope().string(&resolve(&host.host.working_directory(), &texts(call)?))
    })?;
    scope.set(&path, RESOLVE.name, resolved)?;
    let dir = declared(scope, DIRNAME.name, DIRNAME.sig, |call: &Call<'_, V>| {
        let out = call.scope().with_str(&call.args()[0], dirname)?;
        call.scope().string(&out)
    })?;
    scope.set(&path, DIRNAME.name, dir)?;
    let base = declared(scope, BASENAME.name, BASENAME.sig, |call: &Call<'_, V>| {
        let scope = call.scope();
        let ext = match call.args().get(1) {
            Some(ext) if scope.kind(ext) == Kind::String => Some(scope.with_str(ext, str::to_owned)?),
            _ => None,
        };
        let out = scope.with_str(&call.args()[0], |p| basename(p, ext.as_deref()).to_owned())?;
        scope.string(&out)
    })?;
    scope.set(&path, BASENAME.name, base)?;
    let ext = declared(scope, EXTNAME.name, EXTNAME.sig, |call: &Call<'_, V>| {
        let out = call.scope().with_str(&call.args()[0], |p| extname(p).to_owned())?;
        call.scope().string(&out)
    })?;
    scope.set(&path, EXTNAME.name, ext)?;
    Ok(path)
}
