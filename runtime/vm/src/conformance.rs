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
    Callbacks, ClassSpec, Context, ErrorKind, Image, Instance, Kind, Limits, Load, ModuleMeta, ModuleSource, Scope, Vm,
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
    /// How many times `load` was asked for the native module.
    native_loads: Cell<u32>,
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
        } else if name == "native" {
            Ok(String::from("native:probe"))
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
    fn load<'s>(&self, scope: &V::Scope<'s>, name: &str) -> Result<Load, String> {
        if name == "/dep.js" {
            Ok(Load::Ready(ModuleSource { source: b"export const n = 41;".to_vec(), meta: None, image: None, source_map: None }))
        } else if name == "native:probe" {
            // A native module: `twice(x)` is a host closure, `answer` a plain value,
            // `evaluations` counts how often this callback built the module.
            self.0.native_loads.set(self.0.native_loads.get() + 1);
            let twice = scope
                .function("twice", |call| {
                    let value = call.args().first().and_then(|arg| call.scope().as_number(arg)).unwrap_or(0.0);
                    Ok(call.scope().number(value * 2.0))
                })
                .map_err(|_| String::from("function"))?;
            scope
                .native_module(name, &[("twice", twice), ("answer", scope.int(42))])
                .map_err(|_| String::from("native_module"))?;
            Ok(Load::Native)
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
        native_modules::<V>,
        bytecode::<V>,
        rejections::<V>,
        host_objects::<V>,
        class_subclass::<V>,
        class_extends_builtin::<V>,
        class_members::<V>,
        promises::<V>,
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
        // Negative zero must stay a float: an integer zero would make 1 / -0 positive
        // infinity on the JS side, so the sign is checked from both sides.
        let negative_zero = scope.number(-0.0);
        check("negative zero keeps its sign in Rust", scope.as_number(&negative_zero).is_some_and(f64::is_sign_negative));
        scope.set(&scope.global(), "negativeZero", negative_zero).expect("conformance: global");
        check("negative zero reaches JS as -0", scope.as_number(&eval::<V>(&scope, "1 / negativeZero")) == Some(f64::NEG_INFINITY));
        let string = scope.string("h\u{e9}llo \u{263e}").expect("conformance: string");
        check("string kind", scope.kind(&string) == Kind::String);
        check("string round trip", text::<V>(&scope, &string) == "h\u{e9}llo \u{263e}");
        check("with_str rejects a number", scope.with_str(&scope.int(1), |_| ()).is_err());
        let thrown = scope.exception();
        let name = scope.get(&thrown, "name").expect("conformance: error name");
        check("with_str throws a TypeError", text::<V>(&scope, &name) == "TypeError");
        check("exception is cleared once taken", scope.kind(&scope.exception()) == Kind::Undefined);
        // A JS string is UTF-16 and may hold an unpaired surrogate, which has no
        // UTF-8 encoding. `with_str` hands out `&str`, so it must refuse such a
        // string rather than lend bytes that are not UTF-8.
        let lone = scope.eval("'a\\uD800b'").expect("conformance: lone surrogate string");
        check("with_str rejects an unpaired surrogate", scope.with_str(&lone, |_| ()).is_err());
        let thrown = scope.exception();
        let name = scope.get(&thrown, "name").expect("conformance: error name");
        check("with_str throws a TypeError for an unpaired surrogate", text::<V>(&scope, &name) == "TypeError");
        let paired = scope.eval("'a\\uD83D\\uDE00b'").expect("conformance: surrogate pair string");
        check("with_str accepts a surrogate pair", text::<V>(&scope, &paired) == "a\u{1F600}b");
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
            image: None,
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
        let broken = ModuleSource { source: b"import 'nowhere';".to_vec(), meta: None, image: None, source_map: None };
        check("an unresolvable import throws", scope.module("/app/broken.js", broken).is_err());
        scope.exception();
        if !V::capabilities().async_module_load {
            let late = ModuleSource { source: Vec::new(), meta: None, image: None, source_map: None };
            check("complete_load throws without the capability", scope.complete_load(1, Ok(late)).is_err());
            scope.exception();
        }
    });
}

