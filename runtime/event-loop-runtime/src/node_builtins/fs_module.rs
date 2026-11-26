//! `node:fs` and `node:fs/promises`: one table of operations, three flavours
//! (sync, promise, callback), all over [`crate::blocking_filesystem_operations::exec`].
//!
//! Decisions:
//! - The operation codes are [`crate::blocking_filesystem_operations::op`] itself; the glue's hand-copied
//!   `FS_*` table is gone.
//! - An asynchronous request is a promise made with [`Scope::promise`]; its
//!   resolver waits in [`Pending`] (owned by `Node`, cleared on stop) until the
//!   pool's result arrives ([`done`]).
//! - The callback flavour is the promise flavour plus
//!   `.then(cb.bind(undefined, null), cb)`, as the glue's
//!   `run(...args).then((v) => cb(null, v), cb)`: the callback runs as a promise
//!   job, and no native closure holds the callback.
//! - The functions are installed without their declaration's argument check: as
//!   in the glue, a path is converted with `String()` and only the payload is
//!   checked (`fs: data must be ...`). The declarations carry the types a script
//!   should use.

use super::buffer_and_text_codecs::{as_buffer, utf8_only};
use super::{f, is_object, p, Node, Slot};
use crate::blocking_filesystem_operations::{op, FsValue};
use crate::host_services_trait::Host;
use crate::reactor_io_state::{Fault, Shared};
use crate::internal_natives::{fs_run, fs_value, DATA};
use lunarstorm_engine_contract::{Call, ClassDecl, ClassSpec, ErrorKind, Field, FnDecl, Item, Kind, ModuleDecl, Outcome, Scope, Sig, Thrown, Ty, Vm};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// A path argument; converted with `String()`.
const PATH: lunarstorm_engine_contract::Param = p("path", Ty::String, "the path; converted with String()");
/// A payload argument.
const PAYLOAD: lunarstorm_engine_contract::Param = p("data", DATA, "file contents: text (written as UTF-8) or bytes");
/// The options of `readFile`.
const READ_OPTIONS: lunarstorm_engine_contract::Param = p(
    "options",
    Ty::Optional(&Ty::Union(&[Ty::String, Ty::Object(&[Field { name: "encoding", ty: Ty::Optional(&Ty::String), about: "'utf8' for text" }])])),
    "an encoding, or { encoding }; only UTF-8, and text is returned",
);
/// The options of `mkdir` and `rm`.
const RECURSIVE: lunarstorm_engine_contract::Param = p(
    "options",
    Ty::Optional(&Ty::Object(&[Field { name: "recursive", ty: Ty::Optional(&Ty::Bool), about: "parents (mkdir) or the whole tree (rm)" }])),
    "{ recursive }",
);
/// What `readFile` resolves to.
const CONTENTS: Ty = Ty::Union(&[Ty::Named("Buffer"), Ty::String]);

/// One operation of the API, in the module's key order.
#[derive(Clone, Copy)]
enum Api {
    /// `readFile(path, options?)`
    ReadFile,
    /// `writeFile(path, data)`
    WriteFile,
    /// `appendFile(path, data)`
    AppendFile,
    /// `stat(path)`
    Stat,
    /// `readdir(path)`
    Readdir,
    /// `mkdir(path, { recursive }?)`
    Mkdir,
    /// `unlink(path)`
    Unlink,
    /// `rmdir(path)`
    Rmdir,
    /// `rename(from, to)`
    Rename,
    /// `rm(path, { recursive }?)`
    Rm,
}

