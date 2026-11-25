//! HostApi with three test areas: `alpha` and `beta` each serve their module over
//! a native, `stray` serves a module outside its own path.

use std::sync::{Arc, Mutex};

use lunar_extension_hostapi::{Arg, Area, Function, HostApi};
use lunarstorm_runtime::vm::{Call, Scope, Vm};
use lunarstorm_runtime::{Access, Bindings, Host, Log, Manifest, Runtime, Source, Stream, Use};
use lunarstorm_vm_quickjs::QuickJs;

/// An area whose module `lunarstorm:hostapi/alpha` exports `name()`, `"alpha"`.
struct Alpha;

impl<V: Vm> Area<V> for Alpha {
    const NAME: &'static str = "alpha";

    /// `name()`, described for the spec, with an optional repeated argument to show both flags.
    const FUNCTIONS: &'static [Function] =
        &[Function { name: "name", summary: "The area's name.", args: &[Arg { name: "suffix", optional: true, multiple: true }] }];

    /// # Returns
    /// A native and the module over it.
    fn bindings(&self, _using: &Use<'_>) -> Result<Bindings<V>, String> {
        Ok(Bindings::new()
            .function("alpha_name", |call: &Call<'_, V>| call.scope().string("alpha"))
            .module("lunarstorm:hostapi/alpha", "import { ops } from 'lunarstorm:internal'; export const name = () => ops.alpha_name();"))
    }
}

/// An area whose module `lunarstorm:hostapi/beta` exports `name`, `"beta"`.
struct Beta;

impl<V: Vm> Area<V> for Beta {
    const NAME: &'static str = "beta";

    /// # Returns
    /// The module.
    fn bindings(&self, _using: &Use<'_>) -> Result<Bindings<V>, String> {
        Ok(Bindings::new().module("lunarstorm:hostapi/beta", "export const name = 'beta';"))
    }
}

/// An area that serves `lunarstorm:hostapi/alpha`, which is not its own.
struct Stray;

impl<V: Vm> Area<V> for Stray {
    const NAME: &'static str = "stray";

    /// # Returns
    /// A module outside `lunarstorm:hostapi/stray`.
    fn bindings(&self, _using: &Use<'_>) -> Result<Bindings<V>, String> {
        Ok(Bindings::new().module("lunarstorm:hostapi/alpha", "export {};"))
    }
}

/// A host offering HostApi with `alpha`, `beta` and `stray`.
struct TestHost {
    /// The test's manifest.
    manifest: Manifest,
    /// stdout.
    out: Arc<Mutex<String>>,
    /// Uncaught errors.
    errors: Arc<Mutex<Vec<String>>>,
}

impl Host for TestHost {
    /// HostApi and its three areas.
    type Extensions<V: Vm> = (HostApi<V, (Alpha, Beta, Stray)>,);

    /// # Returns
    /// A fresh HostApi.
    fn extensions<V: Vm>(&self) -> Self::Extensions<V> {
        (HostApi::new((Alpha, Beta, Stray)),)
    }

    /// # Returns
    /// The test's manifest.
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// # Returns
    /// A fixed `process.argv`.
    fn args(&self) -> Vec<String> {
        vec![String::from("hostapi")]
    }