/// Native modules: exports are host-built values, evaluated once per instance and
/// shared by every importer.
fn native_modules<V: Vm>() {
    let (instance, seen) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    context.enter(|scope| {
        let first = ModuleSource {
            source: b"import { twice, answer } from 'native'; export const doubled = twice(answer); export { twice };".to_vec(),
            meta: None,
            image: None,
            source_map: None,
        };
        let (one, _) = scope.module("/app/one.js", first).unwrap_or_else(|_| {
            let thrown = scope.exception();
            let message = scope.get(&thrown, "message").ok().and_then(|m| scope.with_str(&m, str::to_owned).ok());
            panic!("conformance: native module import failed: {message:?}")
        });
        while scope.run_job() {}
        check("a native module's function is callable and its value importable", scope.as_number(&scope.get(&one, "doubled").expect("conformance: doubled")) == Some(84.0));
        let second = ModuleSource { source: b"export { twice } from 'native';".to_vec(), meta: None, image: None, source_map: None };
        let (two, _) = scope.module("/app/two.js", second).expect("conformance: second native import");
        while scope.run_job() {}
        let (a, b) = (scope.get(&one, "twice").expect("conformance: twice"), scope.get(&two, "twice").expect("conformance: twice"));
        check("every importer sees the same native export", scope.identity(&a) == scope.identity(&b));
        check("a native module is built once per instance", seen.native_loads.get() == 1);
    });
}


