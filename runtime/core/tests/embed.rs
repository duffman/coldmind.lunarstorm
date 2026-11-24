//! The Runtime under a host that is not a command line: "your host" in the
//! layering. The host here answers every service from memory, which proves the
//! Runtime reaches the outside world only through the host contract, and
//! exercises the whole Runtime interface: `register`, `start`, `load`, `call`,
//! `post`, `poll` and `stop`.

use lunarstorm_runtime::vm::{Call, Kind, Limits, Scope, Vm};
use lunarstorm_runtime::{Access, Arg, Bindings, CallId, Host, Log, Runtime, Source, State, Stream, Wait};
use lunarstorm_vm_quickjs::QuickJs;
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

/// A host with no process behind it: output is captured, policy is a deny list.
#[derive(Default)]
struct MemoryHost {
    /// Everything written to stdout.
    out: RefCell<Vec<u8>>,
    /// Everything written to stderr.
    err: RefCell<Vec<u8>>,
    /// Log sink records, formatted.
    logs: RefCell<Vec<String>>,
    /// Paths containing this text are refused; empty refuses nothing.
    deny_path: &'static str,
    /// Whether listening is refused.
    deny_listen: bool,
    /// When set, the monotonic clock is simulated: it starts here and advances one
    /// microsecond per reading, so loop behaviour that depends on elapsed time is
    /// the same on every machine.
    simulated_clock: Option<Instant>,
    /// Readings of the simulated clock so far.
    clock_readings: Cell<u64>,
    /// Whether the host keeps running after an uncaught script error, as a
    /// desktop host does; `false` keeps the default (log, then exit).
    survive: bool,
}

impl Host for MemoryHost {
    /// The tests read the pulse.
    const PULSE: bool = true;

    /// # Returns
    /// A fixed `process.argv`.
    fn args(&self) -> Vec<String> {
        vec![String::from("embed"), String::from("main"), String::from("--flag")]
    }

    /// # Returns
    /// One variable, to show `process.env` is the host's, not the process's.
    fn environment(&self) -> Vec<(String, String)> {
        vec![(String::from("EMBEDDED"), String::from("yes"))]
    }

    /// # Returns
    /// A directory that exists everywhere.
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
        panic!("the runtime asked the host to exit with {code}; logs: {:?}", self.logs.borrow());
    }

    /// # Arguments
    /// * `access` - what a script is about to do.
    ///
    /// # Returns
    /// `false` for denied paths and, when configured, for listening.
    fn allow(&self, access: Access<'_>) -> bool {
        match access {
            Access::Read(path) | Access::Write(path) => self.deny_path.is_empty() || !path.contains(self.deny_path),
            Access::Listen(_) => !self.deny_listen,
            Access::Connect(..) => true,
        }
    }

    /// # Returns
    /// The simulated clock when one is configured, else the real one.
    fn monotonic(&self) -> Instant {
        match self.simulated_clock {
            Some(origin) => {
                let readings = self.clock_readings.get() + 1;
                self.clock_readings.set(readings);
                origin + Duration::from_micros(readings)
            }
            None => Instant::now(),
        }
    }

    /// Fills the buffer with a known byte, so the test can tell the host was asked.
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

    /// Records the error and, for a surviving host, returns so the loop goes on.
    ///
    /// # Arguments
    /// * `in_promise` - whether it is a rejected promise.
    /// * `description` - the error as `console` would show it.
    fn uncaught(&self, in_promise: bool, description: &str) {
        self.log(Log::Uncaught { in_promise, description });
        if !self.survive {
            self.exit(1)
        }
    }
}

/// Starts a Runtime under a memory host and loads source text as its entry module.
///
/// # Arguments
/// * `host` - the host.
/// * `bindings` - host bindings to register.
/// * `code` - the entry module's source.
///
/// # Returns
/// The loaded Runtime.
fn boot(host: MemoryHost, bindings: Bindings<QuickJs>, code: &str) -> Runtime<QuickJs, MemoryHost> {
    let mut runtime = Runtime::<QuickJs, MemoryHost>::new(host).expect("event loop");
    runtime.register(bindings);
    runtime.start().expect("start");
    runtime.load(Source::Text { name: String::from("main.js"), code: code.to_owned() }).expect("load");
    runtime
}

