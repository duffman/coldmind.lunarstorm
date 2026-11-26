//! The script-visible Node surface of the Runtime, frozen.
//!
//! Every test runs a JS snippet on the real `Runtime` with QuickJS and asserts the
//! exact bytes it wrote to stdout (and the exit code where it matters). The
//! expectations were taken from the JS glue (`src/js/bootstrap.js`) as it is
//! today. This file is frozen: the Rust port of the glue must pass it unchanged.
//! A test that fails after the port means the port changed behaviour a script can
//! see; the port is fixed, not the test.

use lunarstorm_event_loop_runtime::{Access, Bindings, Host, Log, Runtime, Source, Stream};
use lunarstorm_engine_quickjs_adapter::QuickJs;
use std::cell::RefCell;

/// A host with no process behind it: output is captured, everything is allowed.
#[derive(Default)]
struct MemoryHost {
    /// Everything written to stdout.
    out: RefCell<Vec<u8>>,
    /// Everything written to stderr.
    err: RefCell<Vec<u8>>,
    /// Log sink records (uncaught errors), formatted, so a failing test shows them.
    logs: RefCell<Vec<String>>,
}

impl Host for MemoryHost {
    /// No extensions.
    type Extensions<V: lunarstorm_event_loop_runtime::vm::Vm> = ();

    /// # Returns
    /// The empty set.
    fn extensions<V: lunarstorm_event_loop_runtime::vm::Vm>(&self) -> Self::Extensions<V> {}

    /// # Returns
    /// A fixed `process.argv`.
    fn args(&self) -> Vec<String> {
        vec![String::from("node_surface"), String::from("main"), String::from("--flag")]
    }

    /// # Returns
    /// One variable, so `process.env` has a known content.
    fn environment(&self) -> Vec<(String, String)> {
        vec![(String::from("EMBEDDED"), String::from("yes"))]
    }

    /// # Returns
    /// The root, which exists everywhere.
    fn working_directory(&self) -> String {
        String::from("/")
    }

    /// Captures the bytes.
    ///
    /// # Arguments
    /// * `stream` - which buffer.
    /// * `bytes` - what was written.
    fn write(&self, stream: Stream, bytes: &[u8]) {
        let buffer = if stream == Stream::Out { &self.out } else { &self.err };
        buffer.borrow_mut().extend_from_slice(bytes);
    }

