//! The conformance suite: what every VM adapter must do to satisfy the contract.
//!
//! An adapter runs it from a test:
//!
//! ```text
//! #[test]
//! fn conforms() {
//!     lunarstorm_vm::conformance::run::<MyVm>();
//! }
//! ```
//!
//! Decisions:
//! - The suite is generic code compiled against the adapter, so it exercises the
//!   same monomorphized paths the Runtime uses.
//! - Each check uses a fresh [`Instance`]; a failure panics with the check's name.
//! - Checks for optional abilities run only when [`Vm::capabilities`] reports them.

use crate::{
    Callbacks, Context, ErrorKind, Instance, Kind, Limits, Load, ModuleMeta, ModuleSource, Scope, Vm,
    CONTRACT_VERSION,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What the suite's callbacks observed.
#[derive(Default)]
struct Seen {
    /// `(promise identity, handled)` for every rejection report, in order.
    rejections: RefCell<Vec<(usize, bool)>>,
    /// Answer of the interrupt check.
    interrupt: Cell<bool>,
}

/// The suite's callbacks: a two-module table and a recorder.
struct Probe(Rc<Seen>);

impl<V: Vm> Callbacks<V> for Probe {
    /// Resolves `dep` to `/dep.js` and rejects everything else.
    ///
    /// # Arguments
    /// * `_scope` - unused.
    /// * `_base` - unused.
    /// * `name` - the specifier.
    ///
    /// # Returns
    /// The canonical name.
    ///
    /// # Errors
    /// `"unknown module"` for any other specifier.
    fn resolve<'s>(&self, _scope: &V::Scope<'s>, _base: &str, name: &str) -> Result<String, String> {
        if name == "dep" {
            Ok(String::from("/dep.js"))
        } else {
            Err(String::from("unknown module"))
        }
    }

    /// Serves `/dep.js`.
    ///
    /// # Arguments
    /// * `_scope` - unused.
    /// * `name` - canonical name.
    ///
    /// # Returns
    /// The module source.
    ///
    /// # Errors
    /// `"unreadable"` for any other name.
    fn load<'s>(&self, _scope: &V::Scope<'s>, name: &str) -> Result<Load, String> {
        if name == "/dep.js" {
            Ok(Load::Ready(ModuleSource { source: b"export const n = 41;".to_vec(), meta: None, bytecode: false, source_map: None }))
        } else {
            Err(String::from("unreadable"))
        }
    }

    /// Records the report.
    ///
    /// # Arguments
    /// * `_scope` - unused.
    /// * `promise` - identity of the promise.
    /// * `_reason` - unused.
    /// * `handled` - whether a handler was attached.
    fn rejection<'s>(&self, _scope: &V::Scope<'s>, promise: usize, _reason: V::Value<'s>, handled: bool) {
        self.0.rejections.borrow_mut().push((promise, handled));
    }

    /// # Returns
    /// The answer the current check configured.
    fn interrupt(&self) -> bool {
        self.0.interrupt.get()
    }
}

/// Sets a flag when dropped; the payload of the host-object check.
struct Finalized(Rc<Cell<bool>>);

impl Drop for Finalized {
    /// Records that the finalizer ran.
    fn drop(&mut self) {
        self.0.set(true);
    }
}

/// Creates an instance with the suite's callbacks.
///
/// # Arguments
/// * `limits` - limits for the instance.
///
/// # Returns
/// The instance and what its callbacks record.
fn boot<V: Vm>(limits: &Limits) -> (V::Instance, Rc<Seen>) {
    let seen = Rc::new(Seen::default());
    let instance = V::Instance::new(limits, Probe(seen.clone())).expect("conformance: instance creation");
    (instance, seen)
}

/// Fails the suite when a condition does not hold.
///
/// # Arguments
/// * `name` - the check's name.
/// * `holds` - the condition.
fn check(name: &str, holds: bool) {
    assert!(holds, "conformance: {name}");
}

/// # Arguments
/// * `scope` - the scope.
/// * `value` - a string value.
///
/// # Returns
/// The string's text; fails the suite when the value is not a string.
fn text<'s, V: Vm>(scope: &V::Scope<'s>, value: &V::Value<'s>) -> String {
    scope.with_str(value, str::to_owned).expect("conformance: expected a string")
}

