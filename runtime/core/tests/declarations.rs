//! Declared bindings: `Runtime::declarations` lists them without a VM, and every
//! declared native checks its arguments, throwing
//! `TypeError: <fn>: <param> must be <type>` through the real Runtime and QuickJS.

use lunarstorm_runtime::vm::{Call, ClassDecl, FnDecl, Item, ModuleDecl, Param, Scope, Sig, Ty};
use lunarstorm_runtime::{declared, Access, Bindings, ClassImpl, Host, Log, Runtime, Source, Stream};
use lunarstorm_vm_quickjs::QuickJs;
use std::cell::RefCell;

/// A host with no process behind it: stdout is captured, everything is allowed.
#[derive(Default)]
struct MemoryHost {
    /// Everything written to stdout.
    out: RefCell<Vec<u8>>,
}

// Trait-impl members carry the trait's docs; each one here is the obvious stub.
impl Host for MemoryHost {
    type Extensions<V: lunarstorm_runtime::vm::Vm> = ();
    fn extensions<V: lunarstorm_runtime::vm::Vm>(&self) -> Self::Extensions<V> {}
    fn args(&self) -> Vec<String> {
        Vec::new()
    }
    fn environment(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn working_directory(&self) -> String {
        String::from("/")
    }
    fn write(&self, stream: Stream, bytes: &[u8]) {
        if stream == Stream::Out {
            self.out.borrow_mut().extend_from_slice(bytes);
        }
    }
    fn exit(&self, code: i32) -> ! {
        panic!("exit {code}; stdout: {}", String::from_utf8_lossy(&self.out.borrow()));
    }
    fn allow(&self, _access: Access<'_>) -> bool {
        true
    }
    fn entropy(&self, into: &mut [u8]) {
        into.fill(0);
    }
    fn log(&self, record: Log<'_>) {
        panic!("{record:?}");
    }
}

/// Runs `probe` as the host module `host:probe` (which may import `ops`) from an
/// entry module that only imports it.
///
/// # Arguments
/// * `bindings` - extra host bindings; `host:probe` is added to them.
/// * `probe` - the host module's source.
///
/// # Returns
/// What it wrote to stdout.
fn run(bindings: Bindings<QuickJs>, probe: &'static str) -> String {
    let mut runtime = Runtime::<QuickJs, MemoryHost>::new(MemoryHost::default()).expect("event loop");
    runtime.register(bindings.module("host:probe", probe));
    runtime.start().expect("start");
    runtime.load(Source::Text { name: String::from("main.js"), code: String::from("import 'host:probe';") }).expect("load");
    assert_eq!(runtime.run(), 0);
    let out = runtime.host().out.borrow().clone();
    String::from_utf8(out).expect("stdout is UTF-8")
}

/// Calls one op with arguments that do not fit and prints the error.
///
/// # Arguments
/// * `call` - the JS call expression, e.g. `ops.tick(1)`.
///
/// # Returns
/// The printed error, without the newline.
fn wrong(call: &str) -> String {
    let source = format!("import {{ ops }} from 'lunarstorm:internal';\ntry {{ {call}; console.log('no throw'); }} catch (e) {{ console.log(String(e)); }}\n");
    run(Bindings::new(), Box::leak(source.into_boxed_str())).trim_end().to_owned()
}

/// # Arguments
/// * `module` - a declared module.
///
/// # Returns
/// The names of its functions and of its other items, in order.
fn names(module: &ModuleDecl) -> (Vec<&str>, Vec<&str>) {
    let functions = module.items.iter().filter(|item| matches!(item, Item::Function(_))).map(Item::name).collect();
    let others = module.items.iter().filter(|item| !matches!(item, Item::Function(_))).map(Item::name).collect();
    (functions, others)
}

/// The Runtime's ops are listed as `lunarstorm:internal`, without starting a VM.
#[test]
fn declarations_list_every_op() {
    let runtime = Runtime::<QuickJs, MemoryHost>::new(MemoryHost::default()).expect("event loop");
    let modules = runtime.declarations().expect("declarations");
    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0].specifier, "lunarstorm:internal");
    let (functions, values) = names(&modules[0]);
    assert_eq!(
        functions,
        [
            "env", "cwd", "exit", "now", "print", "flush", "random", "settled", "tick", "immediate", "timerStart", "timerCancel",
            "utf8Decode", "utf8Encode", "listen", "listenerClose", "connect", "write", "end", "destroy", "httpRespond", "httpHead",
            "httpChunk", "httpEnd", "fs",
        ]
    );
    assert_eq!(values, ["argv", "pid", "version", "platform", "arch"]);
    assert!(modules[0].items.iter().all(|item| match item {
        Item::Function(function) => !function.sig.about.is_empty() && function.sig.params.iter().all(|param| !param.about.is_empty()),
        Item::Value(field) => !field.about.is_empty(),
        Item::Class(class) => !class.about.is_empty(),
    }));
}

/// Every function on the installed `ops` object is declared, and nothing declared
/// is missing from it.
#[test]
fn installed_ops_match_their_declarations() {
    let out = run(
        Bindings::new(),
        "import { ops } from 'lunarstorm:internal';\nconsole.log(Object.keys(ops).filter((k) => typeof ops[k] === 'function').sort().join());\n",
    );
    let runtime = Runtime::<QuickJs, MemoryHost>::new(MemoryHost::default()).expect("event loop");
    let modules = runtime.declarations().expect("declarations");
    let mut declared = names(&modules[0]).0;
    declared.sort_unstable();
    assert_eq!(out.trim_end(), declared.join(","));
}

