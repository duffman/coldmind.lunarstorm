//! The Runtime's own native functions, exposed to the JS glue as one `ops` object.
//!
//! The surface is deliberately small and low-level: the Node API (EventEmitter,
//! Socket, Server, request/response, fs) is built on top of it in `js/bootstrap.js`.
//! Each op borrows the loop state for the duration of one call and never calls
//! back into JS, so ops cannot re-enter each other.
//!
//! Decisions:
//! - Ops are written against the VM contract only. Each is a closure in the
//!   contract's native function signature; with the Runtime monomorphized over one
//!   engine they compile to direct calls into that engine.
//! - Strings and `Uint8Array`s passed from JS are borrowed in place
//!   (`Scope::with_str`, `Scope::with_bytes`) rather than copied; with a vectored
//!   write the bytes go from the JS heap to the socket with no intermediate copy.
//! - Every op is declared ([`declare`]): its arguments are checked against the
//!   declaration before the body runs, and a mismatch throws
//!   `TypeError: <op>: <param> must be <type>`. The readers below then only
//!   extract; their fallbacks cover optional parameters that were left out.
//! - Everything that reaches outside the process boundary of the Runtime goes
//!   through the host: stdio, exit, environment, clock, entropy, and the fs/net
//!   policy check that precedes every filesystem and socket operation.

use crate::fs::{self as hfs, op as fsop, FsValue};
use crate::host::{Access, Host, Stream};
use crate::io::{Fault, Shared};
use crate::pool::Work;
use crate::bindings::declared;
use crate::modules::INTERNAL_MODULE;
use crate::runtime::Arg;
use lunarstorm_vm::{Call, ErrorKind, Field, FnDecl, Item, Kind, ModuleDecl, Outcome, Param, Scope, Sig, Thrown, Ty, Vm};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::rc::Rc;

/// Largest `crypto.getRandomValues` request, as in the Web Crypto specification.
pub(crate) const ENTROPY_MAX: usize = 65_536;

/// Reads a numeric argument the declaration has checked.
///
/// # Arguments
/// * `call` - the native call.
/// * `index` - argument position.
///
/// # Returns
/// The number; 0 for an optional argument left out.
#[inline]
fn num<V: Vm>(call: &Call<'_, V>, index: usize) -> f64 {
    call.args().get(index).and_then(|value| call.scope().as_number(value)).unwrap_or(0.0)
}

/// Reads an integer argument the declaration has checked.
///
/// # Arguments
/// * `call` - the native call.
/// * `index` - argument position.
///
/// # Returns
/// The number truncated to `i32`.
#[inline]
fn int<V: Vm>(call: &Call<'_, V>, index: usize) -> i32 {
    num(call, index) as i32
}

/// Reads a boolean argument the declaration has checked.
///
/// # Arguments
/// * `call` - the native call.
/// * `index` - argument position.
///
/// # Returns
/// The boolean.
#[inline]
fn flag<V: Vm>(call: &Call<'_, V>, index: usize) -> bool {
    call.args().get(index).and_then(|value| call.scope().as_bool(value)).unwrap_or(false)
}

/// Lends the UTF-8 text of a string argument the declaration has checked.
///
/// # Arguments
/// * `call` - the native call.
/// * `index` - argument position.
/// * `f` - receives the text.
///
/// # Returns
/// The closure's result.
///
/// # Errors
/// A `TypeError` for a string with an unpaired surrogate (no UTF-8 form).
#[inline]
fn text<V: Vm, R>(call: &Call<'_, V>, index: usize, f: impl FnOnce(&str) -> R) -> Outcome<R> {
    match call.args().get(index) {
        Some(value) => call.scope().with_str(value, f),
        None => Err(call.scope().throw_error(ErrorKind::Type, "expected a string")),
    }
}