/// Evaluates a script that must not throw.
///
/// # Arguments
/// * `scope` - the scope.
/// * `source` - the script.
///
/// # Returns
/// Its completion value.
fn eval<'s, V: Vm>(scope: &V::Scope<'s>, source: &str) -> V::Value<'s> {
    scope.eval(source).unwrap_or_else(|_| panic!("conformance: script threw: {source}"))
}

/// Runs the whole suite against an adapter.
///
/// # Returns
/// The number of checks that ran (optional ones are skipped when unsupported).
pub fn run<V: Vm>() -> usize {
    let checks: &[fn()] = &[
        descriptor::<V>,
        values::<V>,
        borrowed_data::<V>,
        objects::<V>,
        calls_and_exceptions::<V>,
        native_functions::<V>,
        persistent_handles::<V>,
        jobs::<V>,
        modules::<V>,
        bytecode::<V>,
        rejections::<V>,
        host_objects::<V>,
        limits::<V>,
        interrupts::<V>,
    ];
    for run in checks {
        run();
    }
    checks.len()
}

/// The adapter identifies itself and implements this contract version.
fn descriptor<V: Vm>() {
    let descriptor = V::descriptor();
    check("descriptor names the adapter and engine", !descriptor.adapter.is_empty() && !descriptor.engine.is_empty());
    check("descriptor reports this contract version", descriptor.contract == CONTRACT_VERSION);
}

/// Primitive constructors, kinds and extractors round-trip.
fn values<V: Vm>() {
    let (instance, _) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    context.enter(|scope| {
        check("undefined", scope.kind(&scope.undefined()) == Kind::Undefined);
        check("null", scope.kind(&scope.null()) == Kind::Null);
        check("boolean", scope.as_bool(&scope.boolean(true)) == Some(true));
        check("int", scope.as_number(&scope.int(-7)) == Some(-7.0));
        check("number", scope.as_number(&scope.number(1.5)) == Some(1.5));
        check("number is not a boolean", scope.as_bool(&scope.int(1)).is_none());
        let string = scope.string("h\u{e9}llo \u{263e}").expect("conformance: string");
        check("string kind", scope.kind(&string) == Kind::String);
        check("string round trip", text::<V>(&scope, &string) == "h\u{e9}llo \u{263e}");
        check("with_str rejects a number", scope.with_str(&scope.int(1), |_| ()).is_err());
        let thrown = scope.exception();
        let name = scope.get(&thrown, "name").expect("conformance: error name");
        check("with_str throws a TypeError", text::<V>(&scope, &name) == "TypeError");
        check("exception is cleared once taken", scope.kind(&scope.exception()) == Kind::Undefined);
    });
}

/// Strings and byte arrays are lent in place; buffers are adopted without copying.
fn borrowed_data<V: Vm>() {
    let (instance, _) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    context.enter(|scope| {
        let buffer = vec![1u8, 2, 3, 250];
        let address = buffer.as_ptr() as usize;
        let adopted = scope.bytes(buffer).expect("conformance: bytes");
        check("bytes kind", scope.kind(&adopted) == Kind::Bytes);
        let seen = scope.with_bytes(&adopted, |bytes| (bytes.as_ptr() as usize, bytes.to_vec()));
        check("bytes adopts the buffer without copying", seen == Ok((address, vec![1, 2, 3, 250])));
        let copied = scope.bytes_copy(&[9, 8]).expect("conformance: bytes_copy");
        check("bytes_copy", scope.with_bytes(&copied, <[u8]>::to_vec) == Ok(vec![9, 8]));
        let string = scope.string("a\u{e9}").expect("conformance: string");
        check("with_bytes lends a string as UTF-8", scope.with_bytes(&string, <[u8]>::to_vec) == Ok(vec![97, 0xc3, 0xa9]));
        check("with_bytes lends nothing for undefined", scope.with_bytes(&scope.undefined(), <[u8]>::len) == Ok(0));
        check("with_bytes rejects a number", scope.with_bytes(&scope.int(1), |_| ()).is_err());
        scope.exception();
    });
}

