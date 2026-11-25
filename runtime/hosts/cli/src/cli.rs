//! The `run` command: runs a script, a `.jbn` package, or code given with `-e`, on
//! a Runtime this host owns; and `jbn key` / `jbn build`, which make signing keys
//! and compile an entry into a signed `.jbn` package.
//!
//! The command line itself (parsing, help, usage errors, `--color`, `--version`) is
//! the host's (`lunarstorm-cli-host`); this module says what `run` accepts and does.
//! The former hand-written parser is archived in
//! `.archive/2026-11-24/runtime/hosts/cli/src/cli.rs`.
//!
//! Decisions:
//! - This is the one place the engine is chosen: `Runtime<QuickJs, CliHost>`.
//!   Everything below it is generic and bound here, at compile time.
//! - `run` is the default command, so `lunarstorm app.js` and `lunarstorm -e code`
//!   work as they did before the command line had other commands.
//! - Options are read only up to the script name, and `-e <code>` ends them too;
//!   everything after belongs to the script, as in Node (`options_first`,
//!   `ends_options`).
//! - A `.jbn` script runs only when a trust file (`--trust`, or `LUNARSTORM_TRUST`)
//!   names its signer; there is no unsigned or untrusted mode.

use crate::host::{self, CliHost};
use lunarstorm_cli_contract::{Arg, Command, Failure, Invocation, Package, Section};
use lunarstorm_runtime::vm::Vm;
use lunarstorm_runtime::{Runtime, Source};
use lunarstorm_vm_quickjs::QuickJs;
use std::path::{Path, PathBuf};

/// The name scripts see as `process.argv[0]`.
const ARGV0: &str = "lunarstorm";

/// The runtime's package.
struct RuntimePackage;

/// Returns the runtime's package for mounting on the command line.
///
/// # Returns
/// The package; it exports `run`.
pub fn package() -> Box<dyn Package> {
    Box::new(RuntimePackage)
}

/// Describes the engine and target, for the version line on a terminal.
///
/// # Returns
/// e.g. `quickjs-ng · aarch64-macos`.
pub fn engine() -> String {
    format!("{} · {}-{}", QuickJs::descriptor().engine, std::env::consts::ARCH, std::env::consts::OS)
}

impl Package for RuntimePackage {
    /// # Returns
    /// `"runtime"`.
    fn id(&self) -> &'static str {
        "runtime"
    }

    /// # Returns
    /// The `run` command.
    fn commands(&self) -> Vec<Command> {
        vec![Command::new("run", "run a script, or code given with -e, on the Lunarstorm runtime")
            .default_command()
            .options_first()
            .arg(Arg::option("eval").short('e').placeholder("<code>").about("evaluate code as an ES module").ends_options())
            .arg(Arg::option("trust").placeholder("<trust.json>").env("LUNARSTORM_TRUST").about("signers a .jbn package may come from"))
            .arg(Arg::rest("script").placeholder("<file.js> [args...]").about("the script, then its own arguments (process.argv)"))
            .example("lunarstorm server.js --port 8080")
            .example("lunarstorm run -e 'console.log(process.version)'")
            .example("lunarstorm run --trust trust.json app.jbn")
            .run(run),
        Command::new("debug", "run a script under the debugger: Debug Adapter Protocol on stdin/stdout")
            .options_first()
            .arg(Arg::rest("script").placeholder("<file.js> [args...]").about("the script, then its own arguments (process.argv)"))
            .example("lunarstorm debug server.js --port 8080")
            .run(debug),
        Command::new("jbn", "signed .jbn packages: compiled modules, run with `run --trust`")
            .child(
                Command::new("key", "write a new Ed25519 signing key and print its trust file")
                    .arg(Arg::positional("file").placeholder("<key.der>").required().about("where to write the key (PKCS#8 DER); never overwritten"))
                    .example("lunarstorm jbn key signing.der > trust.json")
                    .run(key),
            )
            .child(
                Command::new("build", "compile an entry and every module it imports into a signed package")
                    .arg(Arg::positional("entry").placeholder("<entry>").required().about("the entry file (.js, .ts, .mjs, .cjs)"))
                    .arg(Arg::option("output").short('o').placeholder("<file.jbn>").required().about("where to write the package"))
                    .arg(Arg::option("key").placeholder("<key.der>").required().about("signing key from `jbn key`"))
                    .arg(Arg::option("name").placeholder("<name>").about("package name (default: the entry's directory name)"))
                    .arg(Arg::option("package-version").placeholder("<version>").default("0.0.0").about("package version (`--version` is the host's)"))
                    .arg(Arg::option("release").placeholder("<n>").default("1").about("release sequence number"))
                    .example("lunarstorm jbn build src/main.ts -o app.jbn --key signing.der")
                    .run(build),
            )]
    }

    /// # Returns
    /// The modules scripts can import, and examples.
    fn sections(&self) -> Vec<Section> {
        let rows = |rows: &[(&str, &str)]| rows.iter().map(|(left, right)| ((*left).to_owned(), (*right).to_owned())).collect();
        vec![
            (
                "MODULES".to_owned(),
                rows(&[
                    ("node:*", "events net http fs fs/promises path process buffer timers"),
                    ("lunarstorm:ui", "style, progress bars and spinners for the terminal"),
                    ("lunarstorm:hostapi", "the host's API areas; lunarstorm:hostapi/ui is lunarstorm:ui"),
                ]),
            ),
            (
                "EXAMPLES".to_owned(),
                rows(&[("lunarstorm server.js --port 8080", ""), ("lunarstorm -e 'console.log(process.version)'", "")]),
            ),
        ]
    }
}

