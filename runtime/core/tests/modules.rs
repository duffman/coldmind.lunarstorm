//! Module sources on the real engine: TypeScript, JSON and CommonJS files and
//! packages in `node_modules`, loaded by the Runtime from a project on disk, and
//! the same projects compiled into signed `.jbn` packages and run from those.

use lunarstorm_jbn::{Builder, Limits, Package, Signer, TrustStore, Trusted};
use lunarstorm_runtime::{Access, Host, Log, Runtime, Source, State, Stream, Wait};
use lunarstorm_vm_quickjs::QuickJs;
use std::cell::RefCell;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// A host that captures stdout and uncaught errors and refuses paths with `secret`.
#[derive(Default)]
struct TestHost {
    /// Refuse every read: proves a package runs without touching its sources.
    deny_reads: bool,
    /// Everything written to stdout.
    out: RefCell<Vec<u8>>,
    /// Uncaught errors, as `console` would show them.
    uncaught: RefCell<Vec<String>>,
}

impl Host for TestHost {
    /// No extensions.
    type Extensions<V: lunarstorm_runtime::vm::Vm> = ();

    /// # Returns
    /// The empty set.
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
        panic!("exit {code}; uncaught: {:?}", self.uncaught.borrow());
    }

    fn allow(&self, access: Access<'_>) -> bool {
        if self.deny_reads && matches!(access, Access::Read(_)) {
            return false;
        }
        !matches!(access, Access::Read(path) | Access::Write(path) if path.contains("secret"))
    }

    fn entropy(&self, into: &mut [u8]) {
        into.fill(7);
    }

    fn log(&self, _record: Log<'_>) {}

    /// Records the error and keeps the loop going.
    fn uncaught(&self, _in_promise: bool, description: &str) {
        self.uncaught.borrow_mut().push(description.to_owned());
    }
}

/// Writes a project to a temporary directory.
///
/// # Arguments
/// * `files` - project-relative paths with their text.
fn project(files: &[(&str, &str)]) -> TempDir {
    // Scratch lives in the repo, never in the system temp directory.
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.scratch/tests");
    fs::create_dir_all(&scratch).expect("scratch");
    let directory = tempfile::Builder::new().prefix("modules-").tempdir_in(scratch).expect("temp dir");
    for (path, text) in files {
        let path = directory.path().join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }
    directory
}

/// Runs a project's entry file until the loop is idle.
///
/// # Returns
/// Stdout, and the uncaught errors.
fn run(directory: &Path, entry: &str) -> (String, Vec<String>) {
    execute(TestHost::default(), Source::File(directory.join(entry)))
}

/// Runs an entry on a fresh Runtime until the loop is idle.
///
/// # Returns
/// Stdout, and the uncaught errors (a failed load among them).
fn execute(host: TestHost, source: Source) -> (String, Vec<String>) {
    let mut runtime = Runtime::<QuickJs, TestHost>::new(host).expect("event loop");
    runtime.start().expect("start");
    if let Err(e) = runtime.load(source) {
        runtime.host().uncaught.borrow_mut().push(e);
    }
    while runtime.poll(Wait::Block) != State::Idle {}
    let out = String::from_utf8(runtime.host().out.borrow().clone()).expect("utf-8");
    let uncaught = runtime.host().uncaught.borrow().clone();
    (out, uncaught)
}

#[test]
fn typescript_entries_import_typescript_by_their_emitted_names() {
    let dir = project(&[
        (
            "main.ts",
            "import { area, Shape } from './shape.js';\ninterface Point { x: number }\nconst p: Point = { x: 2 };\nconsole.log(area(Shape.Square, p.x));\n",
        ),
        (
            "shape.ts",
            "export enum Shape { Square, Circle }\nexport function area(shape: Shape, size: number): number {\n  return shape === Shape.Square ? size * size : 3 * size * size;\n}\n",
        ),
    ]);
    let (out, uncaught) = run(dir.path(), "main.ts");
    assert_eq!(uncaught, Vec::<String>::new());
    assert_eq!(out, "4\n");
}

#[test]
fn a_typescript_stack_trace_names_the_original_line() {
    let dir = project(&[(
        "main.ts",
        "type Id = string;\nenum Level {\n  Low,\n  High,\n}\ninterface User { id: Id }\nfunction fail(user: User): never {\n  throw new Error(user.id + Level.High);\n}\ntry { fail({ id: 'u' }); } catch (e) { console.log((e as Error).stack); }\n",
    )]);
    let (out, uncaught) = run(dir.path(), "main.ts");
    assert_eq!(uncaught, Vec::<String>::new());
    assert!(out.contains("main.ts:8"), "the throw is on line 8: {out}");
}