/// Every operation: its name, the sync flavour's name, the declaration's
/// parameters, the sync and the promise results, and its summary.
const API: [(Api, &str, &str, &[lunarstorm_engine_contract::Param], Ty, Ty, &str); 10] = [
    (Api::ReadFile, "readFile", "readFileSync", &[PATH, READ_OPTIONS], CONTENTS, Ty::Promise(&CONTENTS), "Reads a file: a Buffer, or text with an encoding."),
    (Api::WriteFile, "writeFile", "writeFileSync", &[PATH, PAYLOAD], Ty::Undefined, DONE, "Creates or truncates a file and writes the data."),
    (Api::AppendFile, "appendFile", "appendFileSync", &[PATH, PAYLOAD], Ty::Undefined, DONE, "Appends the data, creating the file if needed."),
    (Api::Stat, "stat", "statSync", &[PATH], Ty::Named("Stats"), Ty::Promise(&Ty::Named("Stats")), "Size, modification time and type of a path."),
    (Api::Readdir, "readdir", "readdirSync", &[PATH], Ty::Array(&Ty::String), Ty::Promise(&Ty::Array(&Ty::String)), "The names in a directory."),
    (Api::Mkdir, "mkdir", "mkdirSync", &[PATH, RECURSIVE], Ty::Undefined, DONE, "Creates a directory, with its parents when recursive."),
    (Api::Unlink, "unlink", "unlinkSync", &[PATH], Ty::Undefined, DONE, "Removes a file."),
    (Api::Rmdir, "rmdir", "rmdirSync", &[PATH], Ty::Undefined, DONE, "Removes an empty directory."),
    (Api::Rename, "rename", "renameSync", &[PATH, p("to", Ty::String, "the new path; converted with String()")], Ty::Undefined, DONE, "Renames a path."),
    (Api::Rm, "rm", "rmSync", &[PATH, RECURSIVE], Ty::Undefined, DONE, "Removes a file, or a whole tree when recursive."),
];

/// What an operation with no result resolves to.
const DONE: Ty = Ty::Promise(&Ty::Undefined);

const IS_FILE: FnDecl = f("isFile", &[], None, Ty::Bool, "Whether the path is a regular file.");
const IS_DIRECTORY: FnDecl = f("isDirectory", &[], None, Ty::Bool, "Whether the path is a directory.");

/// The `fs.Stats` class.
const STATS: ClassDecl = ClassDecl {
    name: "Stats",
    extends: None,
    constructor: Sig {
        params: &[p("raw", Ty::Array(&Ty::Number), "[size, mtimeMs, kind]; kind 1 is a file, 2 a directory")],
        rest: None,
        returns: Ty::Named("Stats"),
        about: "The result of stat().",
    },
    methods: &[IS_FILE, IS_DIRECTORY],
    accessors: &[Field { name: "mtime", ty: Ty::Named("Date"), about: "Modification time." }],
    statics: &[],
    about: "Result of stat(): size, mtimeMs, mtime and type tests.",
};

/// # Returns
/// The declarations of `node:fs` and `node:fs/promises`.
pub(crate) fn declare() -> Vec<ModuleDecl> {
    // Decision: `fs` lists the classes and the sync flavour; the callback flavour
    // has the names of `fs/promises` with a trailing `(err, value)` callback,
    // which one signature per name cannot also say.
    let mut items = vec![Item::Class(STATS), Item::Function(f("existsSync", &[PATH], None, Ty::Bool, "Whether a path exists."))];
    items.extend(API.iter().map(|&(_, _, name, params, returns, _, about)| Item::Function(FnDecl { name, sig: Sig { params, rest: None, returns, about } })));
    let promises = API.iter().map(|&(_, name, _, params, _, returns, about)| Item::Function(FnDecl { name, sig: Sig { params, rest: None, returns, about } })).collect();
    vec![
        ModuleDecl { specifier: "node:fs", items, about: "The filesystem: sync, callback and promise flavours." },
        ModuleDecl { specifier: "node:fs/promises", items: promises, about: "The filesystem, promise flavour." },
    ]
}

/// Asynchronous requests waiting for the worker pool.
pub(crate) struct Pending<V: Vm> {
    /// Resolver and operation code by request id.
    waiting: RefCell<HashMap<i32, (V::Persistent, i32)>>,
    /// Last request id handed out; 0 is reserved for inline calls.
    seq: Cell<i32>,
}

impl<V: Vm> Default for Pending<V> {
    /// # Returns
    /// No request waiting.
    fn default() -> Self {
        Self { waiting: RefCell::default(), seq: Cell::new(0) }
    }
}

impl<V: Vm> Pending<V> {
    /// Drops every resolver. Runs before the VM instance is destroyed.
    pub(crate) fn clear(&self) {
        let waiting = std::mem::take(&mut *self.waiting.borrow_mut());
        drop(waiting);
    }
}