    /// Fails the test: nothing here is expected to exit.
    ///
    /// # Arguments
    /// * `code` - the exit status the Runtime asked for.
    fn exit(&self, code: i32) -> ! {
        // Child mode of `process_exit_asks_the_host_to_exit`: the host really exits,
        // after saving what the script wrote. Unwinding out of the VM is not an
        // option: QuickJS aborts when its runtime is freed with live objects.
        if let Ok(path) = std::env::var("NODE_SURFACE_EXIT_OUT") {
            std::fs::write(path, &*self.out.borrow()).expect("save stdout");
            std::process::exit(code);
        }
        panic!(
            "the runtime asked the host to exit with {code}; logs: {:?}; stdout so far: {:?}",
            self.logs.borrow(),
            String::from_utf8_lossy(&self.out.borrow())
        );
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

    /// Records the log record; an uncaught error then exits, which fails the test.
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
/// What the script wrote to stdout, what it wrote to stderr, and the exit code.
fn run(code: &str) -> (String, String, i32) {
    let mut runtime = Runtime::<QuickJs, MemoryHost>::new(MemoryHost::default()).expect("event loop");
    runtime.register(Bindings::new());
    runtime.start().expect("start");
    runtime.load(Source::Text { name: String::from("main.js"), code: code.to_owned() }).expect("load");
    let status = runtime.run();
    let host = runtime.host();
    let out = String::from_utf8(host.out.borrow().clone()).expect("stdout is UTF-8");
    let err = String::from_utf8(host.err.borrow().clone()).expect("stderr is UTF-8");
    (out, err, status)
}

/// Runs a script that must exit with status 0 and write nothing to stderr.
///
/// # Arguments
/// * `code` - the entry module's source.
///
/// # Returns
/// What the script wrote to stdout.
fn out(code: &str) -> String {
    let (out, err, status) = run(code);
    assert_eq!(err, "", "stderr");
    assert_eq!(status, 0, "exit status; stdout: {out:?}");
    out
}

/// Creates an empty directory for one test under the build's temporary directory.
///
/// # Arguments
/// * `name` - the test's name, which is the directory's name.
///
/// # Returns
/// The directory's path.
fn scratch(name: &str) -> String {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("node_surface").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the test directory");
    dir.to_str().expect("UTF-8 path").to_owned()
}

/// JS helper for the HTTP tests: sends raw bytes to a port, collects the reply
/// until it is complete (chunked terminator or `content-length` bytes), then
/// destroys the socket and calls `done(raw)`.
const RAW: &str = r#"
import net from "node:net";
import http from "node:http";
function complete(raw) {
  const i = raw.indexOf("\r\n\r\n");
  if (i < 0) return false;
  const head = raw.slice(0, i).toLowerCase();
  if (head.includes("transfer-encoding: chunked")) return raw.endsWith("0\r\n\r\n");
  const m = /content-length: (\d+)/.exec(head);
  return raw.length - (i + 4) >= (m ? Number(m[1]) : 0);
}
function rawRequest(port, request, done) {
  const sock = net.connect(port, "127.0.0.1");
  let raw = "";
  sock.on("data", (d) => {
    raw += d.toString();
    if (complete(raw)) sock.destroy();
  });
  sock.on("close", () => done(raw));
  sock.write(request);
}
function split(raw) {
  const i = raw.indexOf("\r\n\r\n");
  const lines = raw.slice(0, i).split("\r\n");
  const names = lines.slice(1).map((l) => l.split(":")[0].toLowerCase()).sort();
  return { status: lines[0], names: names.join(","), body: raw.slice(i + 4) };
}
"#;

// ------------------------------------------------------------------ ordering

/// Pins the phase order: sync, nextTick, promise jobs, then the first loop turn, where an immediate precedes a 0 ms timer.
#[test]
fn order_tick_before_microtask_before_immediate_before_zero_timer() {
    let text = out(r#"
        setImmediate(() => console.log("immediate"));
        setTimeout(() => console.log("timeout"), 0);
        Promise.resolve().then(() => console.log("promise"));
        process.nextTick(() => console.log("tick"));
        queueMicrotask(() => console.log("microtask"));
        console.log("sync");
    "#);
    assert_eq!(text, "sync\ntick\npromise\nmicrotask\nimmediate\ntimeout\n");
}

/// Pins nested scheduling: ticks queued from ticks and promises, promises queued from both.
#[test]
fn order_nested_ticks_and_microtasks() {
    let text = out(r#"
        process.nextTick(() => {
          console.log("tick1");
          Promise.resolve().then(() => console.log("promise-in-tick"));
          process.nextTick(() => console.log("tick-in-tick"));
        });
        Promise.resolve().then(() => {
          console.log("promise1");
          process.nextTick(() => console.log("tick-in-promise"));
          Promise.resolve().then(() => console.log("promise-in-promise"));
        });
        console.log("sync");
    "#);
    assert_eq!(text, "sync\ntick1\ntick-in-tick\npromise1\npromise-in-tick\npromise-in-promise\ntick-in-promise\n");
}

/// Pins immediates: promise jobs run between two immediates, and an immediate queued by an immediate waits for the next round.
#[test]
fn order_immediates_drain_microtasks_between_callbacks() {
    let text = out(r#"
        setImmediate(() => {
          console.log("i1");
          Promise.resolve().then(() => console.log("i1-promise"));
          process.nextTick(() => console.log("i1-tick"));
          setImmediate(() => console.log("i1-nested"));
        });
        setImmediate(() => console.log("i2"));
        setImmediate((a, b) => console.log("i3", a, b), "x", 2);
    "#);
    assert_eq!(text, "i1\ni1-tick\ni1-promise\ni2\ni3 x 2\ni1-nested\n");
}

/// Pins timer order by delay, then by creation, and work queued from a timer callback.
#[test]
fn order_timers_by_delay_then_creation_with_nested_work() {
    let text = out(r#"
        setTimeout(() => console.log("late"), 60);
        setTimeout(() => {
          console.log("first");
          setImmediate(() => console.log("first-immediate"));
          process.nextTick(() => console.log("first-tick"));
          Promise.resolve().then(() => console.log("first-promise"));
        }, 1);
        setTimeout(() => console.log("second-a"), 20);
        setTimeout(() => console.log("second-b"), 20);
    "#);
    assert_eq!(text, "first\nfirst-tick\nfirst-promise\nfirst-immediate\nsecond-a\nsecond-b\nlate\n");
}

/// Pins clearTimeout, clearImmediate, setInterval with arguments, and the callback type check.
#[test]
fn timers_clear_interval_and_arguments() {
    let text = out(r#"
        const t = setTimeout(() => console.log("never"), 1);
        clearTimeout(t);
        const im = setImmediate(() => console.log("never-immediate"));
        clearImmediate(im);
        clearTimeout(undefined); clearTimeout(12345); clearTimeout("x"); clearImmediate(undefined);
        let n = 0;
        const iv = setInterval((a, b) => {
          n++;
          console.log("tick", a, b, n);
          if (n === 3) { clearInterval(iv); setTimeout((v) => console.log("after", v), 0, "v"); }
        }, 1, "x", "y");
        for (const bad of [undefined, "str", 5]) {
          try { setTimeout(bad); } catch (e) { console.log(e.name, e.message); }
        }
        console.log(typeof t, typeof iv, typeof im);
    "#);
    assert_eq!(
        text,
        "TypeError The \"callback\" argument must be a function\n".repeat(3)
            + "number number object\ntick x y 1\ntick x y 2\ntick x y 3\nafter v\n"
    );
}

// -------------------------------------------------------------- EventEmitter

/// Pins on/once/off/emit return values, listener order, listenerCount, listeners and removeAllListeners.
#[test]
fn events_on_once_off_count_and_remove_all() {
    let text = out(r#"
        import EventEmitter from "node:events";
        const e = new EventEmitter();
        const calls = [];
        const a = (...x) => calls.push(["a", ...x]);
        console.log(e.emit("x"));
        console.log(e.on("x", a) === e, e.once("x", (...x) => calls.push(["once", ...x])) === e);
        console.log(e.listenerCount("x"), e.emit("x", 1, 2), e.emit("x", 3));
        console.log(JSON.stringify(calls));
        console.log(e.listenerCount("x"), e.listeners("x").length, e.listeners("x")[0] === a, e.listeners("none").length);
        e.addListener("y", a); e.on("y", a);
        console.log(e.listenerCount("y"));
        console.log(e.removeListener("y", a) === e, e.listenerCount("y"));
        e.off("y", a); e.off("y", a); e.off("never", a);
        console.log(e.listenerCount("y"));
        e.on("z", a); e.on("w", a);
        e.removeAllListeners("z");
        console.log(e.listenerCount("z"), e.listenerCount("w"));
        console.log(e.removeAllListeners() === e, e.listenerCount("w"));
    "#);
    assert_eq!(
        text,
        "false\ntrue true\n2 true true\n[[\"a\",1,2],[\"once\",1,2],[\"a\",3]]\n1 1 true 0\n2\ntrue 1\n0\n0 1\ntrue 0\n"
    );
}

/// Pins that a listener removed during emit still runs in that emit and one added during emit does not.
#[test]
fn events_remove_and_add_during_emit() {
    let text = out(r#"
        import EventEmitter from "node:events";
        const e = new EventEmitter();
        const seen = [];
        function b() { seen.push("b"); }
        function a() { seen.push("a"); e.off("x", b); e.on("x", () => seen.push("late")); }
        e.on("x", a);
        e.on("x", b);
        e.emit("x");
        console.log(seen.join(","), e.listenerCount("x"));
        seen.length = 0;
        e.emit("x");
        console.log(seen.join(","), e.listenerCount("x"));
    "#);
    assert_eq!(text, "a,b 2\na,late 3\n");
}

/// Pins once: it runs one time, receives the emit arguments and `this`, can be removed by its original function, and does not recurse.
#[test]
fn events_once_semantics() {
    let text = out(r#"
        import EventEmitter from "node:events";
        const e = new EventEmitter();
        let depth = 0;
        e.once("x", function (v) {
          depth++;
          console.log("once", v, this === e, e.listenerCount("x"));
          if (depth < 3) e.emit("x", "again");
        });
        console.log(e.emit("x", "first"), e.emit("x", "second"), depth);
        const f = () => console.log("never");
        e.once("y", f);
        e.off("y", f);
        console.log(e.emit("y"), e.listenerCount("y"));
        const twice = () => console.log("twice");
        e.on("z", twice); e.on("z", twice);
        e.emit("z");
        e.off("z", twice);
        console.log(e.listenerCount("z"));
    "#);
    assert_eq!(text, "once first true 0\ntrue false 1\nfalse 0\ntwice\ntwice\n1\n");
}

/// Pins 'error' with no listener: an Error is thrown as is, anything else becomes "Unhandled error: ...".
#[test]
fn events_error_without_listener_throws() {
    let text = out(r#"
        import EventEmitter from "node:events";
        const e = new EventEmitter();
        const original = new TypeError("bad");
        try { e.emit("error", original); } catch (x) { console.log("caught", x === original, x.name, x.message); }
        try { e.emit("error", "str"); } catch (x) { console.log("caught", x.name, x.message); }
        try { e.emit("error"); } catch (x) { console.log("caught", x.name, x.message); }
        console.log(e.emit("other", new Error("ignored")));
        e.on("error", (x) => console.log("handled", x.message));
        console.log(e.emit("error", new Error("ok")));
    "#);
    assert_eq!(
        text,
        "caught true TypeError bad\ncaught Error Unhandled error: str\ncaught Error Unhandled error: undefined\nfalse\nhandled ok\ntrue\n"
    );
}

/// Pins a user class extending EventEmitter: super(), own state, overridden emit and `this` in listeners.
#[test]
fn events_user_subclass_with_super() {
    let text = out(r#"
        import EventEmitter, { EventEmitter as Named } from "node:events";
        class Counter extends EventEmitter {
          constructor(start) { super(); this.n = start; }
          inc() { this.n++; this.emit("change", this.n); return this; }
          emit(type, ...args) { console.log("emit", type); return super.emit(type, ...args); }
        }
        const c = new Counter(5);
        c.on("change", function (n) { console.log("change", n, this === c); });
        c.inc().inc();
        console.log(c instanceof EventEmitter, c instanceof Counter, c.listenerCount("change"), c.n);
        console.log(EventEmitter === Named, Object.getPrototypeOf(Counter) === EventEmitter);
        class Plain extends Named {}
        const p = new Plain();
        p.once("x", (v) => console.log("plain", v));
        console.log(p.emit("x", 1), p.emit("x", 2));
    "#);
    assert_eq!(
        text,
        "emit change\nchange 6 true\nemit change\nchange 7 true\ntrue true 1 7\ntrue true\nplain 1\ntrue false\n"
    );
}

/// Pins that `process` is an EventEmitter.
#[test]
fn events_process_is_an_emitter() {
    let text = out(r#"
        import EventEmitter from "node:events";
        console.log(process instanceof EventEmitter);
        process.on("custom", (a, b) => console.log("custom", a, b));
        console.log(process.emit("custom", 1, 2), process.emit("nobody"));
    "#);
    assert_eq!(text, "true\ncustom 1 2\ntrue false\n");
}

// -------------------------------------------------------------------- Buffer

/// Pins Buffer.from for strings, arrays and typed arrays, toString, byteLength and the console form.
#[test]
fn buffer_from_string_array_and_to_string() {
    let text = out(r#"
        const b = Buffer.from("héllo €");
        console.log(b.length, Buffer.byteLength("héllo €"), Buffer.byteLength(b));
        console.log(b.toString(), b.toString("utf8"), b.toString("UTF-8"), b.toString("utf-8") === b.toString());
        console.log(Buffer.from("hi", "utf8").toString(), Buffer.from([104, 105]).toString());
        console.log(Array.from(Buffer.from([256, -1, 3.7, 258])).join());
        const src = new Uint8Array([1, 2]);
        const copy = Buffer.from(src);
        src[0] = 9;
        console.log(copy[0], copy instanceof Buffer);
        const ab = new Uint8Array([5, 6]).buffer;
        const view = Buffer.from(ab);
        new Uint8Array(ab)[0] = 7;
        console.log(view[0], view.length);
        console.log(Buffer.from([1, 2, 3]));
        console.log(Buffer.from("").length, Buffer.alloc(3).join(), Buffer.alloc(0).length);
        console.log(Array.from(Buffer.from([0xff, 0x41]).toString(), (c) => c.charCodeAt(0).toString(16)).join());
        for (const bad of ["hex", "base64", "latin1"]) {
          try { Buffer.from("x", bad); console.log("accepted", bad); } catch (e) { console.log(e.name, e.message); }
        }
    "#);
    assert_eq!(
        text,
        "10 10 10\nh\u{e9}llo \u{20ac} h\u{e9}llo \u{20ac} h\u{e9}llo \u{20ac} true\nhi hi\n0,255,3,2\n1 true\n7 2\nBuffer(3) [ 1, 2, 3 ]\n0 0,0,0 0\nfffd,41\n\
         TypeError Unsupported encoding: hex\nTypeError Unsupported encoding: base64\nTypeError Unsupported encoding: latin1\n"
    );
}

/// Pins Buffer as a Uint8Array subclass: instanceof, isBuffer, subarray sharing memory, slice copying (unlike Node), concat.
#[test]
fn buffer_extends_uint8array() {
    let text = out(r#"
        const b = Buffer.from("abcdef");
        console.log(b instanceof Buffer, b instanceof Uint8Array, Buffer.isBuffer(b), Buffer.isBuffer(new Uint8Array(1)), Buffer.isBuffer("x"));
        console.log(Object.getPrototypeOf(Buffer) === Uint8Array, Object.getPrototypeOf(b) === Buffer.prototype, b.constructor.name);
        const sub = b.subarray(1, 3);
        console.log(sub instanceof Buffer, sub.toString(), sub.byteOffset, sub.length);
        sub[0] = 66;
        console.log(b.toString());
        const sl = b.slice(2, 4);
        console.log(sl instanceof Buffer, sl.toString(), sl.byteOffset);
        sl[0] = 67;
        console.log(b.toString());
        const joined = Buffer.concat([Buffer.from("ab"), Buffer.from("cd"), new Uint8Array([101])]);
        console.log(joined instanceof Buffer, joined.toString(), joined.length);
        console.log(Buffer.concat([Buffer.from("abc"), Buffer.from("def")], 4).toString(), Buffer.concat([]).length);
        console.log(b.map((x) => x + 1) instanceof Buffer, [...b.subarray(0, 2)].join(), b.indexOf(100), b.at(-1));
    "#);
    assert_eq!(
        text,
        "true true true false false\ntrue true Buffer\ntrue bc 1 2\naBcdef\ntrue cd 0\naBcdef\n\
         true abcde 5\nabcd 0\ntrue 97,66 3 102\n"
    );
}

/// Pins the TextEncoder and TextDecoder round trip for ASCII, accented, symbol and astral text.
#[test]
fn text_encoder_decoder_round_trip() {
    let text = out(r#"
        const enc = new TextEncoder();
        const dec = new TextDecoder();
        console.log(enc.encoding, dec.encoding);
        for (const s of ["", "plain", "héllo", "€", "\u{1F600}", "日本語", "a\u0000b"]) {
          const bytes = enc.encode(s);
          console.log(JSON.stringify(s), bytes.length, dec.decode(bytes) === s, bytes instanceof Uint8Array, bytes instanceof Buffer);
        }
        console.log(enc.encode("é€\u{1F600}"));
        console.log(enc.encode().length, enc.encode(undefined).length, enc.encode(12).join(), enc.encode(null).length);
    "#);
    assert_eq!(
        text,
        "utf-8 utf-8\n\"\" 0 true true false\n\"plain\" 5 true true false\n\"h\u{e9}llo\" 6 true true false\n\"\u{20ac}\" 3 true true false\n\
         \"\u{1F600}\" 4 true true false\n\"\u{65e5}\u{672c}\u{8a9e}\" 9 true true false\n\"a\\u0000b\" 3 true true false\n\
         Uint8Array(9) [ 195, 169, 226, 130, 172, 240, 159, 152, 128 ]\n0 0 49,50 4\n"
    );
}

/// Pins TextDecoder inputs: ArrayBuffer, offset views, DataView, invalid bytes, missing input and unsupported labels.
#[test]
fn text_decoder_inputs_and_errors() {
    let text = out(r#"
        const bytes = new TextEncoder().encode("xxé€yy");
        const dec = new TextDecoder("utf-8");
        console.log(dec.decode(bytes.buffer) === "xxé€yy");
        console.log(dec.decode(bytes.subarray(2, 4)), dec.decode(bytes.subarray(4, 7)));
        console.log(dec.decode(new DataView(bytes.buffer, 2, 2)));
        console.log(dec.decode(Buffer.from("buf")), dec.decode(), dec.decode(undefined) === "");
        console.log(Array.from(dec.decode(new Uint8Array([0xc3, 0x28, 0xff])), (c) => c.charCodeAt(0).toString(16)).join());
        console.log(new TextDecoder("UTF8").encoding, new TextDecoder("Utf-8").encoding);
        for (const bad of ["latin1", "utf-16le"]) {
          try { new TextDecoder(bad); console.log("accepted", bad); } catch (e) { console.log(e.name, e.message); }
        }
        console.log(Buffer.from("é").toString() === dec.decode(Buffer.from("é")));
    "#);
    assert_eq!(
        text,
        "true\n\u{e9} \u{20ac}\n\u{e9}\nbuf  true\nfffd,28,fffd\nutf-8 utf-8\nRangeError Unsupported encoding: latin1\nRangeError Unsupported encoding: utf-16le\ntrue\n"
    );
}

// ---------------------------------------------------------------------- path

/// Pins path.join and path.normalize on dots, doubled slashes, trailing slashes, empty input and escapes above the root.
#[test]
fn path_join_and_normalize_edges() {
    let text = out(r#"
        import path, { join, normalize } from "node:path";
        const show = (...v) => console.log(JSON.stringify(v));
        show(join("a", "b", "../c"), join("/a/", "/b"), join(), join("", ""), join("a/"), join("..", "x"));
        show(join("/.."), join("a", "..", ".."), join("./a"), join("a//b"), join("a/b/"), join("/"), join("a", "", "b"));
        show(normalize("/a/./b/../c"), normalize("a/b/../"), normalize("a/.."), normalize("/"), normalize(""), normalize("//a//b//"));
        show(normalize("../../a"), normalize("/../a"), normalize("a/b/../../.."), normalize("./a/./b"));
        show(path.sep, path.delimiter, path.isAbsolute("/a"), path.isAbsolute("a"), path.isAbsolute(""), join === path.join);
    "#);
    assert_eq!(
        text,
        "[\"a/c\",\"/a/b\",\".\",\".\",\"a/\",\"../x\"]\n\
         [\"/\",\"..\",\"a\",\"a/b\",\"a/b/\",\"/\",\"a/b\"]\n\
         [\"/a/c\",\"a/\",\".\",\"/\",\".\",\"/a/b/\"]\n\
         [\"../../a\",\"/a\",\"..\",\"a/b\"]\n\
         [\"/\",\":\",true,false,false,true]\n"
    );
}

/// Pins path.resolve against the host's working directory "/" and with absolute and empty segments.
#[test]
fn path_resolve_edges() {
    let text = out(r#"
        import { resolve } from "node:path";
        console.log(JSON.stringify([
          resolve(), resolve(""), resolve("a", "b"), resolve("/a", "b", "../c"), resolve("/a", "/b"),
          resolve("a/"), resolve("/a/b/", ".."), resolve("/"), resolve("x", "", "y"), resolve("/a", "..", ".."), resolve("../a"),
        ]));
    "#);
    assert_eq!(text, "[\"/\",\"/\",\"/a/b\",\"/a/c\",\"/b\",\"/a\",\"/a\",\"/\",\"/x/y\",\"/\",\"/a\"]\n");
}

/// Pins path.basename, path.dirname and path.extname on roots, trailing slashes, dotfiles and suffix stripping.
#[test]
fn path_basename_dirname_extname_edges() {
    let text = out(r#"
        import { basename, dirname, extname } from "node:path";
        const show = (...v) => console.log(JSON.stringify(v));
        show(basename("/a/b.txt"), basename("/a/b.txt", ".txt"), basename(".txt", ".txt"), basename("/a/b/"), basename(""), basename("/"), basename("a"), basename("/a/b.txt", "b.txt"), basename("/a/b.txt", ".md"));
        show(dirname("/a/b"), dirname("/a"), dirname("a"), dirname("/"), dirname(""), dirname("a/b/"), dirname("//"), dirname("a/b/c"), dirname("/a/b//"));
        show(extname("a.b.c"), extname(".bashrc"), extname("a."), extname("a"), extname("/x.y/z"), extname("a.b/"), extname("/a/b.txt"), extname(".a.b"));
    "#);
    assert_eq!(
        text,
        "[\"b.txt\",\"b\",\".txt\",\"b\",\"\",\"\",\"a\",\"b.txt\",\"b.txt\"]\n\
         [\"/a\",\"/\",\".\",\"/\",\".\",\"a\",\"/\",\"a/b\",\"/a\"]\n\
         [\".c\",\"\",\".\",\"\",\"\",\".b\",\".txt\",\".b\"]\n"
    );
}

// ------------------------------------------------------------------- console

/// Pins the printf placeholders %s %d %i %f %j %o %O %% and what happens with too few or too many arguments.
#[test]
fn console_format_placeholders() {
    let text = out(r#"
        console.log("%s is %d years", "Bob", 42);
        console.log("%s", { a: 1 }, "extra");
        console.log("%s|%s|%s", null, undefined, [1, "a"]);
        console.log("%d|%d|%d|%d", "42abc", "3.5", 10n, true);
        console.log("%i|%f|%i", "3.9px", "3.5e1x", "z");
        console.log("%j|%j", { a: [1, 2] }, "s");
        console.log("%o|%O", { a: { b: [1] } }, "text");
        console.log("100%", 5);
        console.log("%% %s", "a");
        console.log("%s");
        console.log("%s %s", "only");
        console.log("%x", 1);
        console.log("a", "b", 3, { c: "d" });
        console.log(5, "%s", "z");
        console.log();
        console.log("");
    "#);
    assert_eq!(
        text,
        "Bob is 42 years\n{ a: 1 } extra\nnull|undefined|[ 1, 'a' ]\nNaN|3.5|10n|1\n3|35|NaN\n{\"a\":[1,2]}|\"s\"\n\
         { a: { b: [ 1 ] } }|text\n100% 5\n% a\n%s\nonly %s\n%x 1\na b 3 { c: 'd' }\n5 %s z\n\n\n"
    );
}

/// Pins how objects, arrays, maps, sets, classes and typed arrays print, including the depth limit and quoting.
#[test]
fn console_inspect_nested_values() {
    let text = out(r#"
        console.log({ a: 1, b: "two", c: [1, 2, { d: 3 }], e: { f: { g: { h: 1 } } } });
        console.log([], {}, [[]], [{}], { "a-b": 1, ok_1: 2, 3: 4 });
        console.log(["a", "b"], "top", ["top"]);
        console.log([[[[1]]]], [[[{ a: 1 }]]]);
        console.log(new Map([["a", 1], [{ k: 1 }, [2]]]), new Set([1, "s"]), new Map(), new Set());
        class Foo { constructor() { this.x = 1; } }
        console.log(new Foo(), [new Foo()], Object.create(null), Object.assign(Object.create(null), { k: 1 }));
        console.log(new Uint8Array([1, 2, 3]), new Float64Array(0), Array.from({ length: 101 }, (_, i) => i).slice(98));
        console.log(Array.from({ length: 102 }, (_, i) => i));
        function named() {}
        console.log(named, () => 1, [named], Symbol("s"), 12n, new Date(0), /re+/gi, new Date(NaN));
    "#);
    assert_eq!(
        text,
        "{ a: 1, b: 'two', c: [ 1, 2, { d: 3 } ], e: { f: { g: [Object] } } }\n\
         [] {} [ [] ] [ {} ] { '3': 4, 'a-b': 1, ok_1: 2 }\n\
         [ 'a', 'b' ] top [ 'top' ]\n\
         [ [ [ [Array] ] ] ] [ [ [ [Object] ] ] ]\n\
         Map(2) { 'a' => 1, { k: 1 } => [ 2 ] } Set(2) { 1, 's' } Map(0) {} Set(0) {}\n\
         Foo { x: 1 } [ Foo { x: 1 } ] [Object: null prototype] {} [Object: null prototype] { k: 1 }\n\
         Uint8Array(3) [ 1, 2, 3 ] Float64Array(0) [] [ 98, 99, 100 ]\n\
         [ 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, ... 2 more items ]\n\
         [Function: named] [Function (anonymous)] [ [Function: named] ] Symbol(s) 12n 1970-01-01T00:00:00.000Z /re+/gi Invalid Date\n"
    );
}

/// Pins cycle detection ([Circular]) and that a repeated, non-cyclic reference prints in full.
#[test]
fn console_cycles_and_shared_references() {
    let text = out(r#"
        const o = { name: "o" }; o.self = o;
        console.log(o);
        const a = [1]; a.push(a);
        console.log(a);
        const x = {}; const y = { x }; x.y = y;
        console.log(x);
        const shared = { v: 1 };
        console.log({ p: shared, q: shared }, [shared, shared]);
        const m = new Map(); m.set("me", m);
        console.log(m);
        console.log("%s", o);
    "#);
    assert_eq!(
        text,
        "{ name: 'o', self: [Circular] }\n[ 1, [Circular] ]\n{ y: { x: [Circular] } }\n\
         { p: { v: 1 }, q: { v: 1 } } [ { v: 1 }, { v: 1 } ]\nMap(1) { 'me' => [Circular] }\n{ name: 'o', self: [Circular] }\n"
    );
}

/// Pins how null, undefined, numbers, booleans, strings and symbols print alone, nested and as extra arguments.
#[test]
fn console_primitives_null_and_undefined() {
    let text = out(r#"
        console.log(undefined, null, true, false, 0, -1.5, NaN, Infinity, 1e21, 0.1 + 0.2);
        console.log([undefined, null, "s", 1, true], { u: undefined, n: null, s: "q'uote" });
        console.log("");
        console.log(" lead", "trail ");
        console.log("multi\nline", ["multi\nline"]);
        console.log([1, , 3].length, [undefined, undefined]);
        console.log("a\tb");
    "#);
    assert_eq!(
        text,
        "undefined null true false 0 -1.5 NaN Infinity 1e+21 0.30000000000000004\n\
         [ undefined, null, 's', 1, true ] { u: undefined, n: null, s: 'q'uote' }\n\n lead trail \n\
         multi\nline [ 'multi\nline' ]\n3 [ undefined, undefined ]\na\tb\n"
    );
}

/// Pins how errors print: name and message when there is no stack, subclasses, nested errors, and the first line of a real stack.
#[test]
fn console_errors() {
    let text = out(r#"
        const e = new Error("boom"); e.stack = "";
        const t = new TypeError("bad"); Object.defineProperty(t, "stack", { value: undefined });
        class MyErr extends Error { constructor(m) { super(m); this.name = "MyErr"; } }
        const m = new MyErr("x"); m.stack = "";
        const n = new Error(""); n.stack = "";
        console.log(e);
        console.log(t, m, n);
        console.log("msg:", e, "%s");
        console.log("%s|%o", e, t);
        console.log({ err: e }, [t]);
        const withStack = new Error("real");
        console.log(withStack);
    "#);
    let mut lines = text.lines();
    assert_eq!(
        (lines.next(), lines.next(), lines.next(), lines.next(), lines.next(), lines.next()),
        (
            Some("Error: boom"),
            Some("TypeError: bad MyErr: x Error"),
            Some("msg: Error: boom %s"),
            Some("Error: boom|TypeError: bad"),
            Some("{ err: Error: boom } [ TypeError: bad ]"),
            Some("Error: real"),
        )
    );
    assert!(lines.next().is_some_and(|frame| frame.trim_start().starts_with("at ")), "a stack frame follows: {text:?}");
}

/// Pins stream routing: log, info and debug go to stdout; error and warn go to stderr; trace writes "Trace:" to stderr.
#[test]
fn console_streams_and_process_stdout() {
    let (out, err, status) = run(r#"
        console.log("log"); console.info("info"); console.debug("debug");
        console.error("error", { a: 1 }); console.warn("warn %s", "x");
        console.trace("here");
        console.log(process.stdout.write("raw"), process.stderr.write(5));
        process.stdout.write("\n");
    "#);
    assert_eq!(status, 0);
    assert_eq!(out, "log\ninfo\ndebug\nrawtrue true\n\n");
    assert!(err.starts_with("error { a: 1 }\nwarn x\nTrace: here\n"), "stderr: {err:?}");
    assert!(err.ends_with('5'), "stderr: {err:?}");
}

// ------------------------------------------------------------------- process

/// Pins process.argv, process.env, process.cwd and the types of the descriptive fields.
#[test]
fn process_argv_env_cwd_and_types() {
    let text = out(r#"
        import proc from "node:process";
        console.log(proc === process, proc === globalThis.process, global === globalThis);
        console.log(process.argv.join("|"), Array.isArray(process.argv));
        console.log(process.env.EMBEDDED, typeof process.env.NOPE, Object.keys(process.env).join(), process.env === process.env);
        process.env.ADDED = "1";
        console.log(process.env.ADDED, Object.keys(process.env).join());
        console.log(process.cwd());
        console.log(typeof process.pid, typeof process.platform, typeof process.arch, typeof process.version);
        console.log(process.version.startsWith("v"), process.version === "v" + process.versions.lunarstorm, typeof process.versions.lunarstorm);
        console.log(typeof process.uptime(), typeof process.hrtime.bigint(), typeof performance.now(), process.hrtime.bigint() > 0n);
        console.log(typeof process.nextTick, typeof process.exit, process.exitCode);
        console.log(crypto.getRandomValues(new Uint8Array(3)).join());
    "#);
    assert_eq!(
        text,
        "true true true\nnode_surface|main|--flag true\nyes undefined EMBEDDED true\n1 EMBEDDED,ADDED\n/\n\
         number string string string\ntrue true string\nnumber bigint number true\nfunction function undefined\n7,7,7\n"
    );
}

/// Pins process.exitCode as the exit status: numbers, numeric strings and fractions, set synchronously or from a timer.
#[test]
fn process_exit_code_becomes_status() {
    for (set, printed, expected) in [("7", "7", 7), ("\"3\"", "3", 3), ("2.9", "2.9", 2), ("0", "0", 0), ("undefined", "undefined", 0), ("-1", "-1", -1)] {
        let (text, err, status) = run(&format!("process.exitCode = {set}; console.log(process.exitCode);"));
        assert_eq!((text.as_str(), err.as_str(), status), (&*format!("{printed}\n"), "", expected), "exitCode = {set}");
    }
    let (text, _, status) = run(r#"setTimeout(() => { console.log("late"); process.exitCode = 9; }, 1); process.nextTick(() => { process.exitCode = 4; });"#);
    assert_eq!((text.as_str(), status), ("late\n", 9));
    let (text, _, status) = run("console.log('quiet');");
    assert_eq!((text.as_str(), status), ("quiet\n", 0));
}

/// Pins the globals and builtin modules a script sees: Buffer, timers and process are the same objects through node: imports.
#[test]
fn module_surface_matches_globals() {
    let text = out(r#"
        import timers, { setTimeout as st, setImmediate as si, clearTimeout as ct } from "node:timers";
        import { Buffer as B } from "node:buffer";
        import { readFileSync, promises } from "node:fs";
        import fsp from "node:fs/promises";
        import fs from "node:fs";
        console.log(timers.setTimeout === setTimeout, st === setTimeout, si === setImmediate, ct === clearTimeout, timers.clearInterval === clearInterval);
        console.log(B === Buffer, readFileSync === fs.readFileSync, promises === fs.promises, fsp === promises);
        console.log(typeof setInterval, typeof clearImmediate, typeof queueMicrotask, typeof TextEncoder, typeof TextDecoder, typeof console.log);
        console.log(typeof require, typeof module, typeof __dirname);
    "#);
    assert_eq!(text, "true true true true true\ntrue true true true\nfunction function function function function function\nundefined undefined undefined\n");
}

// ------------------------------------------------------------------------ fs

/// Pins the synchronous fs API: write, append, read as Buffer and as text, stat, mkdir -p, readdir, rename, unlink, rmdir, rm.
#[test]
fn fs_sync_round_trip() {
    let dir = scratch("fs_sync_round_trip");
    let text = out(&r#"
        import fs from "node:fs";
        const d = "@DIR@";
        console.log(fs.writeFileSync(d + "/a.txt", "héllo"));
        console.log(fs.existsSync(d + "/a.txt"), fs.existsSync(d + "/nope"), fs.existsSync(d));
        const buf = fs.readFileSync(d + "/a.txt");
        console.log(buf instanceof Buffer, buf.length, buf.toString());
        console.log(fs.readFileSync(d + "/a.txt", "utf8"), fs.readFileSync(d + "/a.txt", { encoding: "utf-8" }));
        fs.appendFileSync(d + "/a.txt", "!");
        const st = fs.statSync(d + "/a.txt");
        console.log(st.isFile(), st.isDirectory(), st.size, st.mtime instanceof Date, Math.floor(st.mtimeMs) === st.mtime.getTime(), st instanceof fs.Stats);
        console.log(fs.statSync(d).isDirectory(), fs.statSync(d).isFile());
        console.log(fs.mkdirSync(d + "/sub/deep", { recursive: true }));
        fs.writeFileSync(d + "/sub/b.bin", new Uint8Array([0, 1, 2, 255]));
        console.log(fs.readdirSync(d).sort().join(), fs.readdirSync(d + "/sub").sort().join());
        console.log(Array.from(fs.readFileSync(d + "/sub/b.bin")).join());
        fs.writeFileSync(d + "/a.txt", "over");
        console.log(fs.readFileSync(d + "/a.txt", "utf8"));
        fs.renameSync(d + "/a.txt", d + "/c.txt");
        console.log(fs.existsSync(d + "/a.txt"), fs.existsSync(d + "/c.txt"));
        fs.unlinkSync(d + "/c.txt");
        fs.rmdirSync(d + "/sub/deep");
        fs.rmSync(d + "/sub", { recursive: true });
        console.log(fs.readdirSync(d).length);
    "#.replace("@DIR@", &dir));
    assert_eq!(
        text,
        "undefined\ntrue false true\ntrue 6 h\u{e9}llo\nh\u{e9}llo h\u{e9}llo\ntrue false 7 true true true\ntrue false\nundefined\n\
         a.txt,sub b.bin,deep\n0,1,2,255\nover\nfalse true\n0\n"
    );
}

/// Pins fs errors: the `code` of ENOENT and EEXIST on every flavour, and the callback type check.
#[test]
fn fs_errors_carry_node_codes() {
    let dir = scratch("fs_errors_carry_node_codes");
    let text = out(&r#"
        import fs from "node:fs";
        import fsp from "node:fs/promises";
        const d = "@DIR@";
        const probe = (label, fn) => { try { fn(); console.log(label, "no error"); } catch (e) { console.log(label, e instanceof Error, e.code, typeof e.message, e.message.length > 0); } };
        probe("read", () => fs.readFileSync(d + "/missing"));
        probe("stat", () => fs.statSync(d + "/missing"));
        probe("readdir", () => fs.readdirSync(d + "/missing"));
        probe("unlink", () => fs.unlinkSync(d + "/missing"));
        probe("rename", () => fs.renameSync(d + "/missing", d + "/other"));
        probe("write", () => fs.writeFileSync(d + "/missing/child.txt", "x"));
        fs.mkdirSync(d + "/exists");
        probe("mkdir", () => fs.mkdirSync(d + "/exists"));
        probe("mkdir -p", () => fs.mkdirSync(d + "/exists", { recursive: true }));
        probe("utf8", () => fs.readFileSync(d + "/exists", "latin1"));
        probe("callback", () => fs.readFile(d + "/exists"));
        // Sequential: two requests in flight settle in an unspecified order.
        fsp.readFile(d + "/missing").catch((e) => console.log("promise", e.code))
          .then(() => fs.stat(d + "/missing", (err, st) => console.log("callback", err.code, st)));
    "#.replace("@DIR@", &dir));
    assert_eq!(
        text,
        "read true ENOENT string true\nstat true ENOENT string true\nreaddir true ENOENT string true\nunlink true ENOENT string true\n\
         rename true ENOENT string true\nwrite true ENOENT string true\nmkdir true EEXIST string true\nmkdir -p no error\n\
         utf8 true undefined string true\ncallback true undefined string true\npromise ENOENT\ncallback ENOENT undefined\n"
    );
}

/// Pins the promise and callback flavours of fs, and that fs/promises is fs.promises.
#[test]
fn fs_promise_and_callback_forms() {
    let dir = scratch("fs_promise_and_callback_forms");
    let text = out(&r#"
        import fs from "node:fs";
        import fsp from "node:fs/promises";
        const d = "@DIR@";
        console.log(fsp === fs.promises);
        const call = (name, ...args) => new Promise((resolve) => fs[name](...args, (err, value) => resolve([err, value])));
        async function main() {
          console.log(await fsp.writeFile(d + "/p.txt", "one"));
          await fsp.appendFile(d + "/p.txt", "+two");
          const buf = await fsp.readFile(d + "/p.txt");
          console.log(buf instanceof Buffer, buf.toString(), await fsp.readFile(d + "/p.txt", "utf8"));
          const st = await fsp.stat(d + "/p.txt");
          console.log(st.isFile(), st.size, st instanceof fs.Stats, (await fsp.stat(d)).isDirectory());
          await fsp.mkdir(d + "/x/y", { recursive: true });
          await fsp.rename(d + "/p.txt", d + "/x/q.txt");
          console.log((await fsp.readdir(d)).join(), (await fsp.readdir(d + "/x")).sort().join());
          await fsp.unlink(d + "/x/q.txt");
          await fsp.rmdir(d + "/x/y");
          await fsp.rm(d + "/x", { recursive: true });
          console.log((await fsp.readdir(d)).length);
          try { await fsp.readFile(d + "/gone"); } catch (e) { console.log("rejected", e.code); }
          const [e1, v1] = await call("writeFile", d + "/cb.txt", "callback");
          console.log(e1, v1);
          const [e2, v2] = await call("readFile", d + "/cb.txt", "utf8");
          console.log(e2, v2);
          const [e3, v3] = await call("readFile", d + "/cb.txt");
          console.log(e3, v3 instanceof Buffer, v3.length);
          const [e4, v4] = await call("readdir", d);
          console.log(e4, v4.join());
          const [e5, v5] = await call("readFile", d + "/gone", "utf8");
          console.log(e5.code, v5);
          const [e6] = await call("unlink", d + "/cb.txt");
          console.log(e6, fs.existsSync(d + "/cb.txt"));
        }
        main();
    "#.replace("@DIR@", &dir));
    assert_eq!(
        text,
        "true\nundefined\ntrue one+two one+two\ntrue 7 true true\nx q.txt,y\n\
         0\nrejected ENOENT\nnull undefined\nnull callback\nnull true 8\nnull cb.txt\nENOENT undefined\nnull false\n"
    );
}

// ----------------------------------------------------------------------- net

/// Pins a TCP echo on 127.0.0.1 port 0: listening, connect, data as Buffer, end, close and server.close in one script that ends (the racy tail is sorted).
#[test]
fn net_tcp_echo_closes_cleanly() {
    let text = out(r#"
        import net from "node:net";
        const closes = [];
        // Which of "connection" and "connected" comes first is up to the kernel.
        const first = [];
        const early = (line) => { first.push(line); if (first.length === 2) console.log(first.sort().join("\n")); };
        const closed = (name) => { closes.push(name); if (closes.length === 4) console.log(closes.sort().join("\n")); };
        const server = net.createServer((sock) => {
          early("connection " + sock.remoteAddress + " " + typeof sock.remotePort + " " + (sock.server === server));
          sock.on("data", (d) => { console.log("server got", d.toString(), d instanceof Buffer); sock.write(d.toString().toUpperCase()); });
          sock.on("end", () => console.log("server end"));
          sock.on("close", (hadError) => { closed("server-socket close " + hadError); server.close(); });
        });
        server.on("close", () => closed("server closed"));
        console.log(server.listening, server.address());
        server.listen(0, "127.0.0.1", () => {
          const info = server.address();
          console.log("listening", info.address, info.family, info.port > 0, server.listening);
          const client = net.connect(info.port, "127.0.0.1", () => { early("client connected " + client.connecting); client.write("ping"); });
          console.log(client.connecting, client.remoteAddress, client.remotePort === info.port);
          client.on("data", (d) => { console.log("client got", d.toString()); client.end(); });
          client.on("end", () => closed("client end"));
          client.on("close", (hadError) => closed("client close " + hadError));
        });
    "#);
    assert_eq!(
        text,
        "false null\nlistening 127.0.0.1 IPv4 true true\ntrue 127.0.0.1 true\nclient connected false\nconnection 127.0.0.1 number true\n\
         server got ping true\nclient got PING\nserver end\n\
         client close false\nclient end\nserver closed\nserver-socket close false\n"
    );
}

/// Pins that writes made before the connect completes are queued and replayed in order, with the final end().
#[test]
fn net_writes_before_connect_are_queued() {
    let text = out(r#"
        import net from "node:net";
        const server = net.createServer((sock) => {
          const parts = [];
          sock.on("data", (d) => parts.push(d.toString()));
          sock.on("end", () => { console.log("server received", parts.join("")); server.close(); });
        });
        server.listen(0, "127.0.0.1", () => {
          const client = net.connect({ port: server.address().port, host: "127.0.0.1" });
          console.log(client.write("ab"), client.write(new Uint8Array([99])), client.end("d") === client);
          client.on("close", () => console.log("client closed"));
        });
    "#);
    assert_eq!(text, "true true true\nserver received abcd\nclient closed\n");
}

/// Pins net error behaviour: refused connect, address in use, port range, listen twice, close when not running, write after end.
#[test]
fn net_errors_and_argument_checks() {
    let text = out(r#"
        import net from "node:net";
        const first = net.createServer();
        first.listen(0, "127.0.0.1", () => {
          const port = first.address().port;
          const second = net.createServer();
          second.on("error", (e) => {
            console.log("in use", e.code);
            first.close(() => {
              const refused = net.connect(port, "127.0.0.1");
              refused.on("error", (e) => console.log("refused", /^ECONN(REFUSED|RESET)$/.test(e.code), e instanceof Error));
              refused.on("close", (hadError) => console.log("refused close", hadError));
            });
          });
          second.listen(port, "127.0.0.1");
          try { first.listen(0); } catch (e) { console.log(e.code); }
        });
        for (const bad of [-1, 65536, 1.5, "x"]) {
          try { net.createServer().listen(bad); } catch (e) { console.log(e.name, e.message); }
        }
        const idle = net.createServer();
        idle.close((err) => console.log("close idle", err.code));
        const s = new net.Socket();
        s.destroy();
        s.on("close", (had) => console.log("destroyed close", had, s.destroyed));
        s.write("x");
        s.on("error", (e) => console.log("write error", e.code));
    "#);
    assert_eq!(
        text,
        "RangeError Port should be >= 0 and < 65536\n".repeat(4)
            + "ERR_SERVER_ALREADY_LISTEN\nclose idle ERR_SERVER_NOT_RUNNING\ndestroyed close false true\nwrite error ERR_STREAM_WRITE_AFTER_END\nin use EADDRINUSE\nrefused true true\nrefused close true\n"
    );
}

// ---------------------------------------------------------------------- http

/// Pins a chunked response: write, write, end(chunk), with headers set through setHeader and an array-valued header.
#[test]
fn http_chunked_response() {
    let text = out(&format!(r#"{RAW}
        const server = http.createServer((req, res) => {{
          console.log("request", req.method, req.url, req.headers.host, req.httpVersion, req.complete);
          res.setHeader("Content-Type", "text/plain");
          res.setHeader("X-Multi", ["a", "b"]);
          console.log(res.getHeader("content-type"), res.hasHeader("X-MULTI"), res.hasHeader("none"), res.headersSent);
          console.log(res.write("hello "), res.headersSent);
          console.log(res.write(Buffer.from("chunked ")));
          res.end("world");
          console.log(res.finished, res.statusCode);
        }});
        server.listen(0, "127.0.0.1", () => {{
          rawRequest(server.address().port, "GET /path?q=1 HTTP/1.1\r\nHost: example.test\r\n\r\n", (raw) => {{
            const r = split(raw);
            console.log(r.status);
            console.log(r.names);
            console.log(JSON.stringify(r.body));
            server.close();
          }});
        }});
    "#));
    assert!(text.starts_with("request GET /path?q=1 example.test 1.1 true\ntext/plain true false false\ntrue true\ntrue\ntrue 200\nHTTP/1.1 200 OK\n"), "{text:?}");
    let (_, tail) = text.split_once("HTTP/1.1 200 OK\n").expect("status line");
    assert_eq!(
        tail,
        "content-type,date,transfer-encoding,x-multi,x-multi\n\"6\\r\\nhello \\r\\n8\\r\\nchunked \\r\\n5\\r\\nworld\\r\\n0\\r\\n\\r\\n\"\n"
    );
}

/// Pins a single-write response: writeHead with fields, end(body) framed by content-length, and a 404 when no listener is attached.
#[test]
fn http_single_write_response_and_default_404() {
    let text = out(&format!(r#"{RAW}
        const server = http.createServer((req, res) => {{
          if (req.url === "/made") res.writeHead(201, {{ "X-A": "1", "Content-Type": "text/plain" }}).end("abc");
          else res.statusCode = 404, res.end();
        }});
        const bare = http.createServer();
        server.listen(0, "127.0.0.1", () => bare.listen(0, "127.0.0.1", () => {{
          rawRequest(server.address().port, "GET /made HTTP/1.1\r\nHost: x\r\n\r\n", (raw) => {{
            const r = split(raw);
            console.log(r.status, r.names, JSON.stringify(r.body));
            rawRequest(bare.address().port, "GET / HTTP/1.1\r\nHost: x\r\n\r\n", (raw2) => {{
              const r2 = split(raw2);
              console.log(r2.status, r2.names, JSON.stringify(r2.body));
              server.close(); bare.close();
            }});
          }});
        }}));
    "#));
    assert_eq!(
        text,
        "HTTP/1.1 201 Created content-length,content-type,date,x-a \"abc\"\n\
         HTTP/1.1 404 Not Found content-length,date \"\"\n"
    );
}

/// Pins that a second res.end() is a no-op that returns the response, and that write after end returns false.
#[test]
fn http_end_twice_is_ignored() {
    let text = out(&format!(r#"{RAW}
        const server = http.createServer((req, res) => {{
          res.end("first");
          const again = res.end("second");
          console.log(again === res, res.write("late"), res.finished, res.end() === res);
          try {{ res.setHeader("X", "1"); }} catch (e) {{ console.log(e.code); }}
        }});
        const streamed = http.createServer((req, res) => {{
          res.write("a");
          res.end("b");
          res.end("c");
          res.end();
          console.log(res.write("d"));
        }});
        server.listen(0, "127.0.0.1", () => streamed.listen(0, "127.0.0.1", () => {{
          rawRequest(server.address().port, "GET / HTTP/1.1\r\nHost: x\r\n\r\n", (raw) => {{
            console.log(JSON.stringify(split(raw).body));
            rawRequest(streamed.address().port, "GET / HTTP/1.1\r\nHost: x\r\n\r\n", (raw2) => {{
              console.log(JSON.stringify(split(raw2).body));
              server.close(); streamed.close();
            }});
          }});
        }}));
    "#));
    assert_eq!(
        text,
        "true false true true\nERR_HTTP_HEADERS_SENT\n\"first\"\nfalse\n\"1\\r\\na\\r\\n1\\r\\nb\\r\\n0\\r\\n\\r\\n\"\n"
    );
}

/// Pins the request side: header parsing (lowercase, joined duplicates, cookies, set-cookie array), a POST body via events and via for await.
#[test]
fn http_request_headers_and_body() {
    let text = out(&format!(r#"{RAW}
        const server = http.createServer(async (req, res) => {{
          if (req.url === "/headers") {{
            const h = req.headers;
            console.log(h["x-dup"], h.cookie, h["content-type"], Object.keys(h).includes("Host"), h.host, typeof h["x-missing"]);
            res.end("ok");
          }} else if (req.url === "/events") {{
            let body = "";
            req.on("data", (d) => {{ body += d.toString(); console.log("data", d instanceof Buffer); }});
            req.on("end", () => {{ console.log("end", req.method); res.end("got:" + body); }});
          }} else {{
            const parts = [];
            for await (const chunk of req) parts.push(chunk.toString());
            res.end("iter:" + parts.join("|"));
          }}
        }});
        function post(port, url, body, next) {{
          rawRequest(port, "POST " + url + " HTTP/1.1\r\nHost: x\r\nContent-Length: " + body.length + "\r\n\r\n" + body, (raw) => {{
            console.log(JSON.stringify(split(raw).body));
            next();
          }});
        }}
        server.listen(0, "127.0.0.1", () => {{
          const port = server.address().port;
          rawRequest(port, "GET /headers HTTP/1.1\r\nHost: Example\r\nX-Dup: 1\r\nx-dup:  2 \r\nCookie: a=1\r\nCookie: b=2\r\nContent-Type: text/x\r\n\r\n", (raw) => {{
            console.log(JSON.stringify(split(raw).body));
            post(port, "/events", "hello", () => post(port, "/iter", "world", () => server.close()));
          }});
        }});
    "#));
    assert_eq!(
        text,
        "1, 2 a=1; b=2 text/x false Example undefined\n\"ok\"\ndata true\nend POST\n\"got:hello\"\n\"iter:world\"\n"
    );
}

/// Pins that http.createServer exposes its constants and classes and that a server is a net server with address and close.
#[test]
fn http_module_surface() {
    let text = out(r#"
        import http, { createServer, METHODS, Server } from "node:http";
        import net from "node:net";
        console.log(METHODS.join(), createServer === http.createServer, Server === http.Server);
        const s = http.createServer((req, res) => {});
        console.log(s instanceof Server, s instanceof net.Server, s.listening, s.address(), s.listenerCount("request"));
        const plain = createServer({}, () => {});
        console.log(plain.listenerCount("request"), createServer().listenerCount("request"));
        console.log(typeof http.IncomingMessage, typeof http.ServerResponse, net.createConnection === net.connect);
        const res = new http.ServerResponse(0);
        console.log(res.statusCode, res.headersSent, res.finished, res.getHeader("none"), res.setHeader("A", "b") === res);
        try { res.setHeader("X", "a\r\nInjected: 1"); } catch (e) { console.log(e.name, e.message); }
        try { res.setHeader("X\n", "a"); } catch (e) { console.log(e.name); }
    "#);
    assert_eq!(
        text,
        "GET,HEAD,POST,PUT,DELETE,PATCH,OPTIONS true true\ntrue true false null 1\n1 0\nfunction function true\n200 false false undefined true\n\
         TypeError Invalid character in header \"X\"\nTypeError\n"
    );
}

/// Pins process.exit: stdout written before it reaches the host, the status is the argument, then exitCode, then 0, and nothing after it runs.
///
/// The test runs itself in a child process (`NODE_SURFACE_EXIT_SCRIPT` set), where the host exits for real.
#[test]
fn process_exit_asks_the_host_to_exit() {
    if let Ok(script) = std::env::var("NODE_SURFACE_EXIT_SCRIPT") {
        run(&script);
        unreachable!("process.exit returned");
    }
    for (code, expected) in [
        ("process.exit(3)", 3),
        ("process.exit()", 0),
        ("process.exitCode = 5; process.exit()", 5),
        ("process.exitCode = 5; process.exit(2)", 2),
        ("process.exit(\"4\")", 4),
        ("setTimeout(() => process.exit(6), 1); setTimeout(() => console.log('never'), 30)", 6),
    ] {
        let saved = scratch("process_exit_asks_the_host_to_exit") + "/stdout.txt";
        let script = format!("console.log('before'); {code}; console.log('after');");
        let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", "process_exit_asks_the_host_to_exit", "--test-threads=1"])
            .env("NODE_SURFACE_EXIT_SCRIPT", &script)
            .env("NODE_SURFACE_EXIT_OUT", &saved)
            .output()
            .expect("child test process");
        assert_eq!(status.status.code(), Some(expected), "{code}");
        let printed = std::fs::read_to_string(&saved).expect("the child saved stdout");
        assert_eq!(printed, if code.starts_with("setTimeout") { "before\nafter\n" } else { "before\n" }, "{code}");
    }
}