#[test]
fn commonjs_packages_require_files_json_and_builtins() {
    let dir = project(&[
        ("main.mjs", "import lib, { greet } from 'cjs-lib';\nconsole.log(greet('x'), lib.version, lib.hasFs, lib.missing());\n"),
        ("node_modules/cjs-lib/package.json", r#"{"name":"cjs-lib","main":"lib/index"}"#),
        (
            "node_modules/cjs-lib/lib/index.js",
            "const helper = require('./helper');\nconst data = require('../data.json');\nconst fs = require('fs');\nexports.greet = (n) => helper.prefix + n;\nexports.version = data.version;\nexports.hasFs = typeof fs.readFileSync === 'function';\nexports.missing = () => { try { require('nope'); } catch (e) { return e.code; } };\n",
        ),
        ("node_modules/cjs-lib/lib/helper.js", "module.exports = { prefix: 'hi ' };\n"),
        ("node_modules/cjs-lib/data.json", r#"{"version":"1.2.3"}"#),
    ]);
    let (out, uncaught) = run(dir.path(), "main.mjs");
    assert_eq!(uncaught, Vec::<String>::new());
    assert_eq!(out, "hi x 1.2.3 true MODULE_NOT_FOUND\n");
}

#[test]
fn esm_packages_map_exports_conditions_patterns_and_private_imports() {
    let dir = project(&[
        ("package.json", r##"{"name":"app","type":"module","imports":{"#config":"./config.ts"}}"##),
        (
            "main.js",
            "import { kind } from '@scope/esm';\nimport { feature } from '@scope/esm/feature/a';\nimport config from '#config';\nimport { createRequire } from './require.cjs';\nconsole.log(kind, feature, config.name, createRequire());\n",
        ),
        ("config.ts", "const config: { name: string } = { name: 'cfg' };\nexport default config;\n"),
        ("require.cjs", "exports.createRequire = () => require('@scope/esm').kind;\n"),
        (
            "node_modules/@scope/esm/package.json",
            r#"{"name":"@scope/esm","type":"module","exports":{".":{"types":"./index.d.ts","import":"./dist/index.js","require":"./dist/index.cjs"},"./feature/*":"./dist/features/*.js"}}"#,
        ),
        ("node_modules/@scope/esm/dist/index.js", "export const kind = 'esm';\n"),
        ("node_modules/@scope/esm/dist/index.cjs", "exports.kind = 'cjs';\n"),
        ("node_modules/@scope/esm/dist/features/a.js", "export const feature = 'a';\n"),
    ]);
    let (out, uncaught) = run(dir.path(), "main.js");
    assert_eq!(uncaught, Vec::<String>::new());
    assert_eq!(out, "esm a cfg cjs\n");
}

#[test]
fn package_reads_go_through_the_host_policy() {
    let dir = project(&[
        ("main.mjs", "import x from 'secret-lib';\nconsole.log(x);\n"),
        ("node_modules/secret-lib/package.json", r#"{"name":"secret-lib","main":"index.js"}"#),
        ("node_modules/secret-lib/index.js", "module.exports = 1;\n"),
    ]);
    let (out, uncaught) = run(dir.path(), "main.mjs");
    assert_eq!(out, "");
    assert!(uncaught.iter().any(|e| e.contains("permission denied")), "{uncaught:?}");
}

/// Defect: QuickJS formats a thrown message into a 256-byte buffer, so a resolve
/// error naming two long paths lost its end, and with it the reason ("permission
/// denied by host policy"). The paths here make the message longer than that on
/// any checkout.
#[test]
fn a_long_resolve_error_keeps_its_reason() {
    let deep = "d".repeat(150);
    let (main, manifest, index) = (format!("{deep}/main.mjs"), format!("{deep}/node_modules/secret-lib/package.json"), format!("{deep}/node_modules/secret-lib/index.js"));
    let dir = project(&[
        (&main, "import x from 'secret-lib';\nconsole.log(x);\n"),
        (&manifest, r#"{"name":"secret-lib","main":"index.js"}"#),
        (&index, "module.exports = 1;\n"),
    ]);
    let (out, uncaught) = run(dir.path(), &main);
    assert_eq!(out, "");
    assert!(uncaught.iter().any(|e| e.len() > 256 && e.contains("permission denied by host policy")), "{uncaught:?}");
}

/// A signing key and a trust store that trusts it.
fn keys() -> (Signer, TrustStore) {
    let signer = Signer::generate().expect("key");
    let trust = TrustStore { signers: vec![Trusted { public_key: signer.public_key(), packages: Vec::new(), minimum_release: 0 }] };
    (signer, trust)
}

/// Compiles a project's entry into a package on a fresh Runtime.
///
/// # Returns
/// The package bytes.
fn pack(directory: &Path, entry: &str, signer: &Signer) -> Result<Vec<u8>, String> {
    let mut runtime = Runtime::<QuickJs, TestHost>::new(TestHost::default()).expect("event loop");
    runtime.start().expect("start");
    let builder = runtime.package(&directory.join(entry), "app", "1.0.0", 1)?;
    builder.build(signer).map_err(|e| e.to_string())
}

/// A project that uses every kind of module the loader knows.
const MIXED: &[(&str, &str)] = &[
    (
        "main.ts",
        "import { area } from './shape.js';\nimport cfg from 'cfg';\nimport { join } from 'node:path';\nconst late = await import('./late.js');\nconsole.log(area(3), cfg.name, cfg.version, join('a', 'b'), late.word, import.meta.main);\nfunction fail(): never {\n  throw new Error('packaged');\n}\ntry { fail(); } catch (e) { console.log((e as Error).stack.includes('main.ts:7')); }\n",
    ),
    ("shape.ts", "export const area = (size: number): number => size * size;\n"),
    ("late.js", "export const word = 'late';\n"),
    ("node_modules/cfg/package.json", r#"{ "name": "cfg", "main": "index.js" }"#),
    ("node_modules/cfg/index.js", "const meta = require('./meta.json');\nmodule.exports = { name: meta.name, version: require('path').basename('/x/' + meta.version) };\n"),
    ("node_modules/cfg/meta.json", r#"{ "name": "cfg", "version": "1.2.3" }"#),
];

#[test]
fn a_package_runs_like_its_sources_without_reading_a_file() {
    let dir = project(MIXED);
    let (expected, uncaught) = run(dir.path(), "main.ts");
    assert_eq!(uncaught, Vec::<String>::new());
    assert_eq!(expected, "9 cfg 1.2.3 a/b late true\ntrue\n");

    let (signer, trust) = keys();
    let bytes = pack(dir.path(), "main.ts", &signer).expect("packs");
    let package = Package::open(&bytes, &trust, &Limits::default()).expect("verifies");
    // main, shape, late, the CommonJS body and facade of cfg, and meta.json.
    assert_eq!(package.manifest().modules.len(), 6, "{:?}", package.manifest().modules);

    let host = TestHost { deny_reads: true, ..TestHost::default() };
    let (out, uncaught) = execute(host, Source::Package(package));
    assert_eq!(uncaught, Vec::<String>::new());
    assert_eq!(out, expected);
}

#[test]
fn a_package_is_a_closed_world_for_one_engine() {
    let dir = project(&[
        ("main.js", "const name = './extra.js';\nimport(name).then(() => console.log('loaded'), (e) => console.log(String(e.message)));\nimport('node:path').then((p) => console.log(p.sep));\n"),
        ("extra.js", "export {};\n"),
    ]);
    let (signer, trust) = keys();
    let bytes = pack(dir.path(), "main.js", &signer).expect("packs");
    let package = Package::open(&bytes, &trust, &Limits::default()).expect("verifies");
    let (out, uncaught) = execute(TestHost::default(), Source::Package(package.clone()));
    assert_eq!(uncaught, Vec::<String>::new());
    assert!(out.contains("./extra.js is not in the package"), "{out}");
    assert!(out.contains("/\n"), "builtins stay reachable: {out}");

    // The same bytecode recorded for another engine is refused before it runs.
    let entry = package.manifest().entry.clone();
    let mut other = Builder::new("app", "1.0.0", 1, "v8 v8 13 contract 1", &entry);
    other.module(&entry, package.bytecode(&entry).expect("entry").to_vec(), Vec::new());
    let foreign = Package::open(&other.build(&signer).expect("builds"), &trust, &Limits::default()).expect("verifies");
    let (_, uncaught) = execute(TestHost::default(), Source::Package(foreign));
    assert!(uncaught.iter().any(|e| e.contains("is built for v8")), "{uncaught:?}");
}

#[test]
fn packaging_reports_what_it_cannot_resolve_or_compile() {
    let (signer, _) = keys();
    let missing = project(&[("main.js", "import './gone.js';\n")]);
    let error = pack(missing.path(), "main.js", &signer).unwrap_err();
    assert!(error.contains("cannot resolve ./gone.js"), "{error}");
    let broken = project(&[("main.js", "import './bad.js';\n"), ("bad.js", "export const = 1;\n")]);
    let error = pack(broken.path(), "main.js", &signer).unwrap_err();
    assert!(error.contains("bad.js"), "{error}");
}