/// The Rust data of a `Stats` instance: the kind of path (1 file, 2 directory).
struct PathKind(f64);

/// How a call runs.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Inline, returning the value.
    Sync,
    /// On the pool, returning a promise.
    Promise,
}

/// Picks the operation and its payload for a call, before anything runs.
///
/// As in the glue, the encoding check, the `recursive` read and `String(to)`
/// happen here, outside the promise: their errors throw synchronously.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `api` - which operation.
/// * `args` - the call's arguments.
///
/// # Returns
/// The operation code and the payload argument (`undefined` when it has none).
///
/// # Errors
/// `TypeError: Unsupported encoding`, or what a getter or `String()` throws.
fn select<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, api: Api, args: &[V::Value<'s>]) -> Outcome<(i32, Data<'s, V>)> {
    let at = |index: usize| super::arg::<V>(scope, args, index);
    let recursive = |index: usize| -> Outcome<bool> {
        let options = at(index);
        Ok(node.truthy(scope, &options) && is_object::<V>(scope, &options) && node.truthy(scope, &scope.get(&options, "recursive")?))
    };
    Ok(match api {
        Api::ReadFile => {
            let options = at(1);
            let encoding = match scope.kind(&options) {
                Kind::String => options,
                _ if node.truthy(scope, &options) && is_object::<V>(scope, &options) => scope.get(&options, "encoding")?,
                _ => scope.undefined(),
            };
            if node.truthy(scope, &encoding) {
                utf8_only(node, scope, &encoding, ErrorKind::Type)?;
                (op::READ_UTF8, Data::None)
            } else {
                (op::READ, Data::None)
            }
        }
        Api::WriteFile => (op::WRITE, Data::Value(at(1))),
        Api::AppendFile => (op::APPEND, Data::Value(at(1))),
        Api::Stat => (op::STAT, Data::None),
        Api::Readdir => (op::READDIR, Data::None),
        Api::Mkdir => (if recursive(1)? { op::MKDIR_P } else { op::MKDIR }, Data::None),
        Api::Unlink => (op::UNLINK, Data::None),
        Api::Rmdir => (op::RMDIR, Data::None),
        Api::Rename => (op::RENAME, Data::Text(node.text(scope, &at(1))?)),
        Api::Rm => (if recursive(1)? { op::RM_RF } else { op::UNLINK }, Data::None),
    })
}

/// The payload of an operation.
enum Data<'s, V: Vm> {
    /// None.
    None,
    /// The script's value: text, bytes, `null` or `undefined`.
    Value(V::Value<'s>),
    /// Text made here (a rename's target).
    Text(String),
}

/// Starts an operation: checks the payload, asks the policy, runs it inline or
/// queues it.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state.
/// * `code` - the operation code.
/// * `path` - the path, already text.
/// * `data` - the payload.
/// * `req` - 0 inline, else the request id.
///
/// # Returns
/// `Ok(value)` inline, `Ok(None)` queued, or the fault.
///
/// # Errors
/// `TypeError: fs: data must be ...` for a payload of the wrong type.
fn start<'s, V: Vm, H: Host>(
    scope: &V::Scope<'s>,
    shared: &Shared<H>,
    code: i32,
    path: &str,
    data: &Data<'s, V>,
    req: i32,
) -> Outcome<Result<Option<FsValue>, Fault>> {
    match data {
        Data::None => Ok(fs_run(shared, code, path, &[], req)),
        Data::Text(text) => Ok(fs_run(shared, code, path, text.as_bytes(), req)),
        Data::Value(value) => {
            // The same check and message `ops.fs` gives its `data` parameter.
            if let Err(ty) = DATA.check::<V>(scope, value) {
                return Err(scope.throw_error(ErrorKind::Type, &format!("fs: data must be {ty}")));
            }
            if matches!(scope.kind(value), Kind::Null | Kind::Undefined) {
                return Ok(fs_run(shared, code, path, &[], req));
            }
            scope.with_bytes(value, |bytes| fs_run(shared, code, path, bytes, req))
        }
    }
}