/// # Arguments
/// * `runtime` - a Runtime.
///
/// # Returns
/// What its scripts have written to stdout so far, as text.
fn stdout(runtime: &Runtime<QuickJs, MemoryHost>) -> String {
    String::from_utf8(runtime.host().out.borrow().clone()).expect("stdout is UTF-8")
}

/// Steps the loop until a call settles.
///
/// # Arguments
/// * `runtime` - the Runtime.
/// * `call` - the call to wait for.
///
/// # Returns
/// How the call's promise settled.
fn settle(runtime: &mut Runtime<QuickJs, MemoryHost>, call: CallId) -> Result<Arg, String> {
    loop {
        if let Some(result) = runtime.result(call) {
            return result;
        }
        // A step that fires the last timer reports `Idle` (nothing is left to wait
        // for) and may be the very step that settled the call, so the result is
        // checked again before idleness counts as a failure.
        if runtime.poll(Wait::Block) == State::Idle {
            return runtime.result(call).expect("the loop went idle before the call settled");
        }
    }
}

/// Calls the global function `hit`. Written against the VM contract only, as a
/// host's own code would be.
///
/// # Arguments
/// * `scope` - the context handle a posted task receives.
fn hit<'s, V: Vm>(scope: &V::Scope<'s>) {
    let function = scope.get(&scope.global(), "hit").expect("global hit");
    assert!(scope.kind(&function) == Kind::Function);
    assert!(scope.call(&function, &[scope.string("posted").expect("string")]).is_ok());
}

#[test]
fn services_come_from_the_host() {
    let mut runtime = boot(
        MemoryHost::default(),
        Bindings::new(),
        r#"
        console.log(process.argv.join(" "), process.env.EMBEDDED, process.cwd());
        console.log(crypto.getRandomValues(new Uint8Array(3)).join(","));
        console.error("to stderr");
        console.log(import.meta.filename, import.meta.main);
        "#,
    );
    assert_eq!(runtime.run(), 0);
    assert_eq!(stdout(&runtime), "embed main --flag yes /\n7,7,7\n/main.js true\n");
    assert_eq!(runtime.host().err.borrow().as_slice(), b"to stderr\n");
    assert!(runtime.host().logs.borrow().is_empty());
}