/// Objects, arrays, properties and the global object.
fn objects<V: Vm>() {
    let (instance, _) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    context.enter(|scope| {
        let object = scope.object().expect("conformance: object");
        scope.set(&object, "answer", scope.int(42)).expect("conformance: set");
        let answer = scope.get(&object, "answer").expect("conformance: get");
        check("property round trip", scope.as_number(&answer) == Some(42.0));
        let missing = scope.get(&object, "missing").expect("conformance: get");
        check("absent property is undefined", scope.kind(&missing) == Kind::Undefined);
        check("object kind", scope.kind(&object) == Kind::Object);
        let array = scope.array(&[scope.int(1), scope.boolean(false)]).expect("conformance: array");
        let length = scope.get(&array, "length").expect("conformance: length");
        check("array length", scope.as_number(&length) == Some(2.0));
        let second = scope.get(&array, "1").expect("conformance: element");
        check("array element", scope.as_bool(&second) == Some(false));
        scope.set(&scope.global(), "shared", scope.int(5)).expect("conformance: set global");
        check("global object is the script's global", scope.as_number(&eval::<V>(&scope, "shared + 1")) == Some(6.0));
        check("identity is stable", scope.identity(&object) == scope.identity(&object.clone()));
        check("identity distinguishes objects", scope.identity(&object) != scope.identity(&array));
    });
}

/// Scripts, calls into JS, and exceptions coming back out.
fn calls_and_exceptions<V: Vm>() {
    let (instance, _) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    context.enter(|scope| {
        let add = eval::<V>(&scope, "(function (a, b) { return this === undefined ? a + b : -1; })");
        check("function kind", scope.kind(&add) == Kind::Function);
        let sum = scope.call(&add, &[scope.int(2), scope.int(3)]).expect("conformance: call");
        check("call passes arguments and an undefined this", scope.as_number(&sum) == Some(5.0));
        let many: Vec<V::Value<'_>> = (0..12).map(|i| scope.int(i)).collect();
        let count = eval::<V>(&scope, "(function () { return arguments.length; })");
        let counted = scope.call(&count, &many).expect("conformance: call");
        check("call passes more than eight arguments", scope.as_number(&counted) == Some(12.0));
        check("calling a non-function throws", scope.call(&scope.int(1), &[]).is_err());
        scope.exception();
        check("a throwing script reports Thrown", scope.eval("throw new RangeError('far')").is_err());
        let thrown = scope.exception();
        let message = scope.get(&thrown, "message").expect("conformance: message");
        check("the thrown value is retrievable", text::<V>(&scope, &message) == "far");
        check("a syntax error reports Thrown", scope.eval("let = ;").is_err());
        scope.exception();
        let error = scope.error(ErrorKind::Reference, "gone").expect("conformance: error");
        let name = scope.get(&error, "name").expect("conformance: name");
        check("error() builds without throwing", text::<V>(&scope, &name) == "ReferenceError");
        check("error() leaves nothing pending", scope.kind(&scope.exception()) == Kind::Undefined);
    });
}

/// The native function signature: slice arguments, return values, thrown errors.
fn native_functions<V: Vm>() {
    let (instance, _) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    let calls = Rc::new(Cell::new(0));
    let counter = calls.clone();
    context.enter(|scope| {
        let sum = scope
            .function("sum", move |call| {
                let (scope, args) = (call.scope(), call.args());
                counter.set(counter.get() + 1);
                let total: f64 = args.iter().filter_map(|arg| scope.as_number(arg)).sum();
                Ok(scope.number(total + args.len() as f64 * 100.0))
            })
            .expect("conformance: function");
        scope.set(&scope.global(), "sum", sum.clone()).expect("conformance: set");
        check("native function name", text::<V>(&scope, &scope.get(&sum, "name").expect("conformance: name")) == "sum");
        check("native call with no arguments", scope.as_number(&eval::<V>(&scope, "sum()")) == Some(0.0));
        check("native call sees every argument", scope.as_number(&eval::<V>(&scope, "sum(1, 2, 3)")) == Some(306.0));
        let twelve = eval::<V>(&scope, "sum(1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1)");
        check("native call with more than eight arguments", scope.as_number(&twelve) == Some(1212.0));
        let fail = scope
            .function("fail", |call| Err(call.scope().throw_error(ErrorKind::Range, "native")))
            .expect("conformance: function");
        scope.set(&scope.global(), "fail", fail).expect("conformance: set");
        let caught = eval::<V>(&scope, "(() => { try { fail(); return 'no throw'; } catch (e) { return e.name + ':' + e.message; } })()");
        check("native throw is catchable in JS", text::<V>(&scope, &caught) == "RangeError:native");
    });
    check("native closure ran once per call", calls.get() == 3);
}