/// Converts a result to its Node shape: `READ` gives a `Buffer`, `STAT` a
/// `Stats`, everything else is unchanged.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `code` - the operation code.
/// * `value` - the native result ([`fs_value`]).
///
/// # Returns
/// The value a script sees.
///
/// # Errors
/// What `Stats` or `setPrototypeOf` throws.
fn shape<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, code: i32, value: V::Value<'s>) -> Outcome<V::Value<'s>> {
    match code {
        op::READ => as_buffer(node, scope, value),
        op::STAT => node.construct(scope, &node.get(scope, Slot::Stats), &[value]),
        _ => Ok(value),
    }
}

/// Runs one call of the sync or promise flavour.
///
/// # Arguments
/// * `node` - the kept builtins and pending requests.
/// * `shared` - loop state.
/// * `scope` - the context handle.
/// * `api` - which operation.
/// * `mode` - inline or on the pool.
/// * `args` - the call's arguments.
///
/// # Returns
/// The value, or the promise.
///
/// # Errors
/// What [`select`] throws; inline, also the operation's fault.
fn run<'s, V: Vm, H: Host>(node: &Node<V>, shared: &Shared<H>, scope: &V::Scope<'s>, api: Api, mode: Mode, args: &[V::Value<'s>]) -> Outcome<V::Value<'s>> {
    let (code, data) = select(node, scope, api, args)?;
    let path = super::arg::<V>(scope, args, 0);
    if mode == Mode::Sync {
        let path = node.text(scope, &path)?;
        return match start(scope, shared, code, &path, &data, 0)? {
            Ok(Some(value)) => shape(node, scope, code, fs_value::<V>(scope, value)?),
            Ok(None) => Ok(scope.undefined()),
            Err(fault) => Err(scope.throw(super::coded_error::<V>(scope, &fault.message, Some(fault.code))?)),
        };
    }
    // The glue's executor: everything from here rejects instead of throwing.
    let (promise, resolver) = scope.promise()?;
    let seq = node.fs.seq.get() % 0x7fff_ffff + 1;
    node.fs.seq.set(seq);
    let started = node.text(scope, &path).and_then(|path| start(scope, shared, code, &path, &data, seq));
    match started {
        Ok(Ok(_)) => {
            node.fs.waiting.borrow_mut().insert(seq, (resolver, code));
        }
        Ok(Err(fault)) => scope.settle(&resolver, Err(super::coded_error::<V>(scope, &fault.message, Some(fault.code))?))?,
        Err(Thrown) => scope.settle(&resolver, Err(scope.exception()))?,
    }
    Ok(promise)
}

/// Settles an asynchronous request with the pool's result.
///
/// # Arguments
/// * `node` - the kept builtins and pending requests.
/// * `scope` - the context handle.
/// * `req` - the request id.
/// * `result` - the native value, or the fault's message and code.
///
/// # Errors
/// [`Thrown`] when a value cannot be created.
pub(crate) fn done<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, req: i32, result: Result<V::Value<'s>, (String, String)>) -> Outcome<()> {
    let Some((resolver, code)) = node.fs.waiting.borrow_mut().remove(&req) else { return Ok(()) };
    let outcome = match result {
        Ok(value) => Ok(shape(node, scope, code, value)?),
        Err((message, fault)) => Err(super::coded_error::<V>(scope, &message, Some(&fault))?),
    };
    scope.settle(&resolver, outcome)
}

