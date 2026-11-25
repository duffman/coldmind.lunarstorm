//! `lunarstorm:worker`: workers on their own threads, each its own Runtime. The
//! scenarios are ported from jetbunny's worker harness cases (`w-basic`,
//! `t-backpressure`, `t-nested`, `t-terminate-chaos`, `std-worker-*`).
//!
//! Each scenario writes its program files into a directory under the cargo target
//! directory, runs the main program on a thread of its own and fails (instead of
//! hanging the suite) when it has not finished after 30 seconds.

use lunarstorm_runtime::{Access, Host, Log, Runtime, Source, Stream};
use lunarstorm_vm_quickjs::QuickJs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// What every thread's host writes to, shared across the main program and workers.
#[derive(Clone, Default)]
struct Sink {
    /// stdout, all threads, in write order.
    out: Arc<Mutex<Vec<u8>>>,
    /// Uncaught-error records, all threads.
    errors: Arc<Mutex<Vec<String>>>,
    /// The scenario's directory: the working directory and where worker files live.
    dir: String,
}

/// A host whose workers write to the same sink. `W` turns `lunarstorm:worker` on.
struct TestHost<const W: bool>(Sink);

impl<const W: bool> Host for TestHost<W> {
    /// No extensions.
    type Extensions<V: lunarstorm_runtime::vm::Vm> = ();

    /// # Returns
    /// The empty set.
    fn extensions<V: lunarstorm_runtime::vm::Vm>(&self) -> Self::Extensions<V> {}

    const WORKERS: bool = W;

    /// # Returns
    /// A host over the same sink, built on the worker's thread.
    fn worker_host(&self) -> Option<Box<dyn FnOnce() -> Self + Send>> {
        let sink = self.0.clone();
        Some(Box::new(move || TestHost(sink)))
    }

    /// # Returns
    /// A fixed `process.argv`.
    fn args(&self) -> Vec<String> {
        vec![String::from("worker"), String::from("main")]
    }

    /// # Returns
    /// No variables.
    fn environment(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// # Returns
    /// The scenario's directory.
    fn working_directory(&self) -> String {
        self.0.dir.clone()
    }

    /// Captures stdout; stderr is dropped.
    ///
    /// # Arguments
    /// * `stream` - which stream.
    /// * `bytes` - what was written.
    fn write(&self, stream: Stream, bytes: &[u8]) {
        if stream == Stream::Out {
            self.0.out.lock().unwrap().extend_from_slice(bytes);
        }
    }

    /// Fails the test (or, on a worker thread, ends that thread with a panic).
    ///
    /// # Arguments
    /// * `code` - the exit status the Runtime asked for.
    fn exit(&self, code: i32) -> ! {
        panic!("the runtime asked the host to exit with {code}; errors: {:?}", self.0.errors.lock().unwrap());
    }

    /// # Returns
    /// `true`: the scenarios touch only their own directory.
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

    /// Ignores the record.
    ///
    /// # Arguments
    /// * `_record` - the record.
    fn log(&self, _record: Log<'_>) {}

    /// Records the error and survives it.
    ///
    /// # Arguments
    /// * `_in_promise` - whether it is a rejected promise.
    /// * `description` - the error as `console` would show it.
    fn uncaught(&self, _in_promise: bool, description: &str) {
        self.0.errors.lock().unwrap().push(description.to_owned());
    }
}

/// What a scenario produced.
struct Outcome {
    /// stdout.
    out: String,
    /// Uncaught-error records.
    errors: Vec<String>,
}

/// Runs a main program with the given files next to it.
///
/// # Arguments
/// * `name` - the scenario's directory name.
/// * `files` - `(file name, source)` pairs written before the run.
/// * `main` - the entry module's source.
///
/// # Returns
/// What the program printed and what went uncaught.
///
/// # Panics
/// When the program load fails or the scenario takes more than 30 seconds.
fn scenario<const W: bool>(name: &str, files: &[(&str, &str)], main: &str) -> Outcome {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("worker-tests").join(name);
    std::fs::create_dir_all(&dir).expect("scenario directory");
    for (file, source) in files {
        std::fs::write(dir.join(file), source).expect("scenario file");
    }
    let sink = Sink { dir: dir.to_string_lossy().into_owned(), ..Sink::default() };
    let (done, finished) = mpsc::channel();
    let (host_sink, main) = (sink.clone(), main.to_owned());
    thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let mut runtime = Runtime::<QuickJs, TestHost<W>>::new(TestHost(host_sink)).map_err(|e| e.to_string())?;
            runtime.start()?;
            runtime.load(Source::Text { name: String::from("main.js"), code: main })?;
            runtime.run();
            Ok(())
        })();
        let _ = done.send(result);
    });
    match finished.recv_timeout(Duration::from_secs(30)) {
        Ok(Ok(())) => {}
        Ok(Err(message)) => panic!("scenario {name}: {message}; errors: {:?}", sink.errors.lock().unwrap()),
        Err(_) => panic!("scenario {name} did not finish within 30 seconds"),
    }
    let out = String::from_utf8(sink.out.lock().unwrap().clone()).expect("stdout is UTF-8");
    let errors = sink.errors.lock().unwrap().clone();
    Outcome { out, errors }
}