/// Lends the bytes of a [`DATA`] argument the declaration has checked.
///
/// # Arguments
/// * `call` - the native call.
/// * `index` - argument position.
/// * `f` - receives the bytes; empty for a missing or nullish argument.
///
/// # Returns
/// The closure's result.
///
/// # Errors
/// What the engine throws when it cannot lend the bytes.
#[inline]
fn data<V: Vm, R>(call: &Call<'_, V>, index: usize, f: impl FnOnce(&[u8]) -> R) -> Outcome<R> {
    match call.args().get(index) {
        Some(value) => call.scope().with_bytes(value, f),
        None => Ok(f(&[])),
    }
}

/// Throws a fault as a JS `Error` with a `code` property.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `fault` - the fault to throw.
///
/// # Returns
/// The marker to return from the op.
pub fn throw_fault<'s, V: Vm>(scope: &V::Scope<'s>, fault: Fault) -> Thrown {
    match scope.error(ErrorKind::Error, &fault.message) {
        Ok(error) => {
            if let Ok(code) = scope.string(fault.code) {
                let _ = scope.set(&error, "code", code);
            }
            scope.throw(error)
        }
        Err(thrown) => thrown,
    }
}

/// Writes text to stdout (buffered) or stderr: what `ops.print` and `console` do.
///
/// # Arguments
/// * `shared` - loop state: the stdout buffer and the host.
/// * `fd` - 2 for stderr, anything else for stdout.
/// * `text` - the bytes to write.
pub(crate) fn print<H: Host>(shared: &Shared<H>, fd: i32, text: &[u8]) {
    let mut io = shared.io.borrow_mut();
    if fd == 2 {
        // Keep stdout and stderr in program order when both reach a terminal.
        io.flush_stdout(&shared.host);
        shared.host.write(Stream::Err, text);
    } else {
        io.print(text, &shared.host);
    }
}

/// # Returns
/// The OS, spelled as Node spells it (`darwin`, `win32`, `linux`).
pub(crate) fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// # Returns
/// The CPU, spelled as Node spells it (`arm64`, `x64`).
pub(crate) fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    }
}

/// Builds the fault a script sees when the host's policy refuses an operation.
///
/// # Arguments
/// * `call` - the operation name, as in Node's messages (`open`, `listen`).
/// * `detail` - the path or address.
///
/// # Returns
/// An `EACCES` fault.
fn denied(call: &str, detail: &str) -> Fault {
    Fault { code: "EACCES", message: format!("EACCES: permission denied by host policy, {call} '{detail}'") }
}

/// Asks the host's policy about a filesystem operation.
///
/// Decision: operations that only observe (`read`, `stat`, `readdir`, `exists`)
/// need [`Access::Read`]; everything else needs [`Access::Write`]. A rename needs
/// write access to both ends.
///
/// # Arguments
/// * `host` - the host.
/// * `op` - operation code from [`crate::fs::op`].
/// * `path` - the path operated on.
/// * `extra` - the operation's data; the destination path of a rename.
///
/// # Returns
/// `Ok` when allowed.
///
/// # Errors
/// The `EACCES` fault to throw.
fn fs_allowed<H: Host>(host: &H, op: i32, path: &str, extra: &[u8]) -> Result<(), Fault> {
    let reads = matches!(op, fsop::READ | fsop::READ_UTF8 | fsop::STAT | fsop::READDIR | fsop::EXISTS);
    let allowed = if reads { host.allow(Access::Read(path)) } else { host.allow(Access::Write(path)) };
    if !allowed {
        return Err(denied(if reads { "open" } else { "write" }, path));
    }
    if op == fsop::RENAME {
        let target = String::from_utf8_lossy(extra);
        if !host.allow(Access::Write(&target)) {
            return Err(denied("rename", &target));
        }
    }
    Ok(())
}