/// Timers: a delay that is not a number.
#[test]
fn timers_reject_a_wrong_argument() {
    assert_eq!(wrong("ops.timerStart('soon', false)"), "TypeError: timerStart: ms must be number");
    assert_eq!(wrong("ops.timerCancel()"), "TypeError: timerCancel: id must be number");
}

/// fs: a path that is not a string, and a payload that is neither text nor bytes.
#[test]
fn fs_rejects_a_wrong_argument() {
    assert_eq!(wrong("ops.fs(1, 42, undefined, 0)"), "TypeError: fs: path must be string");
    assert_eq!(wrong("ops.fs(1, '/x', 7, 0)"), "TypeError: fs: data must be string | Uint8Array | null | undefined");
}

/// net: a payload that is neither text nor bytes, and a port that is not a number.
#[test]
fn net_rejects_a_wrong_argument() {
    assert_eq!(wrong("ops.write(0, {})"), "TypeError: write: data must be string | Uint8Array | null | undefined");
    assert_eq!(wrong("ops.connect('127.0.0.1', '80')"), "TypeError: connect: port must be number");
}

/// http: a close flag that is not a boolean.
#[test]
fn http_rejects_a_wrong_argument() {
    assert_eq!(wrong("ops.httpHead(0, 200, '', 'yes')"), "TypeError: httpHead: close must be boolean");
    assert_eq!(wrong("ops.httpRespond(0, '200', '', undefined, false)"), "TypeError: httpRespond: status must be number");
}

/// process and text: missing text, and bytes where a string belongs.
#[test]
fn process_and_text_reject_a_wrong_argument() {
    assert_eq!(wrong("ops.print(1)"), "TypeError: print: text must be string");
    assert_eq!(wrong("ops.utf8Decode('text')"), "TypeError: utf8Decode: bytes must be Uint8Array");
    assert_eq!(wrong("ops.settled(1, 'yes', 0)"), "TypeError: settled: ok must be boolean");
}

/// `greet(name)`, the native module's one function.
const GREET: FnDecl = FnDecl {
    name: "greet",
    sig: Sig { params: &[Param { name: "name", ty: Ty::String, about: "who" }], rest: None, returns: Ty::String, about: "Greets." },
};

/// A native class on `ops` with one method.
const COUNTER: ClassDecl = ClassDecl {
    name: "Counter",
    extends: None,
    constructor: Sig { params: &[Param { name: "start", ty: Ty::Number, about: "first value" }], rest: None, returns: Ty::Undefined, about: "" },
    methods: &[FnDecl {
        name: "add",
        sig: Sig { params: &[Param { name: "n", ty: Ty::Number, about: "amount" }], rest: None, returns: Ty::Number, about: "Adds." },
    }],
    accessors: &[],
    statics: &[],
    about: "Counts.",
};

/// A host's declared function, class and native module are installed, checked
/// and listed, with the host's `ops` members merged into `lunarstorm:internal`.
#[test]
fn host_declarations_are_installed_checked_and_listed() {
    let bindings = || {
        Bindings::<QuickJs>::new()
            .native_module(ModuleDecl { specifier: "host:greet", items: vec![Item::Function(GREET)], about: "Greetings." }, |scope, exports| {
                exports.push(declared(scope, GREET.name, GREET.sig, |call: &Call<'_, QuickJs>| {
                    let scope = call.scope();
                    let name = scope.with_str(&call.args()[0], str::to_owned)?;
                    scope.string(&format!("hi {name}"))
                })?);
                Ok(())
            })
            .class(
                COUNTER,
                ClassImpl {
                    constructor: Box::new(|call: &Call<'_, QuickJs>| Ok(std::cell::Cell::new(call.scope().as_number(&call.args()[0]).unwrap_or(0.0)))),
                    methods: vec![Box::new(|call: &Call<'_, QuickJs>| {
                        let scope = call.scope();
                        let n = scope.as_number(&call.args()[0]).unwrap_or(0.0);
                        let total = scope.host_data(call.this(), |count: &std::cell::Cell<f64>| {
                            count.set(count.get() + n);
                            count.get()
                        });
                        Ok(scope.number(total.unwrap_or(f64::NAN)))
                    })],
                    accessors: Vec::new(),
                    statics: Vec::new(),
                },
            )
    };
    let out = run(
        bindings(),
        "import { ops } from 'lunarstorm:internal';\nimport { greet } from 'host:greet';\n\
         const c = new ops.Counter(1);\nconsole.log(greet('you'), c.add(2), c.add(3));\n\
         for (const f of [() => greet(1), () => c.add('x'), () => new ops.Counter()]) { try { f(); } catch (e) { console.log(String(e)); } }\n",
    );
    assert_eq!(
        out,
        "hi you 3 6\nTypeError: greet: name must be string\nTypeError: add: n must be number\nTypeError: Counter: start must be number\n"
    );
    let mut runtime = Runtime::<QuickJs, MemoryHost>::new(MemoryHost::default()).expect("event loop");
    runtime.register(bindings());
    let modules = runtime.declarations().expect("declarations");
    let specifiers: Vec<_> = modules.iter().map(|module| module.specifier).collect();
    assert_eq!(specifiers, ["lunarstorm:internal", "host:greet"]);
    assert_eq!(modules[0].items.last().map(Item::name), Some("Counter"));
    assert_eq!(modules[1].items, [Item::Function(GREET)]);
}