/// A persistent handle survives its scope and resolves in a later one.
fn persistent_handles<V: Vm>() {
    let (instance, _) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    let kept = context.enter(|scope| scope.persist(&eval::<V>(&scope, "(x) => x * 2")));
    instance.collect_garbage();
    context.enter(|scope| {
        let double = scope.local(&kept);
        let result = scope.call(&double, &[scope.int(21)]).expect("conformance: call");
        check("persistent handle survives scopes and GC", scope.as_number(&result) == Some(42.0));
    });
    drop(kept);
}

/// Promise reactions run only through `run_job`, in queue order.
fn jobs<V: Vm>() {
    let (instance, _) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    context.enter(|scope| {
        eval::<V>(&scope, "globalThis.order = []; Promise.resolve().then(() => order.push(1)).then(() => order.push(2)); order.push(0);");
        check("jobs do not run on their own", text::<V>(&scope, &eval::<V>(&scope, "order.join()")) == "0");
        check("run_job reports a job ran", scope.run_job());
        while scope.run_job() {}
        check("jobs run in order", text::<V>(&scope, &eval::<V>(&scope, "order.join()")) == "0,1,2");
        check("run_job reports an empty queue", !scope.run_job());
    });
}

/// Modules: resolve and load callbacks, namespace, `import.meta`, failed imports.
fn modules<V: Vm>() {
    let (instance, _) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    context.enter(|scope| {
        let source = ModuleSource {
            source: b"import { n } from 'dep'; export const v = n + 1; export const file = import.meta.filename; export const main = import.meta.main; export const url = import.meta.url; export const dir = import.meta.dirname;".to_vec(),
            meta: Some(ModuleMeta { filename: String::from("/app/main.js"), main: true }),
            bytecode: false,
            source_map: None,
        };
        let (namespace, promise) = scope.module("/app/main.js", source).expect("conformance: module");
        while scope.run_job() {}
        check("module evaluation returns a promise", scope.kind(&promise) == Kind::Object);
        check("imports go through resolve and load", scope.as_number(&scope.get(&namespace, "v").expect("conformance: v")) == Some(42.0));
        check("import.meta.filename", text::<V>(&scope, &scope.get(&namespace, "file").expect("conformance: file")) == "/app/main.js");
        check("import.meta.url", text::<V>(&scope, &scope.get(&namespace, "url").expect("conformance: url")) == "file:///app/main.js");
        check("import.meta.dirname", text::<V>(&scope, &scope.get(&namespace, "dir").expect("conformance: dir")) == "/app");
        check("import.meta.main", scope.as_bool(&scope.get(&namespace, "main").expect("conformance: main")) == Some(true));
        let broken = ModuleSource { source: b"import 'nowhere';".to_vec(), meta: None, bytecode: false, source_map: None };
        check("an unresolvable import throws", scope.module("/app/broken.js", broken).is_err());
        scope.exception();
        if !V::capabilities().async_module_load {
            let late = ModuleSource { source: Vec::new(), meta: None, bytecode: false, source_map: None };
            check("complete_load throws without the capability", scope.complete_load(1, Ok(late)).is_err());
            scope.exception();
        }
    });
}