/// Converts a filesystem result into a JS value.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `value` - the result; byte buffers are moved into JS without copying.
///
/// # Returns
/// `undefined`, a boolean, a `Uint8Array`, a string, a `[size, mtimeMs, kind]` array
/// or an array of names.
///
/// # Errors
/// [`Thrown`] when allocation fails.
pub fn fs_value<'s, V: Vm>(scope: &V::Scope<'s>, value: FsValue) -> Outcome<V::Value<'s>> {
    match value {
        FsValue::Unit => Ok(scope.undefined()),
        FsValue::Bool(flag) => Ok(scope.boolean(flag)),
        FsValue::Bytes(bytes) => scope.bytes(bytes),
        FsValue::Text(text) => scope.string(&text),
        FsValue::Stat { size, mtime_ms, kind } => {
            scope.array(&[scope.number(size as f64), scope.number(mtime_ms), scope.number(f64::from(kind))])
        }
        FsValue::Names(names) => {
            let names = names.iter().map(|name| scope.string(name)).collect::<Outcome<Vec<_>>>()?;
            scope.array(&names)
        }
    }
}

/// Converts a settled `Runtime::call` result into an engine-neutral value.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `value` - the value the glue passed; objects were serialized to JSON there.
///
/// # Returns
/// The neutral value; anything unrepresentable becomes [`Arg::Null`].
fn neutral<'s, V: Vm>(scope: &V::Scope<'s>, value: &V::Value<'s>) -> Arg {
    match scope.kind(value) {
        Kind::Bool => Arg::Bool(scope.as_bool(value).unwrap_or(false)),
        Kind::Number => Arg::Number(scope.as_number(value).unwrap_or(0.0)),
        Kind::String => scope.with_str(value, |text| Arg::Text(text.to_owned())).unwrap_or(Arg::Null),
        Kind::Bytes => scope.with_bytes(value, |bytes| Arg::Bytes(bytes.to_vec())).unwrap_or(Arg::Null),
        _ => Arg::Null,
    }
}

/// Turns a `listen` host into a bind address.
///
/// # Arguments
/// * `host` - an address literal, `localhost`, or a name for the system resolver.
/// * `port` - port to bind.
///
/// # Returns
/// The address to bind.
///
/// # Errors
/// The resolver error for an unknown name.
fn bind_addr(host: &str, port: u16) -> std::io::Result<SocketAddr> {
    if let Some(ip) = literal(host) {
        return Ok(SocketAddr::new(ip, port));
    }
    (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::AddrNotAvailable))
}