/// Bytecode: a module compiled to bytecode loads and links like its source, keeps
/// line positions for stack traces, and no longer carries its source text.
fn bytecode<V: Vm>() {
    if !V::capabilities().bytecode_cache {
        let (instance, _) = boot::<V>(&Limits::default());
        let context = instance.context().expect("conformance: context creation");
        context.enter(|scope| {
            let source = ModuleSource { source: b"export const a = 1;".to_vec(), meta: None, image: None, source_map: None };
            check("compile throws without the capability", scope.compile("/app/a.js", source).is_err());
        });
        return;
    }
    let source_of = |code: &str| ModuleSource { source: code.as_bytes().to_vec(), meta: None, image: None, source_map: None };
    let (compiler, _) = boot::<V>(&Limits::default());
    let context = compiler.context().expect("conformance: context creation");
    let image = context.enter(|scope| {
        let broken = scope.compile("/app/broken.js", source_of("export const = ;"));
        check("compile reports a syntax error", broken.is_err());
        // The contract leaves the exception pending until it is taken.
        let _ = scope.exception();
        Image::compile(
            &scope,
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
            .module("/app/bc.js", ModuleSource { source: Vec::new(), meta, image: Some(image.clone()), source_map: None })
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

/// Evaluates a script and returns its completion as text, panicking with the
/// thrown message when it throws.
///
/// # Arguments
/// * `scope` - the scope.
/// * `source` - the script; its completion value must be a string.
///
/// # Returns
/// The text.
fn eval_text<'s, V: Vm>(scope: &V::Scope<'s>, source: &str) -> String {
    match scope.eval(source) {
        Ok(value) => text::<V>(scope, &value),
        Err(_) => {
            let thrown = scope.exception();
            let message = scope.get(&thrown, "message").ok().and_then(|m| scope.with_str(&m, str::to_owned).ok());
            panic!("conformance: script threw {message:?}: {source}")
        }
    }
}

/// A native base class `Counter`: data is the first argument as a number,
/// `get()` reads it back through `this`.
///
/// # Arguments
/// * `scope` - the scope.
///
/// # Returns
/// The constructor.
fn counter_class<'s, V: Vm>(scope: &V::Scope<'s>) -> V::Value<'s> {
    let get = scope
        .function("get", |call| {
            let scope = call.scope();
            match scope.host_data(call.this(), |count: &f64| *count) {
                Some(count) => Ok(scope.number(count)),
                None => Err(scope.throw_error(ErrorKind::Type, "not a Counter")),
            }
        })
        .expect("conformance: function");
    let spec = ClassSpec::<V>::new("Counter").method("get", get);
    scope
        .class(spec, |call| Ok(call.args().first().and_then(|arg| call.scope().as_number(arg)).unwrap_or(0.0)))
        .expect("conformance: class")
}

/// Decisive for native classes: a JS class extends a native one, calls `super()`,
/// and reaches both its own and the native methods and the instance's host data.
fn class_subclass<V: Vm>() {
    let (instance, _) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    context.enter(|scope| {
        let counter = counter_class::<V>(&scope);
        scope.set(&scope.global(), "Counter", counter).expect("conformance: set");
        eval::<V>(&scope, "globalThis.Sub = class Sub extends Counter { constructor(n) { super(n + 1); this.own = 'own'; } twice() { return this.get() * 2; } }");
        let report = eval_text::<V>(
            &scope,
            "const s = new Sub(4); [s.twice(), s.get(), s.own, s instanceof Sub, s instanceof Counter, Object.getPrototypeOf(Sub.prototype) === Counter.prototype, new Counter(3).get(), Counter.name].join()",
        );
        check("a JS subclass of a native class works", report == "10,5,own,true,true,true,3,Counter");
        let sub = eval::<V>(&scope, "s");
        check("host data is attached to a subclass instance", scope.host_data(&sub, |count: &f64| *count) == Some(5.0));
        let foreign = eval_text::<V>(&scope, "try { Counter.prototype.get.call({}); 'no throw' } catch (e) { e.name }");
        check("a native method rejects a foreign this", foreign == "TypeError");
    });
}

/// Decisive for native classes: a native class extends `Uint8Array`; instances
/// are real byte arrays and carry the native prototype methods and host data.
fn class_extends_builtin<V: Vm>() {
    let (instance, _) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    context.enter(|scope| {
        let uint8 = eval::<V>(&scope, "Uint8Array");
        let sum = scope
            .function("sum", |call| {
                let scope = call.scope();
                let total = scope.with_bytes(call.this(), |bytes| bytes.iter().map(|b| f64::from(*b)).sum::<f64>())?;
                let tag = scope.host_data(call.this(), |tag: &&'static str| *tag).unwrap_or("none");
                let tagged = format!("{tag}:{total}");
                scope.string(&tagged)
            })
            .expect("conformance: function");
        let spec = ClassSpec::<V>::new("Bytes").extends(uint8).method("sum", sum);
        let bytes = scope.class(spec, |_| Ok("tag")).expect("conformance: class");
        scope.set(&scope.global(), "Bytes", bytes).expect("conformance: set");
        let report = eval_text::<V>(
            &scope,
            "const b = new Bytes(3); b[1] = 7; b[2] = 300; [b.length, b[1], b[2], b instanceof Uint8Array, b instanceof Bytes, ArrayBuffer.isView(b), b.sum(), Object.getPrototypeOf(Bytes.prototype) === Uint8Array.prototype, Object.getOwnPropertySymbols(b).length, Object.keys(b).join('')].join()",
        );
        check("a native class extending Uint8Array makes real byte arrays", report == "3,7,44,true,true,true,tag:51,true,0,012");
        let made = eval::<V>(&scope, "b");
        check("a byte-array instance is Bytes to the contract", scope.kind(&made) == Kind::Bytes);
        check("a byte-array instance carries host data", scope.host_data(&made, |tag: &&'static str| *tag) == Some("tag"));
        let from = eval_text::<V>(&scope, "const f = new Bytes([1, 2]); f.sum()");
        check("the base receives the constructor arguments", from == "tag:3");
        let deeper = eval_text::<V>(
            &scope,
            "class Buf extends Bytes { hex() { return Array.from(this, (x) => x.toString(16)).join(''); } } const d = new Buf([10, 11]); [d.hex(), d.sum(), d instanceof Uint8Array, d.subarray(1).length].join()",
        );
        check("a JS subclass of a native Uint8Array class works", deeper == "ab,tag:21,true,1");
    });
}

/// Native class members and call shape: `this` and `new.target`, accessors,
/// statics, the refusal of a call without `new`, and a native class extending
/// another one with both data kept.
fn class_members<V: Vm>() {
    let (instance, _) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    context.enter(|scope| {
        let receiver = scope
            .function("receiver", |call| {
                let scope = call.scope();
                let same = scope.identity(call.this()) == scope.identity(&scope.global());
                let plain = scope.kind(call.new_target()) == Kind::Undefined;
                Ok(scope.boolean(plain && !same && scope.kind(call.this()) == Kind::Object))
            })
            .expect("conformance: function");
        scope.set(&scope.global(), "receiver", receiver).expect("conformance: set");
        check("a native method sees this and no new.target", eval_text::<V>(&scope, "String({ receiver }.receiver())") == "true");
        let value = scope
            .function("value", |call| {
                let scope = call.scope();
                Ok(scope.number(scope.host_data(call.this(), |cell: &Cell<f64>| cell.get()).unwrap_or(f64::NAN)))
            })
            .expect("conformance: function");
        let assign = scope
            .function("value", |call| {
                let scope = call.scope();
                let next = call.args().first().and_then(|arg| scope.as_number(arg)).unwrap_or(0.0);
                scope.host_data(call.this(), |cell: &Cell<f64>| cell.set(next));
                Ok(scope.undefined())
            })
            .expect("conformance: function");
        let origin = scope.function("origin", |call| Ok(call.scope().int(7))).expect("conformance: function");
        let spec = ClassSpec::<V>::new("Point")
            .accessor("value", Some(value), Some(assign))
            .static_member("origin", origin)
            .static_member("dimensions", scope.int(2));
        let point = scope
            .class(spec, |call| {
                let scope = call.scope();
                // `this` is the new instance and `new.target` the constructor `new` was
                // applied to; both are recorded on the instance for the script to check.
                scope.set(call.this(), "target", call.new_target().clone())?;
                Ok(Cell::new(call.args().first().and_then(|arg| scope.as_number(arg)).unwrap_or(0.0)))
            })
            .expect("conformance: class");
        scope.set(&scope.global(), "Point", point).expect("conformance: set");
        let report = eval_text::<V>(
            &scope,
            "const p = new Point(3); const first = p.value; p.value = 9; class Q extends Point {} const q = new Q(1); [first, p.value, p.target === Point, q.target === Q, q.value, Point.origin(), Q.origin(), Point.dimensions, Object.keys(p).join('|'), Object.getOwnPropertyNames(Point.prototype).sort().join('|')].join()",
        );
        check("accessors, statics, this and new.target", report == "3,9,true,true,1,7,7,2,target,constructor|value");
        let plain = eval_text::<V>(&scope, "try { Point(1); 'no throw' } catch (e) { e.name }");
        check("calling a native class without new throws a TypeError", plain == "TypeError");
        let named = scope.class(ClassSpec::<V>::new("Named").extends(scope.int(1)), |_| Ok(()));
        check("extending a non-constructor throws", named.is_err());
        let thrown = scope.exception();
        check("extending a non-constructor throws a TypeError", text::<V>(&scope, &scope.get(&thrown, "name").expect("conformance: name")) == "TypeError");
        let base = scope.class(ClassSpec::<V>::new("Base"), |_| Ok(1u8)).expect("conformance: class");
        let derived = scope.class(ClassSpec::<V>::new("Derived").extends(base), |_| Ok(2u16)).expect("conformance: class");
        scope.set(&scope.global(), "Derived", derived).expect("conformance: set");
        let both = eval::<V>(&scope, "new Derived()");
        check("a native subclass of a native class keeps both data", scope.host_data(&both, |a: &u8| *a) == Some(1) && scope.host_data(&both, |b: &u16| *b) == Some(2));
    });
}

/// A host-made promise settles from Rust: fulfilled and rejected, observed by JS
/// reactions only after jobs run; a second settle is ignored.
fn promises<V: Vm>() {
    let (instance, seen) = boot::<V>(&Limits::default());
    let context = instance.context().expect("conformance: context creation");
    let (resolve, reject) = context.enter(|scope| {
        let (fulfilled, resolve) = scope.promise().expect("conformance: promise");
        let (rejected, reject) = scope.promise().expect("conformance: promise");
        scope.set(&scope.global(), "fulfilled", fulfilled).expect("conformance: set");
        scope.set(&scope.global(), "rejected", rejected).expect("conformance: set");
        eval::<V>(&scope, "globalThis.log = []; fulfilled.then((v) => log.push('ok:' + v)); rejected.catch((e) => log.push('err:' + e.message));");
        while scope.run_job() {}
        check("a host promise starts pending", eval_text::<V>(&scope, "log.join()").is_empty());
        (resolve, reject)
    });
    // Settled from a later scope through the persistent resolvers.
    context.enter(|scope| {
        scope.settle(&resolve, Ok(scope.int(5))).expect("conformance: settle");
        let error = scope.error(ErrorKind::Error, "no").expect("conformance: error");
        scope.settle(&reject, Err(error)).expect("conformance: settle");
        check("settling runs no reaction by itself", eval_text::<V>(&scope, "log.join()").is_empty());
        while scope.run_job() {}
        check("settled promises run their reactions as jobs", eval_text::<V>(&scope, "log.join()") == "ok:5,err:no");
        scope.settle(&resolve, Ok(scope.int(6))).expect("conformance: settle");
        while scope.run_job() {}
        check("a second settle is ignored", eval_text::<V>(&scope, "log.join()") == "ok:5,err:no");
        let stranger = scope.persist(&scope.object().expect("conformance: object"));
        check("settle refuses a handle that is not a resolver", scope.settle(&stranger, Ok(scope.undefined())).is_err());
        scope.exception();
    });
    check("a handled host rejection is not reported", seen.rejections.borrow().is_empty());
    drop((resolve, reject));
}

/// Memory limits make allocation fail instead of growing without bound.
fn limits<V: Vm>() {
    if !V::capabilities().memory_limits {
        return;
    }
    let (instance, _) = boot::<V>(&Limits { memory: Some(8 << 20), stack: None, gc_threshold: None });
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
