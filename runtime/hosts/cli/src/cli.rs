//! The `run` command: runs a script, or code given with `-e`, on a Runtime this host
//! owns.
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

use crate::host::{self, CliHost};
use lunarstorm_cli_contract::{Arg, Command, Failure, Invocation, Package, Section};
use lunarstorm_runtime::vm::Vm;
use lunarstorm_runtime::{Runtime, Source};
use lunarstorm_vm_quickjs::QuickJs;
use std::path::PathBuf;

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
            .arg(Arg::rest("script").placeholder("<file.js> [args...]").about("the script, then its own arguments (process.argv)"))
            .example("lunarstorm server.js --port 8080")
            .example("lunarstorm run -e 'console.log(process.version)'")
            .run(run)]
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