    /// # Returns
    /// No variables.
    fn environment(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// # Returns
    /// The crate's directory.
    fn working_directory(&self) -> String {
        String::from(env!("CARGO_MANIFEST_DIR"))
    }

    /// Captures stdout.
    ///
    /// # Arguments
    /// * `stream` - which stream.
    /// * `bytes` - what was written.
    fn write(&self, stream: Stream, bytes: &[u8]) {
        if stream == Stream::Out {
            self.out.lock().unwrap().push_str(&String::from_utf8_lossy(bytes));
        }
    }

    /// Fails the test.
    ///
    /// # Arguments
    /// * `code` - the exit status.
    fn exit(&self, code: i32) -> ! {
        panic!("exit {code}; errors: {:?}", self.errors.lock().unwrap());
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
        self.errors.lock().unwrap().push(description.to_owned());
    }
}

/// The outcome of one program.
struct Run {
    /// `start`'s error, if it failed.
    start: Option<String>,
    /// stdout.
    out: String,
    /// Uncaught errors.
    errors: Vec<String>,
}

/// Runs `main` under a manifest that uses HostApi with `areas`.
///
/// # Arguments
/// * `areas` - the JSON array of active areas.
/// * `main` - the entry module's source.
///
/// # Returns
/// What happened.
fn run(areas: &str, main: &str) -> Run {
    let manifest = Manifest::parse(&format!(r#"{{ "extensions": {{ "hostapi": {{ "plugins": {areas} }} }} }}"#)).expect("test manifest");
    let (out, errors) = (Arc::<Mutex<String>>::default(), Arc::<Mutex<Vec<String>>>::default());
    let host = TestHost { manifest, out: out.clone(), errors: errors.clone() };
    let mut runtime = Runtime::<QuickJs, TestHost>::new(host).expect("runtime");
    if let Err(error) = runtime.start() {
        return Run { start: Some(error), out: String::new(), errors: Vec::new() };
    }
    if runtime.load(Source::Text { name: String::from("main.js"), code: main.to_owned() }).is_ok() {
        runtime.run();
    }
    let (out, errors) = (out.lock().unwrap().clone(), errors.lock().unwrap().clone());
    Run { start: None, out, errors }
}

#[test]
fn areas_lists_active_areas_in_manifest_order() {
    let run = run(r#"["beta", "alpha"]"#, "import { areas } from 'lunarstorm:hostapi'; console.log(areas.join(), Object.isFrozen(areas));");
    assert_eq!(run.errors, Vec::<String>::new());
    assert_eq!(run.out, "beta,alpha true\n");
}

#[test]
fn inactive_area_does_not_resolve() {
    let run = run(r#"["alpha"]"#, "import { name } from 'lunarstorm:hostapi/beta';");
    assert_eq!(run.out, "");
    assert!(run.errors.iter().any(|e| e.contains("lunarstorm:hostapi/beta")), "{:?}", run.errors);
}

#[test]
fn area_modules_work() {
    let run = run(
        r#"["alpha", "beta"]"#,
        "import { name } from 'lunarstorm:hostapi/alpha'; import { name as beta } from 'lunarstorm:hostapi/beta'; console.log(name(), beta);",
    );
    assert_eq!(run.errors, Vec::<String>::new());
    assert_eq!(run.out, "alpha beta\n");
}

#[test]
fn no_areas_is_valid() {
    let run = run("[]", "import { areas } from 'lunarstorm:hostapi'; console.log(areas.length);");
    assert_eq!(run.errors, Vec::<String>::new());
    assert_eq!(run.out, "0\n");
}

#[test]
fn unknown_area_is_a_load_error() {
    let run = run(r#"["gamma"]"#, "");
    assert_eq!(run.start.as_deref(), Some("manifest: extension hostapi has no plugin gamma"));
}

#[test]
fn an_area_outside_its_path_is_refused() {
    let run = run(r#"["stray"]"#, "");
    assert_eq!(run.start.as_deref(), Some("extension hostapi: area stray: module lunarstorm:hostapi/alpha is outside lunarstorm:hostapi/stray"));
}

#[test]
fn spec_describes_every_offered_area() {
    let spec = HostApi::<QuickJs, _>::new((Alpha, Beta, Stray)).spec();
    assert_eq!(
        spec,
        serde_json::json!({"version": 1, "areas": [
            {"name": "alpha", "module": "lunarstorm:hostapi/alpha", "functions": [
                {"name": "name", "summary": "The area's name.", "args": [{"name": "suffix", "optional": true, "multiple": true}]}
            ]},
            {"name": "beta", "module": "lunarstorm:hostapi/beta", "functions": []},
            {"name": "stray", "module": "lunarstorm:hostapi/stray", "functions": []}
        ]})
    );
}