/// The `run` handler.
///
/// # Arguments
/// * `invocation` - `eval` (the code, when given) and `script` (with `-e`: the
///   script's arguments; without: the script path, then its arguments).
///
/// # Returns
/// The exit status of [`execute`].
///
/// # Errors
/// A usage error when neither a script nor `-e` was given.
fn run(invocation: &mut Invocation) -> Result<i32, Failure> {
    let words = invocation.values.all("script");
    let (source, name, args) = match invocation.values.text("eval") {
        Some(code) => (Source::Text { name: String::from("[eval]"), code: code.to_owned() }, String::from("[eval]"), words),
        None => match words.split_first() {
            Some((script, args)) if script.ends_with(".jbn") => {
                let trust = invocation.values.path("trust").ok_or_else(|| Failure::usage("a .jbn package needs --trust <trust.json>"))?;
                (Source::Package(open(Path::new(script), &trust)?), script.clone(), args)
            }
            Some((script, args)) => (Source::File(PathBuf::from(script)), script.clone(), args),
            None => return Err(Failure::usage("run needs <file.js> or -e <code>")),
        },
    };
    // `process.argv` mirrors Node (`[runtime, script, ...args]`), so scripts written
    // for Node read their arguments from index 2 unchanged.
    let mut argv = vec![String::from(ARGV0), name];
    argv.extend(args.iter().cloned());
    Ok(execute(source, argv))
}

/// Runs a script: creates the Runtime this host owns, registers the terminal
/// bindings, and drives it to completion.
///
/// # Arguments
/// * `source` - the entry module.
/// * `argv` - `process.argv`.
///
/// # Returns
/// The exit status: `process.exitCode` or 0; 1 when the script cannot be read;
/// 70 when the Runtime cannot start. Uncaught errors and `process.exit()` end the
/// process through the host and do not return.
fn execute(source: Source, argv: Vec<String>) -> i32 {
    let mut runtime = match Runtime::<QuickJs, CliHost>::new(CliHost::new(argv)) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("lunarstorm: cannot create the event loop: {error}");
            return 70;
        }
    };
    runtime.register(host::bindings());
    if let Err(message) = runtime.start() {
        eprintln!("lunarstorm: bootstrap failed: {message}");
        return 70;
    }
    if let Err(message) = runtime.load(source) {
        eprintln!("lunarstorm: {message}");
        return 1;
    }
    runtime.run()
}

/// The `debug` handler: runs the script on its own thread with a debugger attached,
/// and speaks the Debug Adapter Protocol on stdin/stdout (jetbunny's
/// `debug::host::run`). A DAP client (an editor) starts `lunarstorm debug` as its
/// debug adapter. Script stdout goes to stderr.
///
/// # Errors
/// A usage error without a script; a failure when the protocol or the script
/// cannot start.
fn debug(invocation: &mut Invocation) -> Result<i32, Failure> {
    use lunarstorm_runtime::vm::lunarstorm_debug::debug::{host::OwnerSession, DebugSessionOptions};
    use lunarstorm_runtime::vm::lunarstorm_debug::vm::VmHandle;
    use lunarstorm_runtime::{State, Wait};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let words = invocation.values.all("script");
    let (script, args) = words.split_first().ok_or_else(|| Failure::usage("debug needs <file.js>"))?;
    let mut argv = vec![String::from(ARGV0), script.clone()];
    argv.extend(args.iter().cloned());
    let path = PathBuf::from(script);
    let code = Arc::new(std::sync::atomic::AtomicI32::new(0));
    let exit = code.clone();
    lunarstorm_runtime::vm::lunarstorm_debug::debug::host::run(move |ready| {
        let mut runtime = Runtime::<QuickJs, CliHost>::new(CliHost::for_debugger(argv)).map_err(|e| format!("cannot create the event loop: {e}"))?;
        runtime.register(host::bindings());
        runtime.start().map_err(|e| format!("bootstrap failed: {e}"))?;
        // Decision: always held at entry. jetbunny's DAP adapter requires it: the
        // script starts at the client's `configurationDone`, after its breakpoints
        // are set. (A `--break` flag that turned it off was dropped: the adapter
        // refuses such a session.)
        let debugger = runtime.debug(DebugSessionOptions { break_on_start: true })?;
        // The client's stop: end running JavaScript, then the loop at its next step.
        let stopped = Arc::new(AtomicBool::new(false));
        let interrupt = runtime.interrupt().ok_or("the runtime is not started")?;
        let poster = runtime.poster();
        let flag = stopped.clone();
        let stop = VmHandle::new(1, move || {
            flag.store(true, Ordering::Release);
            interrupt.request();
            poster.post(|_| {});
            Ok(())
        });
        ready.send(OwnerSession { debugger, stop, targets: None }).map_err(|_| String::from("the debug adapter is gone"))?;
        runtime.load(Source::File(path))?;
        while !stopped.load(Ordering::Acquire) && runtime.poll(Wait::Block) == State::Running {}
        exit.store(runtime.stop(), Ordering::Release);
        Ok(())
    })
    .map_err(Failure::failed)?;
    Ok(code.load(std::sync::atomic::Ordering::Acquire))
}

