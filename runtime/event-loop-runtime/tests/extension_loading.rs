//! Runtime-level extensions: a host's extension set, selected by its manifest,
//! installed by `Runtime::start` in every Runtime, workers included.
//!
//! The test extension `probe` counts its `bindings` calls, keeps a counter per
//! Runtime behind `ops.probe_next`, and serves `lunarstorm:probe`, a source module
//! over those natives.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use lunarstorm_event_loop_runtime::vm::{Call, Scope, Sig, Ty, Vm};
use lunarstorm_event_loop_runtime::{Access, Bindings, Extension, Host, Log, Manifest, Runtime, Source, Stream, Use};
use lunarstorm_engine_quickjs_adapter::QuickJs;

/// The declaration of a test native that takes nothing and returns a number.
const COUNTER: Sig = Sig { params: &[], rest: None, returns: Ty::Number, about: "A number for the test." };

/// The module `probe` serves: its natives, reached through `lunarstorm:internal`.
const PROBE_SOURCE: &str = "import { ops } from 'lunarstorm:internal';
export const value = ops.probe_value();
export const next = () => ops.probe_next();";

/// How `probe` behaves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Serves `lunarstorm:probe`.
    Normal,
    /// Also serves `lunarstorm:other`, outside its namespace.
    Outside,
    /// Also binds `ops.host_fn`, which the host binds itself.
    Duplicate,
}

/// The test extension.
struct Probe {
    /// `bindings` calls across every Runtime of the test.
    calls: Arc<AtomicUsize>,
    /// What it does wrong, if anything.
    mode: Mode,
}

impl<V: Vm> Extension<V> for Probe {
    const NAME: &'static str = "probe";

    /// Counts the call and builds natives over a counter of this Runtime.
    ///
    /// # Arguments
    /// * `_using` - unused.
    ///
    /// # Returns
    /// The natives and `lunarstorm:probe`, plus whatever `mode` adds.
    fn bindings(&self, _using: &Use<'_>) -> Result<Bindings<V>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let counter = Rc::new(Cell::new(0));
        let mut bindings = Bindings::new()
            .function("probe_value", COUNTER, |call: &Call<'_, V>| Ok(call.scope().int(42)))
            .function("probe_next", COUNTER, move |call: &Call<'_, V>| {
                counter.set(counter.get() + 1);
                Ok(call.scope().int(counter.get()))
            })
            .module("lunarstorm:probe", PROBE_SOURCE);
        if self.mode == Mode::Outside {
            bindings = bindings.module("lunarstorm:other", "export {};");
        }
        if self.mode == Mode::Duplicate {
            bindings = bindings.function("host_fn", COUNTER, |call: &Call<'_, V>| Ok(call.scope().int(0)));
        }
        Ok(bindings)
    }
}

/// What a test's hosts share, across the main Runtime and its workers.
#[derive(Clone)]
struct Shared {
    /// stdout of every Runtime.
    out: Arc<Mutex<String>>,
    /// Uncaught errors of every Runtime.
    errors: Arc<Mutex<Vec<String>>>,
    /// `probe`'s `bindings` calls.
    calls: Arc<AtomicUsize>,
    /// The manifest.
    manifest: Manifest,
    /// `probe`'s behaviour.
    mode: Mode,
    /// The working directory, where worker files live.
    dir: String,
}

/// A host with the extension set `(Probe,)` and workers enabled.
struct TestHost(Shared);

impl Host for TestHost {
    const WORKERS: bool = true;

    /// `probe` is the only extension.
    type Extensions<V: Vm> = (Probe,);

    /// # Returns
    /// A fresh `probe` for one Runtime.
    fn extensions<V: Vm>(&self) -> Self::Extensions<V> {
        (Probe { calls: self.0.calls.clone(), mode: self.0.mode },)
    }

    /// # Returns
    /// The test's manifest.
    fn manifest(&self) -> &Manifest {
        &self.0.manifest
    }

    /// # Returns
    /// A host over the same shared state, built on the worker's thread.
    fn worker_host(&self) -> Option<Box<dyn FnOnce() -> Self + Send>> {
        let shared = self.0.clone();
        Some(Box::new(move || TestHost(shared)))
    }

    /// # Returns
    /// A fixed `process.argv`.
    fn args(&self) -> Vec<String> {
        vec![String::from("extensions")]
    }