/// A worker that stays alive until terminated or closed.
const IDLE: &str = "import { onMessage } from 'lunarstorm:worker'; onMessage(() => {});";

#[test]
fn a_worker_answers_messages_and_ends_when_it_closes() {
    let outcome = scenario::<true>(
        "basic",
        &[(
            "double.js",
            "import { onMessage, postMessage, close } from 'lunarstorm:worker';
             onMessage((m) => { postMessage({ n: m.n * 2 }); if (m.last) close(); });",
        )],
        "import { spawn } from 'lunarstorm:worker';
         const w = spawn('double.js');
         let sum = 0;
         w.onMessage((m) => { sum += m.n; });
         for (let i = 1; i <= 3; i++) if (!w.postMessage({ n: i, last: i === 3 })) throw new Error('message refused');
         const exit = await w.exited;
         console.log('sum', sum, JSON.stringify(exit));",
    );
    assert_eq!(outcome.out, "sum 12 {\"state\":\"fulfilled\"}\n");
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
}

#[test]
fn a_listener_registered_after_the_worker_ended_still_gets_what_it_sent() {
    let outcome = scenario::<true>(
        "late-listener",
        &[("chatty.js", "import { postMessage } from 'lunarstorm:worker'; for (let n = 1; n <= 5; n++) postMessage({ n });")],
        "import { spawn } from 'lunarstorm:worker';
         const w = spawn('chatty.js');
         const exit = await w.exited;
         const got = [];
         w.onMessage((m) => got.push(m.n));
         await new Promise((resolve) => setTimeout(resolve, 100));
         console.log(JSON.stringify(exit), got.join());",
    );
    assert_eq!(outcome.out, "{\"state\":\"fulfilled\"} 1,2,3,4,5\n");
}

#[test]
fn every_message_a_worker_sent_arrives_before_its_exit_is_reported() {
    let outcome = scenario::<true>(
        "delivered-before-exit",
        &[("flood.js", "import { postMessage } from 'lunarstorm:worker'; for (let n = 0; n < 100; n++) postMessage({ n });")],
        "import { spawn } from 'lunarstorm:worker';
         const w = spawn('flood.js');
         let count = 0;
         w.onMessage(() => { count += 1; });
         const exit = await w.exited;
         console.log(count, exit.state);",
    );
    assert_eq!(outcome.out, "100 fulfilled\n");
}

#[test]
fn a_listener_that_throws_is_reported_and_later_messages_still_arrive() {
    let outcome = scenario::<true>(
        "throwing-listener",
        &[("three.js", "import { postMessage } from 'lunarstorm:worker'; for (let n = 1; n <= 3; n++) postMessage({ n });")],
        "import { spawn } from 'lunarstorm:worker';
         const w = spawn('three.js');
         const got = [];
         w.onMessage((m) => { if (m.n === 1) throw new Error('boom'); got.push(m.n); });
         await w.exited;
         console.log(got.join());",
    );
    assert_eq!(outcome.out, "2,3\n");
    assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
    assert!(outcome.errors[0].contains("boom"));
}

#[test]
fn terminate_ends_a_worker_that_is_waiting() {
    let outcome = scenario::<true>(
        "terminate-idle",
        &[("idle.js", IDLE)],
        "import { spawn } from 'lunarstorm:worker';
         const w = spawn('idle.js');
         w.terminate();
         console.log(JSON.stringify(await w.exited));",
    );
    assert_eq!(outcome.out, "{\"state\":\"terminated\"}\n");
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
}