/// Reads and verifies a package against a trust file.
///
/// # Errors
/// A failure naming the file that cannot be read or the check that refused it.
fn open(package: &Path, trust: &Path) -> Result<lunarstorm_jbn::Package, Failure> {
    let read = |path: &Path| std::fs::read(path).map_err(|e| Failure::failed(format!("cannot read {}: {e}", path.display())));
    let store = lunarstorm_jbn::TrustStore::from_json(&read(trust)?).map_err(|e| Failure::failed(e.to_string()))?;
    lunarstorm_jbn::Package::open(&read(package)?, &store, &lunarstorm_jbn::Limits::default())
        .map_err(|e| Failure::failed(format!("{}: {e}", package.display())))
}

/// The `jbn key` handler: writes a new key with owner-only permissions and prints a
/// trust file that trusts it for every package.
///
/// # Errors
/// The file exists or cannot be written.
fn key(invocation: &mut Invocation) -> Result<i32, Failure> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let file = invocation.values.path("file").ok_or_else(|| Failure::usage("jbn key needs <key.der>"))?;
    let signer = lunarstorm_jbn::Signer::generate().map_err(|e| Failure::failed(e.to_string()))?;
    let der = signer.to_pkcs8_der().map_err(|e| Failure::failed(e.to_string()))?;
    let cannot = |e: std::io::Error| Failure::failed(format!("cannot write {}: {e}", file.display()));
    // Decision: never overwrite; losing a signing key is not recoverable.
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&file).map_err(cannot)?;
    out.write_all(&der).map_err(cannot)?;
    let trusted = lunarstorm_jbn::Trusted { public_key: signer.public_key(), packages: Vec::new(), minimum_release: 0 };
    println!("{}", lunarstorm_jbn::TrustStore { signers: vec![trusted] }.to_json());
    Ok(0)
}

/// The `jbn build` handler: compiles on the same Runtime and host modules `run`
/// uses, so a package links exactly as its sources would.
///
/// # Errors
/// Unreadable key or entry, a module that does not resolve or compile, or an
/// unwritable output.
fn build(invocation: &mut Invocation) -> Result<i32, Failure> {
    let values = &invocation.values;
    let (entry, output, key) = match (values.path("entry"), values.path("output"), values.path("key")) {
        (Some(entry), Some(output), Some(key)) => (entry, output, key),
        _ => return Err(Failure::usage("jbn build needs <entry>, -o <file.jbn> and --key <key.der>")),
    };
    let der = std::fs::read(&key).map_err(|e| Failure::failed(format!("cannot read {}: {e}", key.display())))?;
    let signer = lunarstorm_jbn::Signer::from_pkcs8_der(&der).map_err(|e| Failure::failed(e.to_string()))?;
    let directory = std::fs::canonicalize(&entry).ok().and_then(|p| p.parent()?.file_name().map(|n| n.to_string_lossy().into_owned()));
    let name = values.text("name").map(str::to_owned).or(directory).unwrap_or_else(|| String::from("app"));
    let version = values.text("package-version").unwrap_or("0.0.0").to_owned();
    let release = values.int("release").and_then(|n| u64::try_from(n).ok()).ok_or_else(|| Failure::usage("--release must be a non-negative number"))?;

    let mut runtime = Runtime::<QuickJs, CliHost>::new(CliHost::new(vec![String::from(ARGV0)]))
        .map_err(|e| Failure::failed(format!("cannot create the event loop: {e}")))?;
    runtime.register(host::bindings());
    runtime.start().map_err(|e| Failure::failed(format!("bootstrap failed: {e}")))?;
    let builder = runtime.package(&entry, &name, &version, release).map_err(Failure::failed)?;
    let bytes = builder.build(&signer).map_err(|e| Failure::failed(e.to_string()))?;
    std::fs::write(&output, &bytes).map_err(|e| Failure::failed(format!("cannot write {}: {e}", output.display())))?;
    eprintln!("{}: {name} {version}, {} bytes", output.display(), bytes.len());
    Ok(0)
}