/// Bytecode: a module compiled to bytecode loads and links like its source, keeps
/// line positions for stack traces, and no longer carries its source text.
fn bytecode<V: Vm>() {
    if !V::capabilities().bytecode_cache {
        let (instance, _) = boot::<V>(&Limits::default());
        let context = instance.context().expect("conformance: context creation");
        context.enter(|scope| {
            let source = ModuleSource { source: b"export const a = 1;".to_vec(), meta: None, bytecode: false, source_map: None };
            check("compile throws without the capability", scope.compile("/app/a.js", source).is_err());
        });
        return;
    }
    let source_of = |code: &str| ModuleSource { source: code.as_bytes().to_vec(), meta: None, bytecode: false, source_map: None };
    let (compiler, _) = boot::<V>(&Limits::default());
    let context = compiler.context().expect("conformance: context creation");
    let image = context.enter(|scope| {
        let broken = scope.compile("/app/broken.js", source_of("export const = ;"));
        check("compile reports a syntax error", broken.is_err());
        // The contract leaves the exception pending until it is taken.
        let _ = scope.exception();
        scope
            .compile(
                "/app/bc.js",
                source_of("import { n } from 'dep';\nexport const w = n * 2;\nexport function f() {\n  throw new Error('x');\n}\nexport const shown = f.toString();\n"),
            )
            .expect("conformance: compile")
    });
    // A different instance: bytecode does not depend on the one that wrote it.
    let (instance, _) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    context.enter(|scope| {
        let meta = Some(ModuleMeta { filename: String::from("/app/bc.js"), main: false });
        let (namespace, _) = scope
            .module("/app/bc.js", ModuleSource { source: image.clone(), meta, bytecode: true, source_map: None })
            .expect("conformance: bytecode module");
        while scope.run_job() {}
        check("bytecode imports go through resolve and load", scope.as_number(&scope.get(&namespace, "w").expect("conformance: w")) == Some(82.0));
        let f = scope.get(&namespace, "f").expect("conformance: f");
        let failure = scope.call(&f, &[]);
        check("a bytecode function throws", failure.is_err());
        let error = scope.exception();
        let stack = text::<V>(&scope, &scope.get(&error, "stack").expect("conformance: stack"));
        check("bytecode keeps line positions", stack.contains("/app/bc.js:4"));
        let shown = text::<V>(&scope, &scope.get(&namespace, "shown").expect("conformance: shown"));
        check("bytecode carries no source text", !shown.contains("throw"));
    });
}
/// Unhandled rejections are reported, and reported again when handled later.
fn rejections<V: Vm>() {
    let (instance, seen) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    context.enter(|scope| {
        let promise = eval::<V>(&scope, "globalThis.p = Promise.reject(new Error('r')); p");
        while scope.run_job() {}
        let identity = scope.identity(&promise);
        check("an unhandled rejection is reported", *seen.rejections.borrow() == [(identity, false)]);
        eval::<V>(&scope, "p.catch(() => {})");
        while scope.run_job() {}
        check("a late handler is reported for the same promise", *seen.rejections.borrow() == [(identity, false), (identity, true)]);
    });
}

/// Host objects own a Rust value and finalize it when collected.
fn host_objects<V: Vm>() {
    let (instance, _) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    let finalized = Rc::new(Cell::new(false));
    let flag = finalized.clone();
    context.enter(|scope| {
        let object = scope.host_object((7u32, Finalized(flag))).expect("conformance: host_object");
        check("host data is readable", scope.host_data(&object, |data: &(u32, Finalized)| data.0) == Some(7));
        check("host data is typed", scope.host_data(&object, |_: &String| ()).is_none());
        let plain = scope.object().expect("conformance: object");
        check("a plain object has no host data", scope.host_data(&plain, |_: &(u32, Finalized)| ()).is_none());
    });
    instance.collect_garbage();
    check("the finalizer runs when the object is collected", finalized.get());
}

/// Memory limits make allocation fail instead of growing without bound.
fn limits<V: Vm>() {
    if !V::capabilities().memory_limits {
        return;
    }
    let (instance, _) = boot::<V>(&Limits { memory: Some(8 << 20), stack: None });
    let context = instance.context().expect("conformance: context creation");
    context.enter(|scope| {
        check("allocation past the memory limit throws", scope.eval("const all = []; for (;;) all.push(new Array(4096).fill(1));").is_err());
        scope.exception();
    });
    let heap = instance.heap();
    check("heap statistics report the limit", heap.limit == Some(8 << 20) && heap.used > 0);
}

/// Running JS is terminated by the interrupt handle and by the interrupt callback.
fn interrupts<V: Vm>() {
    if !V::capabilities().termination {
        return;
    }
    let (instance, seen) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    let interrupt = instance.interrupt();
    context.enter(|scope| {
        interrupt.request();
        check("the interrupt handle terminates a loop", scope.eval("for (;;) {}").is_err());
        scope.exception();
        check("the request is consumed", scope.as_number(&eval::<V>(&scope, "1 + 1")) == Some(2.0));
        seen.interrupt.set(true);
        check("the interrupt callback terminates a loop", scope.eval("for (;;) {}").is_err());
        scope.exception();
        seen.interrupt.set(false);
    });
}