#[test]
fn terminate_stops_a_worker_that_never_yields() {
    let outcome = scenario::<true>(
        "terminate-busy",
        &[("busy.js", "import { postMessage } from 'lunarstorm:worker'; postMessage('ready'); for (;;) {}")],
        "import { spawn } from 'lunarstorm:worker';
         const w = spawn('busy.js');
         w.onMessage(() => w.terminate());
         console.log(JSON.stringify(await w.exited));",
    );
    assert_eq!(outcome.out, "{\"state\":\"terminated\"}\n");
    assert!(outcome.errors.is_empty(), "terminating a busy worker is not an uncaught error: {:?}", outcome.errors);
}

#[test]
fn a_worker_that_does_not_read_refuses_messages_past_its_mailbox_bound() {
    let outcome = scenario::<true>(
        "backpressure",
        &[("deaf.js", "setInterval(() => {}, 1000);")],
        "import { spawn } from 'lunarstorm:worker';
         const w = spawn('deaf.js');
         let accepted = 0;
         while (w.postMessage(1) && accepted < 10000) accepted += 1;
         const small = w.postMessage(new Uint8Array(8 * 1024 * 1024 - 1));
         const big = w.postMessage(new Uint8Array(8 * 1024 * 1024));
         w.terminate();
         await w.exited;
         console.log(accepted, small, big);",
    );
    // 4096 queued messages is the bound, whatever their size.
    assert_eq!(outcome.out, "4096 false false\n");
}

#[test]
fn a_message_over_the_size_bound_is_refused_and_one_under_it_is_not() {
    let outcome = scenario::<true>(
        "size-bound",
        &[("deaf.js", "setInterval(() => {}, 1000);")],
        "import { spawn } from 'lunarstorm:worker';
         const w = spawn('deaf.js');
         const small = w.postMessage(new Uint8Array(8 * 1024 * 1024 - 1));
         const big = w.postMessage(new Uint8Array(8 * 1024 * 1024));
         w.terminate();
         await w.exited;
         console.log(small, big);",
    );
    assert_eq!(outcome.out, "true false\n");
}