#[test]
fn call_settles_with_values_and_errors() {
    let mut runtime = boot(
        MemoryHost::default(),
        Bindings::new(),
        r#"
        export function add(a, b) { return a + b; }
        export async function shout(text) {
          await new Promise((resolve) => setTimeout(resolve, 5));
          return text.toUpperCase();
        }
        export function size(bytes) { return bytes.length; }
        export function record() { return { ok: true, list: [1, 2] }; }
        export function reverse(bytes) { return bytes.reverse(); }
        export function nothing() {}
        export function boom() { throw new TypeError("nope"); }
        export async function late() { await null; throw new RangeError("later"); }
        export const constant = 1;
        "#,
    );

    let add = runtime.call("add", &[Arg::Number(2.0), Arg::Number(40.0)]).expect("add");
    // A synchronous export has settled by the time `call` returns.
    assert_eq!(runtime.result(add), Some(Ok(Arg::Number(42.0))));
    assert_eq!(runtime.result(add), None, "a result is collected once");

    let shout = runtime.call("shout", &[Arg::Text(String::from("storm"))]).expect("shout");
    assert_eq!(runtime.result(shout), None, "an async export is pending until its timer fires");
    assert_eq!(settle(&mut runtime, shout), Ok(Arg::Text(String::from("STORM"))));

    let size = runtime.call("size", &[Arg::Bytes(vec![1, 2, 3])]).expect("size");
    assert_eq!(settle(&mut runtime, size), Ok(Arg::Number(3.0)));
    let record = runtime.call("record", &[]).expect("record");
    assert_eq!(settle(&mut runtime, record), Ok(Arg::Text(String::from(r#"{"ok":true,"list":[1,2]}"#))));
    let reverse = runtime.call("reverse", &[Arg::Bytes(vec![1, 2, 3])]).expect("reverse");
    assert_eq!(settle(&mut runtime, reverse), Ok(Arg::Bytes(vec![3, 2, 1])));
    let nothing = runtime.call("nothing", &[Arg::Null, Arg::Bool(true)]).expect("nothing");
    assert_eq!(settle(&mut runtime, nothing), Ok(Arg::Null));

    let boom = runtime.call("boom", &[]).expect("boom");
    let error = settle(&mut runtime, boom).expect_err("a throw rejects the call");
    assert!(error.starts_with("TypeError: nope"), "{error}");
    let late = runtime.call("late", &[]).expect("late");
    let error = settle(&mut runtime, late).expect_err("an async throw rejects the call");
    assert!(error.starts_with("RangeError: later"), "{error}");

    assert_eq!(runtime.call("constant", &[]).err().as_deref(), Some("export 'constant' is not a function"));
    assert_eq!(runtime.call("missing", &[]).err().as_deref(), Some("export 'missing' is not a function"));
    assert_eq!(runtime.stop(), 0);
    assert!(runtime.host().logs.borrow().is_empty(), "a rejected call is the host's to handle, not uncaught");
}

#[test]
fn post_runs_tasks_from_another_thread_on_the_loop() {
    let mut runtime = boot(
        MemoryHost::default(),
        Bindings::new(),
        r#"
        let hits = 0;
        globalThis.hit = (from) => {
          hits += 1;
          Promise.resolve().then(() => console.log("hit", hits, from));
        };
        "#,
    );
    assert_eq!(runtime.poll(Wait::NoBlock), State::Idle, "nothing is scheduled yet");

    let poster = runtime.poster();
    let thread = std::thread::spawn(move || {
        for _ in 0..3 {
            assert!(poster.post(|scope| hit::<QuickJs>(scope)));
        }
    });
    thread.join().expect("poster thread");

    // Queued tasks make the loop runnable; one step drains the burst, and the
    // promise job each task queued runs right after it.
    assert_eq!(runtime.poll(Wait::Block), State::Running);
    assert_eq!(runtime.poll(Wait::NoBlock), State::Idle);
    assert_eq!(runtime.stop(), 0);
    assert_eq!(stdout(&runtime), "hit 1 posted\nhit 2 posted\nhit 3 posted\n");

    let poster = runtime.poster();
    drop(runtime);
    assert!(!poster.post(|_| {}), "posting to a dropped runtime reports failure");
}

#[test]
fn host_bindings_and_modules_are_registered_by_the_host() {
    let bindings = Bindings::<QuickJs>::new()
        .function("double", |call: &Call<'_, QuickJs>| {
            let scope = call.scope();
            let number = call.args().first().and_then(|value| Scope::as_number(scope, value)).unwrap_or(f64::NAN);
            Ok(Scope::number(scope, number * 2.0))
        })
        .values(|scope, ops| {
            let name = Scope::string(scope, "memory")?;
            Scope::set(scope, ops, "hostName", name)
        })
        .module(
            "host:math",
            "const { ops } = __lunarstorm.internal;\nexport const double = (n) => ops.double(n);\nexport const host = ops.hostName;\n",
        );
    let mut runtime = boot(
        MemoryHost::default(),
        bindings,
        r#"
        import { double, host } from "host:math";
        import { join } from "node:path";
        console.log(double(21), host, join("a", "b"));
        "#,
    );
    assert_eq!(runtime.run(), 0);
    assert_eq!(stdout(&runtime), "42 memory a/b\n");
}

#[test]
fn host_policy_refuses_fs_and_net() {
    let host = MemoryHost { deny_path: "secret", deny_listen: true, ..MemoryHost::default() };
    let mut runtime = boot(
        host,
        Bindings::new(),
        r#"
        import fs from "node:fs";
        import net from "node:net";
        const attempt = (label, action) => {
          try { action(); console.log(label, "allowed"); } catch (error) { console.log(label, error.code); }
        };
        attempt("read", () => fs.readFileSync("/tmp/secret.txt"));
        attempt("write", () => fs.writeFileSync("/tmp/secret.txt", "x"));
        attempt("rename", () => fs.renameSync("/tmp/lunarstorm-absent", "/tmp/secret-target"));
        attempt("exists", () => fs.existsSync("/"));
        // Awaited so the two reports below are ordered by the script, not by the
        // relative priority of promise jobs and `nextTick`.
        await fs.promises.readFile("/tmp/secret.txt").catch((error) => console.log("async", error.code));
        net.createServer().on("error", (error) => console.log("listen", error.code)).listen(0, "127.0.0.1");
        "#,
    );
    assert_eq!(runtime.run(), 0);
    assert_eq!(
        stdout(&runtime),
        "read EACCES\nwrite EACCES\nrename EACCES\nexists allowed\nasync EACCES\nlisten EACCES\n"
    );
}

#[test]
fn poll_steps_the_loop_for_a_host_with_its_own() {
    let mut runtime = boot(
        MemoryHost::default(),
        Bindings::new(),
        r#"
        let ticks = 0;
        const timer = setInterval(() => {
          console.log("tick", ++ticks);
          if (ticks === 3) { clearInterval(timer); process.exitCode = 9; }
        }, 2);
        "#,
    );
    let mut steps = 0;
    while runtime.poll(Wait::NoBlock) == State::Running {
        steps += 1;
        // The host's own work would go here.
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(steps >= 3, "three timer ticks need at least three steps, took {steps}");
    assert_eq!(runtime.stop(), 9);
    assert_eq!(stdout(&runtime), "tick 1\ntick 2\ntick 3\n");
    assert_eq!(runtime.poll(Wait::NoBlock), State::Idle, "a stopped runtime is idle");
    assert_eq!(runtime.stop(), 0, "stopping twice is harmless");
}

#[test]
fn lifecycle_errors_are_reported_not_panics() {
    let mut runtime = Runtime::<QuickJs, MemoryHost>::new(MemoryHost::default()).expect("event loop");
    assert_eq!(runtime.load(Source::Text { name: "a.js".into(), code: String::new() }).err().as_deref(), Some("the runtime is not started"));
    assert_eq!(runtime.call("x", &[]).err().as_deref(), Some("the runtime is not started"));
    assert!(runtime.interrupt().is_none());
    runtime.limits(Limits { memory: Some(64 * 1024 * 1024), stack: None });
    runtime.start().expect("start");
    assert_eq!(runtime.start().err().as_deref(), Some("the runtime is already started"));
    assert_eq!(runtime.call("x", &[]).err().as_deref(), Some("no module is loaded"));
    assert!(runtime.interrupt().is_some());

    let denied = MemoryHost { deny_path: "Cargo", ..MemoryHost::default() };
    let mut runtime = Runtime::<QuickJs, MemoryHost>::new(denied).expect("event loop");
    runtime.start().expect("start");
    let error = runtime.load(Source::File("Cargo.toml".into())).expect_err("policy refuses the entry module");
    assert_eq!(error, "cannot read Cargo.toml: permission denied by host policy");
    let error = runtime.load(Source::File("no-such-file.js".into())).expect_err("missing file");
    assert!(error.starts_with("cannot read no-such-file.js: "), "{error}");
}

/// Defect: with a `setImmediate` callback queued, every loop turn asked the kernel
/// for readiness, so a chain of immediates paid one syscall per callback.
#[test]
fn queued_immediates_do_not_poll_the_kernel_every_turn() {
    let host = MemoryHost { simulated_clock: Some(Instant::now()), ..MemoryHost::default() };
    let mut runtime = boot(
        host,
        Bindings::new(),
        r#"
        import fs from "node:fs";
        // The chain spins until an async read (a worker result, which only a
        // kernel poll can deliver) arrives, then for 10000 turns more.
        let turns = 0, arrived = 0;
        fs.promises.readFile("Cargo.toml").then(() => { arrived = turns; });
        const spin = () => {
          turns += 1;
          if (arrived === 0 && turns === 50_000_000) return console.log("starved");
          if (arrived > 0 && turns >= arrived + 10000) return console.log("turns", turns);
          setImmediate(spin);
        };
        setImmediate(spin);
        "#,
    );
    assert_eq!(runtime.run(), 0);
    let output = stdout(&runtime);
    let turns: u64 = output.trim().strip_prefix("turns ").unwrap_or_else(|| panic!("I/O never arrived: {output}")).parse().expect("turn count");
    assert!(turns >= 10_000, "{turns}");
    let polls = runtime.polls();
    assert!(polls > 0, "the worker result needs at least one kernel poll");
    assert!(polls * 100 <= turns, "{polls} kernel polls for {turns} immediate turns: more than 1 poll per 100 turns");
}

#[test]
fn a_host_may_survive_uncaught_script_errors() {
    let host = MemoryHost { survive: true, ..MemoryHost::default() };
    let code = "setTimeout(() => { throw new Error('thrown in a timer'); }, 0);\n\
                Promise.reject(new Error('rejected and dropped'));\n\
                setImmediate(() => { throw new Error('thrown in an immediate'); });\n\
                setTimeout(() => console.log('still running'), 5);";
    let mut runtime = boot(host, Bindings::new(), code);
    while runtime.poll(Wait::Block) == State::Running {}
    runtime.stop();
    let logs = runtime.host().logs.borrow().join("\n");
    assert!(logs.contains("in_promise: true") && logs.contains("rejected and dropped"), "{logs}");
    assert!(logs.contains("in_promise: false") && logs.contains("thrown in a timer"), "{logs}");
    assert!(logs.contains("thrown in an immediate"), "{logs}");
    assert_eq!(runtime.host().logs.borrow().len(), 3, "{logs}");
    assert_eq!(stdout(&runtime), "still running\n");
}

#[test]
fn a_surviving_host_gets_a_failed_load_as_an_error() {
    let host = MemoryHost { survive: true, ..MemoryHost::default() };
    let mut runtime = Runtime::<QuickJs, MemoryHost>::new(host).expect("event loop");
    runtime.start().expect("start");
    let loaded = runtime.load(Source::Text { name: String::from("main.js"), code: String::from("let = = 1;") });
    assert!(loaded.is_err());
    let logs = runtime.host().logs.borrow().join("\n");
    assert!(logs.contains("SyntaxError"), "{logs}");
}

#[test]
fn a_held_loop_sleeps_until_work_is_posted() {
    let mut runtime = boot(MemoryHost::default(), Bindings::new(), "globalThis.hit = (text) => console.log(text);");
    // Nothing is open: the loop is idle and `poll` returns at once.
    assert_eq!(runtime.poll(Wait::Block), State::Idle);

    runtime.hold(true);
    let poster = runtime.poster();
    let started = Instant::now();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        poster.post(hit::<QuickJs>);
    });
    // Sleeps until the task is posted, then runs it. Output is handed to the host
    // at the start of the next step, which must not sleep: nothing would wake it.
    assert_eq!(runtime.poll(Wait::Block), State::Running);
    assert_eq!(runtime.poll(Wait::NoBlock), State::Running);
    thread.join().expect("poster thread");
    assert!(started.elapsed() >= Duration::from_millis(40), "the held loop did not wait for the task");
    assert!(runtime.polls() <= 4, "the held loop spun: {} polls", runtime.polls());
    assert_eq!(stdout(&runtime), "posted\n");

    runtime.hold(false);
    assert_eq!(runtime.poll(Wait::Block), State::Idle);
}

/// Defect: under a memory limit, a script that caught the out-of-memory error went
/// on running on an exhausted heap and the program ended normally.
#[test]
#[should_panic(expected = "the engine ran out of memory")]
fn a_caught_out_of_memory_error_still_ends_the_program() {
    let mut runtime = Runtime::<QuickJs, MemoryHost>::new(MemoryHost::default()).expect("event loop");
    runtime.limits(Limits { memory: Some(8 << 20), stack: None });
    runtime.start().expect("start");
    let code = "const hoard = [];\n\
                try { for (;;) hoard.push(new Array(65536).fill(1)); } catch { hoard.length = 0; }\n\
                console.log('survived');";
    runtime.load(Source::Text { name: String::from("main.js"), code: String::from(code) }).expect("load");
    runtime.run();
}

#[test]
fn a_watchdog_ends_a_script_that_overruns_its_budget() {
    use std::sync::atomic::Ordering;
    let host = MemoryHost { survive: true, ..MemoryHost::default() };
    let code = "setTimeout(() => { for (;;) {} }, 0);\nsetTimeout(() => console.log('next event ran'), 1);";
    let mut runtime = boot(host, Bindings::new(), code);
    let pulse = runtime.pulse();
    assert_eq!(pulse.load(Ordering::Relaxed) % 2, 0, "the pulse is even while no script runs");
    let interrupt = runtime.interrupt().expect("started");
    let watchdog = std::thread::spawn(move || {
        // The budget is 50 ms: the same odd value seen twice, that far apart.
        let mut seen = pulse.load(Ordering::Relaxed);
        loop {
            std::thread::sleep(Duration::from_millis(50));
            let now = pulse.load(Ordering::Relaxed);
            if now == seen && now % 2 == 1 {
                interrupt.request();
                return;
            }
            seen = now;
        }
    });
    while runtime.poll(Wait::Block) == State::Running {}
    watchdog.join().expect("watchdog thread");
    let logs = runtime.host().logs.borrow().join("\n");
    assert!(logs.contains("in_promise: false"), "{logs}");
    assert_eq!(stdout(&runtime), "next event ran\n");
    assert_eq!(runtime.pulse().load(Ordering::Relaxed) % 2, 0);
}
