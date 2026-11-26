//! Node mismatches of the JS glue, fixed in the Rust port of the builtins.
//!
//! Each test runs a JS snippet on the real `Runtime` with QuickJS, the way
//! `node_surface.rs` does, and asserts what Node prints. Every test failed on
//! the JS glue before its fix; the fix site carries a comment naming the test.

use lunarstorm_runtime::{Access, Bindings, Host, Log, Runtime, Source, Stream};
use lunarstorm_vm_quickjs::QuickJs;
use std::cell::RefCell;

/// A host with no process behind it: output is captured, everything is allowed.
#[derive(Default)]
struct MemoryHost {
    /// Everything written to stdout.
    out: RefCell<Vec<u8>>,
    /// Log sink records (uncaught errors), formatted, so a failing test shows them.
    logs: RefCell<Vec<String>>,
}

impl Host for MemoryHost {
    /// No extensions.
    type Extensions<V: lunarstorm_runtime::vm::Vm> = ();

    /// # Returns
    /// The empty set.
    fn extensions<V: lunarstorm_runtime::vm::Vm>(&self) -> Self::Extensions<V> {}

    /// # Returns
    /// A fixed `process.argv`.
    fn args(&self) -> Vec<String> {
        vec![String::from("node_fixes")]
    }

    /// # Returns
    /// No variables.
    fn environment(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// # Returns
    /// The root, which exists everywhere.
    fn working_directory(&self) -> String {
        String::from("/")
    }

    /// Captures stdout; stderr is dropped.
    ///
    /// # Arguments
    /// * `stream` - which stream.
    /// * `bytes` - what was written.
    fn write(&self, stream: Stream, bytes: &[u8]) {
        if stream == Stream::Out {
            self.out.borrow_mut().extend_from_slice(bytes);
        }
    }

    /// Fails the test: nothing here is expected to exit.
    ///
    /// # Arguments
    /// * `code` - the exit status the Runtime asked for.
    fn exit(&self, code: i32) -> ! {
        panic!("the runtime asked the host to exit with {code}; logs: {:?}", self.logs.borrow());
    }

    /// # Arguments
    /// * `_access` - what a script is about to do.
    ///
    /// # Returns
    /// Always `true`.
    fn allow(&self, _access: Access<'_>) -> bool {
        true
    }

    /// Fills the buffer with a known byte.
    ///
    /// # Arguments
    /// * `into` - the buffer to fill.
    fn entropy(&self, into: &mut [u8]) {
        into.fill(7);
    }

    /// Records the log record.
    ///
    /// # Arguments
    /// * `record` - the record.
    fn log(&self, record: Log<'_>) {
        self.logs.borrow_mut().push(format!("{record:?}"));
    }
}

/// Runs source text as the entry module until the loop is idle.
///
/// # Arguments
/// * `code` - the entry module's source.
///
/// # Returns
/// What the script wrote to stdout.
fn out(code: &str) -> String {
    let mut runtime = Runtime::<QuickJs, MemoryHost>::new(MemoryHost::default()).expect("event loop");
    runtime.register(Bindings::new());
    runtime.start().expect("start");
    runtime.load(Source::Text { name: String::from("main.js"), code: code.to_owned() }).expect("load");
    assert_eq!(runtime.run(), 0, "exit status");
    let text = String::from_utf8(runtime.host().out.borrow().clone()).expect("stdout is UTF-8");
    text
}

/// a. Node: `path.extname("..")` is `""`; the glue gave `"."`.
#[test]
fn path_extname_of_dot_dot_is_empty() {
    let text = out(r#"
        import { extname } from "node:path";
        console.log(JSON.stringify([extname(".."), extname("a/.."), extname("..."), extname("a.")]));
    "#);
    assert_eq!(text, "[\"\",\"\",\".\",\".\"]\n");
}

/// b. Node: `path.normalize("./")` is `"./"`; the glue gave `"."`.
#[test]
fn path_normalize_keeps_trailing_slash_of_dot() {
    let text = out(r#"
        import { normalize } from "node:path";
        console.log(JSON.stringify([normalize("./"), normalize("a/../"), normalize("."), normalize("a/..")]));
    "#);
    assert_eq!(text, "[\"./\",\"./\",\".\",\".\"]\n");
}

/// c. Node: a lone surrogate encodes as U+FFFD (`[239,191,189]`); the glue threw.
#[test]
fn text_encoder_replaces_lone_surrogates() {
    let text = out(r#"
        try {
          console.log(new TextEncoder().encode("\ud800").join(), new TextEncoder().encode("a\udc00b").join());
        } catch (e) {
          console.log("threw", e.name, e.message);
        }
    "#);
    assert_eq!(text, "239,191,189 97,239,191,189,98\n");
}

/// d. Node keeps no enumerable listener storage visible to `Object.keys`; the glue listed `_ev`.
#[test]
fn emitter_keys_hide_listener_storage() {
    let text = out(r#"
        import EventEmitter from "node:events";
        const e = new EventEmitter();
        e.on("x", () => {});
        e.once("y", () => {});
        class Sub extends EventEmitter { constructor() { super(); this.own = 1; } }
        const s = new Sub();
        s.on("x", () => {});
        console.log(JSON.stringify(Object.keys(e)), JSON.stringify(Object.keys(s)));
    "#);
    assert_eq!(text, "[] [\"own\"]\n");
}

/// e. Node: `listeners()` returns the function given to `once`, not a wrapper.
#[test]
fn emitter_listeners_unwrap_once() {
    let text = out(r#"
        import EventEmitter from "node:events";
        const e = new EventEmitter();
        const f = () => {};
        const g = () => {};
        e.once("x", f);
        e.on("x", g);
        const [first, second] = e.listeners("x");
        console.log(first === f, second === g, e.listenerCount("x"));
        e.emit("x");
        console.log(e.listeners("x").length, e.listeners("x")[0] === g);
    "#);
    assert_eq!(text, "true true 2\n1 true\n");
}

/// f. Node: `res.statusCode = '404'` (a numeric string) sends status 404; the
/// glue sent 0, and the typed `ops.httpRespond` threw a `TypeError`.
///
/// The first probe answers on a key with no connection, so a throw is printed
/// instead of leaving a real exchange hanging; only when it does not throw does
/// the script make the real request and print the status line it got.
#[test]
fn http_numeric_string_status_code_is_sent() {
    let text = out(r#"
        import http from "node:http";
        import net from "node:net";
        let threw = false;
        try {
          const probe = new http.ServerResponse(1000000);
          probe.statusCode = "404";
          probe.end();
        } catch (e) {
          threw = true;
          console.log("threw", e.name, e.message);
        }
        const server = http.createServer((req, res) => { res.statusCode = "404"; res.end("x"); });
        if (!threw) server.listen(0, "127.0.0.1", () => {
          const client = net.connect(server.address().port, "127.0.0.1", () => client.write("GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"));
          let raw = "";
          client.on("data", (d) => { raw += d.toString(); });
          client.on("close", () => { console.log(raw.split("\r\n")[0]); server.close(); });
        });
    "#);
    assert_eq!(text, "HTTP/1.1 404 Not Found\n");
}