#[test]
fn a_worker_can_start_workers_and_the_messages_travel_up_the_chain() {
    let outcome = scenario::<true>(
        "nested",
        &[
            ("a.js", "import { spawn, postMessage } from 'lunarstorm:worker';
                      const b = spawn('b.js');
                      b.onMessage((m) => postMessage('a got: ' + m));
                      await b.exited;"),
            ("b.js", "import { postMessage } from 'lunarstorm:worker'; postMessage('hello from b');"),
        ],
        "import { spawn } from 'lunarstorm:worker';
         const a = spawn('a.js');
         a.onMessage((m) => console.log(m));
         console.log(JSON.stringify(await a.exited));",
    );
    assert_eq!(outcome.out, "a got: hello from b\n{\"state\":\"fulfilled\"}\n");
}

/// A worker that registered a listener stays alive while idle and ends only on `close()`.
///
/// Fixes a flaky ordering: the script used to sleep 150 ms after `postMessage('hi')` and
/// assume the reply had arrived, so a worker slower than 150 ms to boot (cold glue
/// compile, parallel tests) printed `still alive` before `heard hi`. Decision: wait for
/// the first reply, then idle 150 ms; the 300 ms busy boot in the fixture makes the slow
/// case run every time. A longer sleep was not used, since it only narrows the race.
#[test]
fn a_worker_with_a_listener_lives_until_it_closes() {
    let outcome = scenario::<true>(
        "live-listener",
        &[(
            "listener.js",
            "import { onMessage, close, postMessage } from 'lunarstorm:worker';
             const ready = Date.now() + 300;
             while (Date.now() < ready) {}
             onMessage((m) => { postMessage('heard ' + m); if (m === 'bye') close(); });",
        )],
        "import { spawn } from 'lunarstorm:worker';
         const w = spawn('listener.js');
         let heard;
         const first = new Promise((resolve) => { heard = resolve; });
         w.onMessage((m) => { console.log(m); heard(); });
         w.postMessage('hi');
         await first;
         await new Promise((resolve) => setTimeout(resolve, 150));
         console.log('still alive');
         w.postMessage('bye');
         console.log(JSON.stringify(await w.exited));",
    );
    assert_eq!(outcome.out, "heard hi\nstill alive\nheard bye\n{\"state\":\"fulfilled\"}\n");
}

#[test]
fn at_most_64_workers_run_at_once() {
    let outcome = scenario::<true>(
        "limit",
        &[("idle.js", IDLE)],
        "import { spawn } from 'lunarstorm:worker';
         const workers = [];
         for (let i = 0; i < 64; i++) workers.push(spawn('idle.js', { maxMemory: 16 * 1024 * 1024 }));
         let refused = '';
         try { spawn('idle.js'); } catch (e) { refused = e.message; }
         console.log(refused);
         for (const w of workers) w.terminate();
         await Promise.all(workers.map((w) => w.exited));
         const again = spawn('idle.js');
         again.terminate();
         console.log((await again.exited).state);",
    );
    assert_eq!(outcome.out, "worker limit reached\nterminated\n");
}

#[test]
fn a_host_without_workers_does_not_serve_the_module() {
    let sink = Sink::default();
    let mut runtime = Runtime::<QuickJs, TestHost<false>>::new(TestHost(sink)).expect("event loop");
    runtime.start().expect("start");
    let loaded = runtime.load(Source::Text {
        name: String::from("main.js"),
        code: String::from("import { spawn } from 'lunarstorm:worker'; spawn('x.js');"),
    });
    assert!(loaded.is_err(), "WORKERS = false must not serve lunarstorm:worker");
}

#[test]
fn spawn_checks_its_arguments_and_the_entry() {
    let outcome = scenario::<true>(
        "arguments",
        &[("idle.js", IDLE)],
        "import { spawn } from 'lunarstorm:worker';
         for (const bad of [0, -1, 1.5, '1', null]) {
           try { spawn('idle.js', { maxMemory: bad }); console.log('no throw'); } catch (e) { console.log(e.name + ': ' + e.message); }
         }
         try { spawn(5); } catch (e) { console.log(e.name); }
         try { spawn('missing.js'); } catch (e) { console.log(e.message.startsWith('worker entry not found')); }",
    );
    let refused = "RangeError: maxMemory must be a positive byte count\n";
    assert_eq!(outcome.out, format!("{}TypeError\ntrue\n", refused.repeat(5)));
}

#[test]
fn an_entry_that_cannot_load_rejects_exited_with_a_worker_error() {
    let outcome = scenario::<true>(
        "broken-entry",
        &[("broken.js", "this is not javascript (")],
        "import { spawn } from 'lunarstorm:worker';
         const w = spawn('broken.js');
         try { await w.exited; console.log('fulfilled'); } catch (e) { console.log(e.code); }",
    );
    assert_eq!(outcome.out, "WorkerError\n");
}

#[test]
fn outside_a_worker_the_worker_side_calls_do_nothing() {
    let outcome = scenario::<true>(
        "outside",
        &[],
        "import { postMessage, onMessage, close } from 'lunarstorm:worker';
         console.log(postMessage({ a: 1 }), onMessage(() => {}), close());",
    );
    assert_eq!(outcome.out, "false false undefined\n");
}

#[test]
fn bytes_and_json_data_survive_the_trip() {
    let outcome = scenario::<true>(
        "data",
        &[(
            "echo.js",
            "import { onMessage, postMessage, close } from 'lunarstorm:worker';
             let seen = 0;
             onMessage((m) => { postMessage(m); if (++seen === 2) close(); });",
        )],
        "import { spawn } from 'lunarstorm:worker';
         const w = spawn('echo.js');
         w.onMessage((m) => console.log(m instanceof Uint8Array ? 'bytes ' + Array.from(m).join() : 'json ' + JSON.stringify(m)));
         w.postMessage(new Uint8Array([1, 2, 3]));
         w.postMessage({ list: [1, 'two', null], nested: { ok: true } });
         await w.exited;
         try { w.postMessage(() => {}); } catch (e) { console.log(e.name); }",
    );
    assert_eq!(outcome.out, "bytes 1,2,3\njson {\"list\":[1,\"two\",null],\"nested\":{\"ok\":true}}\nTypeError\n");
}

#[test]
fn dropping_the_runtime_ends_its_workers_without_hanging() {
    let sink = Sink {
        dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("worker-tests").join("drop").to_string_lossy().into_owned(),
        ..Sink::default()
    };
    std::fs::create_dir_all(&sink.dir).expect("scenario directory");
    std::fs::write(PathBuf::from(&sink.dir).join("idle.js"), IDLE).expect("scenario file");
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let mut runtime = Runtime::<QuickJs, TestHost<true>>::new(TestHost(sink)).expect("event loop");
        runtime.start().expect("start");
        runtime
            .load(Source::Text {
                name: String::from("main.js"),
                code: String::from("import { spawn } from 'lunarstorm:worker'; spawn('idle.js'); spawn('idle.js');"),
            })
            .expect("load");
        drop(runtime);
        let _ = done.send(());
    });
    finished.recv_timeout(Duration::from_secs(30)).expect("dropping a Runtime with live workers must end them");
}