/// Builds `fs.Stats`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `node` - the kept builtins; the class is kept as [`Slot::Stats`].
///
/// # Returns
/// The class.
///
/// # Errors
/// [`Thrown`] when a value cannot be created.
fn stats<'s, V: Vm>(scope: &V::Scope<'s>, node: &Rc<Node<V>>) -> Outcome<V::Value<'s>> {
    let kind_of = |call: &Call<'_, V>| call.scope().host_data::<PathKind, _>(call.this(), |kind| kind.0).unwrap_or(0.0);
    let is_file = scope.function(IS_FILE.name, move |call: &Call<'_, V>| Ok(call.scope().boolean(kind_of(call) == 1.0)))?;
    let is_directory = scope.function(IS_DIRECTORY.name, move |call: &Call<'_, V>| Ok(call.scope().boolean(kind_of(call) == 2.0)))?;
    let n = node.clone();
    let mtime = scope.function("mtime", move |call: &Call<'_, V>| {
        let scope = call.scope();
        let ms = scope.get(call.this(), "mtimeMs")?;
        n.construct(scope, &n.get(scope, Slot::Date), &[ms])
    })?;
    let spec = ClassSpec::new(STATS.name).method(IS_FILE.name, is_file).method(IS_DIRECTORY.name, is_directory).accessor("mtime", Some(mtime), None);
    let n = node.clone();
    // `size` and `mtimeMs` are own properties, as in the glue; the kind is the
    // instance's Rust data, so it no longer shows as an enumerable `_kind`.
    let class = scope.class::<PathKind, _>(spec, move |call: &Call<'_, V>| {
        let scope = call.scope();
        let raw = super::arg::<V>(scope, call.args(), 0);
        let this = call.this();
        scope.set(this, "size", n.at(scope, &raw, 0)?)?;
        scope.set(this, "mtimeMs", n.at(scope, &raw, 1)?)?;
        Ok(PathKind(scope.as_number(&n.at(scope, &raw, 2)?).unwrap_or(0.0)))
    })?;
    node.keep(scope, Slot::Stats, &class);
    Ok(class)
}

/// Builds `node:fs` and `node:fs/promises`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state: the host's policy and the pool.
/// * `node` - the kept builtins and pending requests.
///
/// # Returns
/// `(fs, fs/promises)`; the second is `fs.promises`.
///
/// # Errors
/// [`Thrown`] when a value cannot be created.
pub(crate) fn install<'s, V: Vm, H: Host>(scope: &V::Scope<'s>, shared: &Rc<Shared<H>>, node: &Rc<Node<V>>) -> Outcome<(V::Value<'s>, V::Value<'s>)> {
    let promises = scope.object()?;
    let fs = scope.object()?;
    scope.set(&fs, "promises", promises.clone())?;
    scope.set(&fs, STATS.name, stats(scope, node)?)?;
    let (s, n) = (shared.clone(), node.clone());
    let exists = scope.function("existsSync", move |call: &Call<'_, V>| {
        let scope = call.scope();
        let path = n.text(scope, &super::arg::<V>(scope, call.args(), 0))?;
        match fs_run(&s, op::EXISTS, &path, &[], 0) {
            Ok(Some(value)) => fs_value::<V>(scope, value),
            Ok(None) => Ok(scope.undefined()),
            Err(fault) => Err(scope.throw(super::coded_error::<V>(scope, &fault.message, Some(fault.code))?)),
        }
    })?;
    scope.set(&fs, "existsSync", exists)?;
    for (api, name, sync_name, ..) in API {
        let (s, n) = (shared.clone(), node.clone());
        let promise = scope.function(name, move |call: &Call<'_, V>| run(&n, &s, call.scope(), api, Mode::Promise, call.args()))?;
        scope.set(&promises, name, promise)?;
        let (s, n) = (shared.clone(), node.clone());
        let sync = scope.function(sync_name, move |call: &Call<'_, V>| run(&n, &s, call.scope(), api, Mode::Sync, call.args()))?;
        scope.set(&fs, sync_name, sync)?;
        // Callback flavour: the last argument is the Node-style callback.
        let (s, n) = (shared.clone(), node.clone());
        let callback = scope.function(name, move |call: &Call<'_, V>| {
            let scope = call.scope();
            let args = call.args();
            let Some((cb, rest)) = args.split_last().filter(|(cb, _)| scope.kind(cb) == Kind::Function) else {
                return Err(scope.throw_error(ErrorKind::Type, "The \"cb\" argument must be a function"));
            };
            let promise = run(&n, &s, scope, api, Mode::Promise, rest)?;
            let ok = scope.call_with(&n.get(scope, Slot::Bind), cb, &[scope.undefined(), scope.null()])?;
            super::method::<V>(scope, &promise, "then", &[ok, cb.clone()])?;
            Ok(scope.undefined())
        })?;
        scope.set(&fs, name, callback)?;
    }
    Ok((fs, promises))
}
