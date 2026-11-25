//! The frontend commands of the `lunarstorm` command line.
//!
//! ```text
//! lunarstorm serve [root] [--host H] [--port P] [--define K=V]... [--no-watch]
//! lunarstorm bundle [root] [-o DIR] [--no-minify]
//! lunarstorm preview [root] [-o DIR] [--port P]
//! lunarstorm transpile <file|-> [-o FILE] [--sourcemap] [--loader ts|tsx|js|jsx]
//! ```
//!
//! The host parses, checks value types and choices, and renders help and usage errors.
//! The handlers check only what a type cannot say (the port range, `KEY=VALUE`, `--loader`
//! only with stdin) and return those as usage errors too, so every bad command line exits 2
//! and points at `--help`. Explicit options override the project's native config; options
//! left out are not set, so the config (or its defaults) applies.

use std::path::{Path, PathBuf};

use devtools_bundler::BuildOptions;
use devtools_server::Config;
use lunarstorm_cli_contract::{Arg, Command, Failure, Invocation, Package, ValueType};

use crate::{execute, transpile, Task};

/// The frontend tools' package.
struct FrontendPackage;

/// Returns the frontend tools' package for mounting on the command line.
///
/// # Returns
/// The package; it exports `serve`, `bundle`, `preview` and `transpile`.
pub fn package() -> Box<dyn Package> {
    Box::new(FrontendPackage)
}

impl Package for FrontendPackage {
    /// # Returns
    /// `"frontend"`.
    fn id(&self) -> &'static str {
        "frontend"
    }

    /// # Returns
    /// The four commands.
    fn commands(&self) -> Vec<Command> {
        let root = || {
            Arg::positional("root")
                .placeholder("<root>")
                .value(ValueType::Path)
                .about("the app root, holding index.html [default: .]")
        };
        let out = |about: &str| Arg::option("out").short('o').placeholder("<dir>").value(ValueType::Path).about(about);
        let port = |about: &str| Arg::option("port").placeholder("<port>").value(ValueType::Int).about(about);
        vec![
            Command::new("serve", "serve source modules, watch them and send hot updates")
                .arg(root())
                .arg(Arg::option("host").placeholder("<host>").about("bind address [default: 127.0.0.1]"))
                .arg(port("port; 0 selects a free one [default: 5173]"))
                .arg(Arg::option("define").placeholder("<key=value>").about("compile-time JS expression; may be repeated"))
                .arg(Arg::flag("no-watch").about("do not watch files"))
                .example("lunar serve ./app --port 5173")
                .run(serve),
            Command::new("bundle", "bundle an HTML frontend into production JavaScript, CSS and assets")
                .arg(root())
                .arg(out("output directory, replaced on each bundle [default: config, then dist]"))
                .arg(Arg::flag("no-minify").about("do not minify"))
                .example("lunar bundle ./app --out dist")
                .run(bundle),
            Command::new("preview", "serve an existing bundle on loopback, including SPA routes; does not bundle first")
                .arg(root())
                .arg(out("the bundle directory [default: config, then dist]"))
                .arg(port("port; 0 selects a free one").default("4173"))
                .example("lunar preview ./app --port 4173")
                .run(preview),
            Command::new("transpile", "transpile one TS/TSX/JS/JSX module to JavaScript; no bundling, no type-checking")
                .arg(Arg::positional("input").placeholder("<file|->").value(ValueType::Path).about("the module, or - for stdin").required())
                .arg(Arg::option("out").short('o').placeholder("<file>").value(ValueType::Path).about("write JavaScript here [default: stdout; - also means stdout]"))
                .arg(Arg::flag("sourcemap").about("external map with --out, inline map on stdout"))
                .arg(Arg::option("loader").placeholder("<loader>").choices(&["ts", "tsx", "js", "jsx"]).about("syntax of stdin input [default: ts]"))
                .example("lunar transpile src/main.ts")
                .example("lunar transpile src/App.tsx --out dist/App.js --sourcemap")
                .example("cat src/main.ts | lunarstorm transpile - --loader ts")
                .run(transpile_command),
        ]
    }
}