    /// # Returns
    /// No variables.
    fn environment(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// # Returns
    /// The test's directory.
    fn working_directory(&self) -> String {
        self.0.dir.clone()
    }

    /// Captures stdout.
    ///
    /// # Arguments
    /// * `stream` - which stream.
    /// * `bytes` - what was written.
    fn write(&self, stream: Stream, bytes: &[u8]) {
        if stream == Stream::Out {
            self.0.out.lock().unwrap().push_str(&String::from_utf8_lossy(bytes));
        }
    }

    /// Fails the test.
    ///
    /// # Arguments
    /// * `code` - the exit status.
    fn exit(&self, code: i32) -> ! {
        panic!("exit {code}; errors: {:?}", self.0.errors.lock().unwrap());
    }

    /// # Returns
    /// `true`.
    fn allow(&self, _access: Access<'_>) -> bool {
        true
    }

    /// Fills the buffer with a known byte.
    ///
    /// # Arguments
    /// * `into` - the buffer.
    fn entropy(&self, into: &mut [u8]) {
        into.fill(7);
    }

    /// Ignores the record.
    ///
    /// # Arguments
    /// * `_record` - the record.
    fn log(&self, _record: Log<'_>) {}

    /// Records the error and goes on.
    ///
    /// # Arguments
    /// * `_in_promise` - unused.
    /// * `description` - the error.
    fn uncaught(&self, _in_promise: bool, description: &str) {
        self.0.errors.lock().unwrap().push(description.to_owned());
    }
}

/// A test's state and its Runtime, not started.
///
/// # Arguments
/// * `name` - the test's directory name.
/// * `manifest` - the manifest's JSON.
/// * `mode` - `probe`'s behaviour.
/// * `files` - `(file name, source)` written into the directory.
///
/// # Returns
/// The shared state and the Runtime.
fn setup(name: &str, manifest: &str, mode: Mode, files: &[(&str, &str)]) -> (Shared, Runtime<QuickJs, TestHost>) {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("extension-tests").join(name);
    std::fs::create_dir_all(&dir).expect("test directory");
    for (file, source) in files {
        std::fs::write(dir.join(file), source).expect("test file");
    }
    let shared = Shared {
        out: Arc::default(),
        errors: Arc::default(),
        calls: Arc::default(),
        manifest: Manifest::parse(manifest).expect("test manifest"),
        mode,
        dir: dir.to_string_lossy().into_owned(),
    };
    let runtime = Runtime::<QuickJs, TestHost>::new(TestHost(shared.clone())).expect("runtime");
    (shared, runtime)
}

/// The manifest that uses `probe`.
const USES_PROBE: &str = r#"{ "extensions": { "probe": {} } }"#;

/// Starts the Runtime, runs `main` to the end and returns stdout.
///
/// # Arguments
/// * `shared` - the test's state.
/// * `runtime` - the Runtime, not started.
/// * `main` - the entry module's source.
///
/// # Returns
/// What the program printed.
fn run(shared: &Shared, mut runtime: Runtime<QuickJs, TestHost>, main: &str) -> String {
    runtime.start().expect("start");
    runtime.load(Source::Text { name: String::from("main.js"), code: main.to_owned() }).expect("load");
    runtime.run();
    assert!(shared.errors.lock().unwrap().is_empty(), "{:?}", shared.errors.lock().unwrap());
    let out = shared.out.lock().unwrap().clone();
    out
}

#[test]
fn used_extension_serves_its_module() {
    let (shared, runtime) = setup("serves", USES_PROBE, Mode::Normal, &[]);
    let out = run(&shared, runtime, "import { value, next } from 'lunarstorm:probe'; console.log(value, next(), next());");
    assert_eq!(out, "42 1 2\n");
}

#[test]
fn unused_extension_is_inert() {
    let (shared, mut runtime) = setup("inert", "{}", Mode::Normal, &[]);
    runtime.start().expect("start");
    let loaded = runtime.load(Source::Text { name: String::from("main.js"), code: String::from("import { value } from 'lunarstorm:probe';") });
    loaded.expect_err("lunarstorm:probe must not resolve");
    // `load` reports the reason through `Host::uncaught` and returns a summary.
    let errors = shared.errors.lock().unwrap().clone();
    assert!(errors.iter().any(|e| e.contains("lunarstorm:probe")), "{errors:?}");
    assert_eq!(shared.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn bindings_run_once_per_runtime() {
    let (shared, runtime) = setup("once", USES_PROBE, Mode::Normal, &[("other.js", "export { next } from 'lunarstorm:probe';")]);
    let out = run(&shared, runtime, "import { next } from 'lunarstorm:probe'; import { next as again } from './other.js'; console.log(next(), again());");
    assert_eq!(out, "1 2\n", "both imports share one instance");
    assert_eq!(shared.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_worker_gets_its_own_instance() {
    let worker = "import { postMessage } from 'lunarstorm:worker'; import { next } from 'lunarstorm:probe'; postMessage({ n: next() });";
    let (shared, runtime) = setup("worker", USES_PROBE, Mode::Normal, &[("worker.js", worker)]);
    let out = run(
        &shared,
        runtime,
        "import { spawn } from 'lunarstorm:worker'; import { next } from 'lunarstorm:probe';
         next(); next();
         const w = spawn('worker.js');
         let got;
         w.onMessage((m) => { got = m.n; });
         await w.exited;
         console.log(next(), got);",
    );
    assert_eq!(out, "3 1\n", "the worker's counter starts at its own 1");
    assert_eq!(shared.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn load_time_error_stops_start() {
    let (_shared, mut runtime) = setup("load-error", r#"{ "extensions": { "nosuch": {} } }"#, Mode::Normal, &[]);
    assert_eq!(runtime.start(), Err(String::from("manifest: this host has no extension nosuch")));
}

#[test]
fn module_outside_namespace_is_refused() {
    let (_shared, mut runtime) = setup("outside", USES_PROBE, Mode::Outside, &[]);
    assert_eq!(runtime.start(), Err(String::from("extension probe: module lunarstorm:other is outside lunarstorm:probe")));
}

#[test]
fn duplicate_binding_is_refused() {
    let (_shared, mut runtime) = setup("duplicate", USES_PROBE, Mode::Duplicate, &[]);
    runtime.register(Bindings::new().function("host_fn", COUNTER, |call: &Call<'_, QuickJs>| Ok(call.scope().int(1))));
    assert_eq!(runtime.start(), Err(String::from("extension probe: ops.host_fn is already bound")));
}

#[test]
fn extension_module_reaches_internal_and_a_program_does_not() {
    let (shared, runtime) = setup("internal", USES_PROBE, Mode::Normal, &[]);
    let out = run(
        &shared,
        runtime,
        "import { value } from 'lunarstorm:probe';
         let refused = false;
         try { await import('lunarstorm:internal'); } catch { refused = true; }
         console.log(value, refused);",
    );
    assert_eq!(out, "42 true\n");
}