/// Parses a host that needs no resolver.
///
/// # Arguments
/// * `host` - host string from JS.
///
/// # Returns
/// The address for an IP literal or `localhost`; `None` for any other name.
fn literal(host: &str) -> Option<IpAddr> {
    if host == "localhost" {
        return Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    host.parse().ok()
}

/// A payload: text (sent as UTF-8) or bytes.
///
/// Decision: `null` and `undefined` are admitted and mean no bytes, because the
/// glue passes the script's value through unchanged: `res.end()` reaches
/// `httpRespond` with `undefined`, `fs.writeFileSync(p)` reaches `fs` with it,
/// and `socket.write(null)` reaches `write`. Narrowing it would change what
/// those scripts see.
const DATA: Ty = Ty::Union(&[Ty::String, Ty::Bytes, Ty::Null, Ty::Undefined]);

/// Shorthand for a [`Param`] in the declarations below.
///
/// # Arguments
/// * `name` - the parameter name.
/// * `ty` - its type.
/// * `about` - one line.
///
/// # Returns
/// The parameter.
const fn p(name: &'static str, ty: Ty, about: &'static str) -> Param {
    Param { name, ty, about }
}

/// Shorthand for an op's [`FnDecl`] with no rest parameter.
///
/// # Arguments
/// * `name` - the op's name on `ops`.
/// * `params` - its parameters.
/// * `returns` - its result.
/// * `about` - one line.
///
/// # Returns
/// The declaration.
const fn op(name: &'static str, params: &'static [Param], returns: Ty, about: &'static str) -> FnDecl {
    FnDecl { name, sig: Sig { params, rest: None, returns, about } }
}

/// The handle key of a socket, listener or HTTP exchange.
const KEY: Param = p("key", Ty::Number, "handle key from listen, connect or a connection event");

const ENV: FnDecl = op("env", &[], Ty::Object(&[]), "The host's environment variables as an object.");
const CWD: FnDecl = op("cwd", &[], Ty::String, "The host's working directory.");
const EXIT: FnDecl = op("exit", &[p("code", Ty::Number, "exit status")], Ty::Undefined, "Flushes stdout and asks the host to exit.");
const NOW: FnDecl = op("now", &[], Ty::Number, "Milliseconds since the Runtime started, from the host's monotonic clock.");
const PRINT: FnDecl = op(
    "print",
    &[p("fd", Ty::Number, "2 for stderr, anything else for stdout"), p("text", Ty::String, "what to write")],
    Ty::Undefined,
    "Writes text to stdout (buffered) or stderr.",
);
const FLUSH: FnDecl = op("flush", &[], Ty::Undefined, "Writes buffered stdout to the host.");
const RANDOM: FnDecl = op("random", &[p("length", Ty::Number, "byte count, at most 65536")], Ty::Bytes, "Bytes from the host's entropy source.");
const SETTLED: FnDecl = op(
    "settled",
    &[
        p("id", Ty::Number, "the host's call id"),
        p("ok", Ty::Bool, "whether the call's promise fulfilled"),
        p("value", Ty::Any, "the result, or the rejection's description"),
    ],
    Ty::Undefined,
    "Reports how a Runtime::call settled.",
);
const TICK: FnDecl = op("tick", &[], Ty::Undefined, "Asks the loop to drain the nextTick queue.");
const IMMEDIATE: FnDecl = op("immediate", &[], Ty::Undefined, "Asks the loop to run the setImmediate queue.");
const TIMER_START: FnDecl = op(
    "timerStart",
    &[p("ms", Ty::Number, "delay in milliseconds"), p("repeat", Ty::Bool, "whether the timer repeats")],
    Ty::Number,
    "Starts a timer and returns its id.",
);
const TIMER_CANCEL: FnDecl = op("timerCancel", &[p("id", Ty::Number, "timer id")], Ty::Bool, "Cancels a timer; whether it was active.");
const UTF8_DECODE: FnDecl = op("utf8Decode", &[p("bytes", Ty::Bytes, "UTF-8 bytes")], Ty::String, "Decodes UTF-8, replacing invalid sequences.");
const UTF8_ENCODE: FnDecl = op("utf8Encode", &[p("text", Ty::String, "text")], Ty::Bytes, "Encodes text as UTF-8.");
const LISTEN: FnDecl = op(
    "listen",
    &[p("host", Ty::String, "address or name to bind"), p("port", Ty::Number, "port, 0 for any"), p("http", Ty::Bool, "whether connections speak HTTP/1.1")],
    Ty::Array(&Ty::Number),
    "Binds a TCP listener; returns [key, port].",
);
const LISTENER_CLOSE: FnDecl = op("listenerClose", &[KEY], Ty::Undefined, "Closes a listener.");
const CONNECT: FnDecl = op(
    "connect",
    &[p("host", Ty::String, "address or name"), p("port", Ty::Number, "port")],
    Ty::Number,
    "Starts a TCP connection; returns its key.",
);
const WRITE: FnDecl = op("write", &[KEY, p("data", DATA, "bytes to send")], Ty::Bool, "Queues bytes on a socket; false when the caller should wait for drain.");
const END: FnDecl = op("end", &[KEY], Ty::Undefined, "Half-closes a socket after its queued bytes.");
const DESTROY: FnDecl = op("destroy", &[KEY], Ty::Undefined, "Closes a socket at once.");
const HTTP_RESPOND: FnDecl = op(
    "httpRespond",
    &[
        KEY,
        p("status", Ty::Number, "status code"),
        p("fields", Ty::String, "header lines"),
        p("body", DATA, "the whole body"),
        p("close", Ty::Bool, "whether to close the connection after"),
    ],
    Ty::Undefined,
    "Sends a complete response.",
);
const HTTP_HEAD: FnDecl = op(
    "httpHead",
    &[KEY, p("status", Ty::Number, "status code"), p("fields", Ty::String, "header lines"), p("close", Ty::Bool, "whether to close the connection after")],
    Ty::Undefined,
    "Sends the head of a chunked response.",
);
const HTTP_CHUNK: FnDecl = op("httpChunk", &[KEY, p("body", DATA, "a piece of the body")], Ty::Undefined, "Sends one chunk of a chunked response.");
const HTTP_END: FnDecl = op("httpEnd", &[KEY], Ty::Undefined, "Ends a chunked response.");
const FS: FnDecl = op(
    "fs",
    &[
        p("op", Ty::Number, "operation code"),
        p("path", Ty::String, "the path operated on"),
        p("data", DATA, "payload: file contents, or a rename's target"),
        p("req", Ty::Number, "0 to run inline, else the request id of the async result"),
    ],
    Ty::Any,
    "Runs a filesystem operation inline or on the worker pool.",
);

/// Every op, in the order [`install`] sets them on `ops`.
const OPS: [FnDecl; 25] = [
    ENV, CWD, EXIT, NOW, PRINT, FLUSH, RANDOM, SETTLED, TICK, IMMEDIATE, TIMER_START, TIMER_CANCEL, UTF8_DECODE, UTF8_ENCODE,
    LISTEN, LISTENER_CLOSE, CONNECT, WRITE, END, DESTROY, HTTP_RESPOND, HTTP_HEAD, HTTP_CHUNK, HTTP_END, FS,
];

/// The constant values on `ops`.
const VALUES: [Field; 5] = [
    Field { name: "argv", ty: Ty::Array(&Ty::String), about: "process.argv, from the host." },
    Field { name: "pid", ty: Ty::Number, about: "The process id." },
    Field { name: "version", ty: Ty::String, about: "The Runtime's version." },
    Field { name: "platform", ty: Ty::String, about: "The OS, spelled as Node spells it (darwin, win32, linux)." },
    Field { name: "arch", ty: Ty::String, about: "The CPU, spelled as Node spells it (arm64, x64)." },
];

/// Declares the Runtime's `ops` object as the module host modules import it from.
///
/// # Returns
/// `lunarstorm:internal` with the constant values, then every op.
pub fn declare() -> ModuleDecl {
    let items = VALUES.iter().copied().map(Item::Value).chain(OPS.iter().copied().map(Item::Function)).collect();
    ModuleDecl { specifier: INTERNAL_MODULE, items, about: "The Runtime's natives, for host modules only." }
}

/// Builds the `ops` object.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state and host the ops operate on.
///
/// # Returns
/// The object passed to the bootstrap script. Host bindings are added to it next.
///
/// # Errors
/// [`Thrown`] when a function or property cannot be created.
pub fn install<'s, V: Vm, H: Host>(scope: &V::Scope<'s>, shared: &Rc<Shared<H>>) -> Outcome<V::Value<'s>> {
    let ops = scope.object()?;

    /// Registers one declared op: clones the shared state into the closure and
    /// sets it on `ops`. The body sees `$s` (the shared state) and `$call` (the
    /// native call), and runs only when the arguments fit the declaration.
    macro_rules! op {
        ($decl:ident, |$s:ident, $call:ident| $body:expr) => {{
            let $s = shared.clone();
            let function = declared(scope, $decl.name, $decl.sig, move |$call: &Call<'_, V>| $body)?;
            scope.set(&ops, $decl.name, function)?;
        }};
    }

    // ---- process ----------------------------------------------------------------
    let argv = shared.host.args().iter().map(|arg| scope.string(arg)).collect::<Outcome<Vec<_>>>()?;
    scope.set(&ops, "argv", scope.array(&argv)?)?;
    scope.set(&ops, "pid", scope.number(f64::from(std::process::id())))?;
    scope.set(&ops, "version", scope.string(env!("CARGO_PKG_VERSION"))?)?;
    scope.set(&ops, "platform", scope.string(platform())?)?;
    scope.set(&ops, "arch", scope.string(arch())?)?;

    op!(ENV, |s, call| {
        let scope = call.scope();
        let env = scope.object()?;
        for (name, value) in s.host.environment() {
            scope.set(&env, &name, scope.string(&value)?)?;
        }
        Ok(env)
    });
    op!(CWD, |s, call| call.scope().string(&s.host.working_directory()));
    op!(EXIT, |s, call| {
        s.io.borrow_mut().flush_stdout(&s.host);
        s.host.exit(int(call, 0))
    });
    op!(NOW, |s, call| {
        Ok(call.scope().number(s.host.monotonic().duration_since(s.start).as_secs_f64() * 1000.0))
    });
    op!(PRINT, |s, call| {
        text(call, 1, |line| print(&s, int(call, 0), line.as_bytes()))?;
        Ok(call.scope().undefined())
    });
    op!(FLUSH, |s, call| {
        s.io.borrow_mut().flush_stdout(&s.host);
        Ok(call.scope().undefined())
    });
    op!(RANDOM, |s, call| {
        let scope = call.scope();
        let len = num(call, 0);
        if !(0.0..=ENTROPY_MAX as f64).contains(&len) {
            return Err(scope.throw_error(ErrorKind::Range, "getRandomValues: at most 65536 bytes per call"));
        }
        let mut bytes = vec![0u8; len as usize];
        s.host.entropy(&mut bytes);
        scope.bytes(bytes)
    });
    op!(SETTLED, |s, call| {
        let scope = call.scope();
        let result = match call.args().get(2) {
            Some(value) if flag(call, 1) => Ok(neutral::<V>(scope, value)),
            Some(value) => Err(scope.with_str(value, str::to_owned).unwrap_or_default()),
            None => Ok(Arg::Null),
        };
        s.calls.borrow_mut().push((int(call, 0) as u32, result));
        Ok(scope.undefined())
    });

    // ---- scheduling -------------------------------------------------------------
    op!(TICK, |s, call| {
        s.tick.set(true);
        Ok(call.scope().undefined())
    });
    op!(IMMEDIATE, |s, call| {
        s.immediate.set(true);
        Ok(call.scope().undefined())
    });
    op!(TIMER_START, |s, call| {
        let scope = call.scope();
        match s.io.borrow_mut().timer_start(num(call, 0), flag(call, 1), s.host.monotonic()) {
            Some(id) => Ok(scope.number(id)),
            None => Err(scope.throw_error(ErrorKind::Range, "too many active timers")),
        }
    });
    op!(TIMER_CANCEL, |s, call| Ok(call.scope().boolean(s.io.borrow_mut().timer_cancel(num(call, 0)))));

    // ---- text -------------------------------------------------------------------
    op!(UTF8_DECODE, |_s, call| {
        let scope = call.scope();
        let decoded = data(call, 0, |bytes| String::from_utf8_lossy(bytes).into_owned())?;
        scope.string(&decoded)
    });
    op!(UTF8_ENCODE, |_s, call| {
        let scope = call.scope();
        text(call, 0, |text| scope.bytes_copy(text.as_bytes()))?
    });

    // ---- tcp --------------------------------------------------------------------
    op!(LISTEN, |s, call| {
        let scope = call.scope();
        let (port, http) = (int(call, 1), flag(call, 2));
        let bound = text(call, 0, |host| {
            let fault = |e: std::io::Error| Fault::new(&e, "listen", &format!("{host}:{port}"));
            let addr = bind_addr(host, port as u16).map_err(fault)?;
            if !s.host.allow(Access::Listen(addr)) {
                return Err(denied("listen", &addr.to_string()));
            }
            s.io.borrow_mut().listen(addr, http).map_err(fault)
        })?;
        match bound {
            Ok((key, port)) => scope.array(&[scope.number(key as f64), scope.number(f64::from(port))]),
            Err(fault) => Err(throw_fault::<V>(scope, fault)),
        }
    });
    op!(LISTENER_CLOSE, |s, call| {
        s.io.borrow_mut().listener_close(int(call, 0) as usize);
        Ok(call.scope().undefined())
    });
    op!(CONNECT, |s, call| {
        let scope = call.scope();
        let port = int(call, 1) as u16;
        let key = text(call, 0, |host| {
            if !s.host.allow(Access::Connect(host, port)) {
                return Err(denied("connect", &format!("{host}:{port}")));
            }
            let mut io = s.io.borrow_mut();
            let key = io.reserve();
            match literal(host) {
                // Address literals connect at once; only real names cost a pool round trip.
                Some(ip) => io.connect(key, Ok(SocketAddr::new(ip, port))),
                None => io.pool.submit(Work::Resolve { conn: key, host: host.to_owned(), port }),
            }
            Ok(key)
        })?;
        match key {
            Ok(key) => Ok(scope.int(key as i32)),
            Err(fault) => Err(throw_fault::<V>(scope, fault)),
        }
    });
    op!(WRITE, |s, call| {
        let key = int(call, 0) as usize;
        let accepted = data(call, 1, |bytes| s.io.borrow_mut().tcp_write(key, bytes))?;
        Ok(call.scope().boolean(accepted))
    });
    op!(END, |s, call| {
        s.io.borrow_mut().tcp_end(int(call, 0) as usize);
        Ok(call.scope().undefined())
    });
    op!(DESTROY, |s, call| {
        s.io.borrow_mut().destroy(int(call, 0) as usize);
        Ok(call.scope().undefined())
    });

    // ---- http -------------------------------------------------------------------
    op!(HTTP_RESPOND, |s, call| {
        let (key, status, close) = (int(call, 0) as usize, int(call, 1) as u16, flag(call, 4));
        text(call, 2, |fields| {
            data(call, 3, |body| s.io.borrow_mut().http_respond(key, status, fields, body, close, &s.host))
        })??;
        Ok(call.scope().undefined())
    });
    op!(HTTP_HEAD, |s, call| {
        let (key, status, close) = (int(call, 0) as usize, int(call, 1) as u16, flag(call, 3));
        text(call, 2, |fields| s.io.borrow_mut().http_head(key, status, fields, close, &s.host))?;
        Ok(call.scope().undefined())
    });
    op!(HTTP_CHUNK, |s, call| {
        let key = int(call, 0) as usize;
        data(call, 1, |bytes| s.io.borrow_mut().http_chunk(key, bytes))?;
        Ok(call.scope().undefined())
    });
    op!(HTTP_END, |s, call| {
        s.io.borrow_mut().http_end(int(call, 0) as usize);
        Ok(call.scope().undefined())
    });

    // ---- fs ---------------------------------------------------------------------
    // `req == 0` runs the operation inline (the `*Sync` API) and returns its value;
    // any other `req` queues it on the worker pool and the result arrives as an
    // event. The host's policy is asked here, on the loop thread, in both cases.
    op!(FS, |s, call| {
        let scope = call.scope();
        let (op, req) = (int(call, 0), int(call, 3));
        let outcome = text(call, 1, |path| {
            data(call, 2, |extra| {
                fs_allowed(&s.host, op, path, extra)?;
                if req == 0 {
                    return hfs::exec(op, path, extra).map(Some);
                }
                s.io.borrow_mut().pool.submit(Work::Fs { req, op, path: path.to_owned(), data: extra.to_vec() });
                Ok(None)
            })
        })??;
        match outcome {
            Ok(Some(value)) => fs_value::<V>(scope, value),
            Ok(None) => Ok(scope.undefined()),
            Err(fault) => Err(throw_fault::<V>(scope, fault)),
        }
    });

    Ok(ops)
}