/// The `serve` handler.
///
/// # Arguments
/// * `invocation` - `root`, `host`, `port`, `define`, `no-watch`.
///
/// # Returns
/// 0 after Ctrl-C.
///
/// # Errors
/// A usage error for a port out of range or a malformed `--define`; a failure when the
/// server cannot start.
fn serve(invocation: &mut Invocation) -> Result<i32, Failure> {
    let values = &invocation.values;
    let mut config = Config::new(root(invocation));
    if let Some(port) = port(invocation)? {
        config.set_port(port);
    }
    if let Some(host) = values.text("host") {
        config.set_host(host.to_owned());
    }
    for pair in values.all("define") {
        let (key, expression) = pair.split_once('=').ok_or_else(|| Failure::usage("--define needs KEY=VALUE"))?;
        if key.is_empty() || expression.is_empty() {
            return Err(Failure::usage("--define needs a non-empty key and expression"));
        }
        config.define.push((key.to_owned(), expression.to_owned()));
    }
    config.watch = !values.flag("no-watch");
    finish(Task::Serve(config))
}

/// The `bundle` handler.
///
/// # Arguments
/// * `invocation` - `root`, `out`, `no-minify`.
///
/// # Returns
/// 0 once bundled.
///
/// # Errors
/// A failure with the bundler's diagnostics.
fn bundle(invocation: &mut Invocation) -> Result<i32, Failure> {
    let minify = invocation.values.flag("no-minify").then_some(false);
    let options = BuildOptions { root: root(invocation), out: invocation.values.path("out"), minify, production: true };
    finish(Task::Bundle(options))
}

/// The `preview` handler.
///
/// # Arguments
/// * `invocation` - `root`, `out`, `port`.
///
/// # Returns
/// 0 after Ctrl-C.
///
/// # Errors
/// A usage error for a port out of range; a failure when the bundle cannot be served.
fn preview(invocation: &mut Invocation) -> Result<i32, Failure> {
    let port = port(invocation)?.unwrap_or(4173);
    finish(Task::Preview { root: root(invocation), out: invocation.values.path("out"), port })
}

/// The `transpile` handler.
///
/// # Arguments
/// * `invocation` - `input`, `out`, `sourcemap`, `loader`.
///
/// # Returns
/// 0 once written.
///
/// # Errors
/// A usage error for `--loader` with a file input; a failure for unreadable or invalid
/// source.
fn transpile_command(invocation: &mut Invocation) -> Result<i32, Failure> {
    let values = &invocation.values;
    let input = values.path("input").unwrap_or_else(|| PathBuf::from("-"));
    let loader = values.text("loader").map(str::to_owned);
    // A file's syntax comes from its extension; a loader would silently disagree with it.
    if loader.is_some() && input != Path::new("-") {
        return Err(Failure::usage("--loader is only supported with stdin input '-'; file syntax comes from its extension"));
    }
    let options = transpile::Options { input, out: values.path("out"), sourcemap: values.flag("sourcemap"), loader };
    finish(Task::Transpile(options))
}

/// The app root argument.
///
/// # Arguments
/// * `invocation` - the invocation.
///
/// # Returns
/// `root`, or the current directory.
fn root(invocation: &Invocation) -> PathBuf {
    invocation.values.path("root").unwrap_or_else(|| PathBuf::from("."))
}

/// The `--port` argument, checked against the port range.
///
/// # Arguments
/// * `invocation` - the invocation.
///
/// # Returns
/// The port, or `None` when not given.
///
/// # Errors
/// A usage error outside 0..=65535.
fn port(invocation: &Invocation) -> Result<Option<u16>, Failure> {
    invocation
        .values
        .int("port")
        .map(|port| u16::try_from(port).map_err(|_| Failure::usage("--port must be an integer from 0 to 65535")))
        .transpose()
}

/// Runs a task and maps its outcome to an exit status.
///
/// # Arguments
/// * `task` - the task.
///
/// # Returns
/// 0 on success.
///
/// # Errors
/// [`Failure::Failed`] with the error's context chain, status 1.
fn finish(task: Task) -> Result<i32, Failure> {
    execute(task).map(|()| 0).map_err(|error| Failure::failed(format!("{error:#}")))
}
