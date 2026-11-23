//! The `lunarstorm` command line: its options, the content of its screens, and
//! the life of the Runtime it owns.
//!
//! How the screens look (colors, layout, error frames, progress bars) is the
//! `lunarstorm-cliui` package in `cliui/`; this module only decides what the
//! runtime's own command line accepts and says.
//!
//! Decisions:
//! - This is the one place the engine is chosen: `Runtime<QuickJs, CliHost>`.
//!   Everything below it is generic and bound here, at compile time.
//! - Options are read only up to the script name; everything after it belongs to
//!   the script, as in Node.
//! - Output that programs parse stays byte-stable when piped: `--version` prints
//!   exactly `lunarstorm <version>` and nothing contains escape sequences.
//! - Usage errors exit with status 2 and suggest the nearest option.

use crate::host::{self, CliHost};
use lunarstorm_cliui::screen::{self, Brand, Section};
use lunarstorm_cliui::term::{self, ColorChoice, Depth, Pen};
use lunarstorm_runtime::vm::Vm;
use lunarstorm_runtime::{Runtime, Source};
use lunarstorm_vm_quickjs::QuickJs;
use std::path::PathBuf;

/// The runtime's identity on its screens.
const BRAND: Brand = Brand {
    mark: "☾ lunarstorm",
    version: env!("CARGO_PKG_VERSION"),
    tagline: "Node.js-inspired JavaScript runtime on QuickJS",
};
/// Long options, the candidates for "did you mean".
const OPTIONS: [&str; 4] = ["--eval", "--color", "--version", "--help"];
/// The help screen's content.
const SECTIONS: [Section; 5] = [
    ("USAGE", &[("lunarstorm [options] <file.js> [args...]", ""), ("lunarstorm [options] -e <code> [args...]", "")]),
    (
        "OPTIONS",
        &[
            ("-e, --eval <code>", "evaluate code as an ES module"),
            ("    --color <when>", "color output: auto, always or never"),
            ("-v, --version", "print the version"),
            ("-h, --help", "print this help"),
        ],
    ),
    (
        "MODULES",
        &[
            ("node:*", "events net http fs fs/promises path process buffer timers"),
            ("lunarstorm:ui", "style, progress bars and spinners for the terminal"),
        ],
    ),
    ("ENVIRONMENT", &[("NO_COLOR", "disable color"), ("FORCE_COLOR", "0 off, 1 basic, 2 256 colors, 3 truecolor")]),
    ("EXAMPLES", &[("lunarstorm server.js --port 8080", ""), ("lunarstorm -e 'console.log(process.version)'", "")]),
];

/// What the command line asks for.
enum Action {
    /// Print the help screen to stdout.
    Help,
    /// Print the version to stdout.
    Version,
    /// No script was given: print the help screen to stderr, exit 2.
    Missing,
    /// Run a script with this `process.argv`.
    Run(Source, Vec<String>),
}

/// A command line that cannot be acted on.
struct UsageError {
    /// What is wrong.
    message: String,
    /// The suggested fix, if one is close enough.
    hint: Option<String>,
}

/// Runs the command line of the current process.
///
/// # Returns
/// The process exit status.
pub fn run() -> i32 {
    let mut color = ColorChoice::Auto;
    let parsed = parse(std::env::args().skip(1), &mut color);
    term::init(color);
    let code = match parsed {
        Ok(Action::Help) => {
            print!("{}", screen::help(Pen(term::depth(1)), &BRAND, &SECTIONS));
            0
        }
        Ok(Action::Version) => {
            println!("{}", version(Pen(term::depth(1))));
            0
        }
        Ok(Action::Missing) => {
            eprint!("{}", screen::help(Pen(term::depth(2)), &BRAND, &SECTIONS));
            2
        }
        Ok(Action::Run(source, argv)) => execute(source, argv),
        Err(error) => {
            let pen = Pen(term::depth(2));
            eprint!("{}", screen::usage_error(pen, &error.message, error.hint.as_deref(), "lunarstorm --help"));
            2
        }
    };
    term::restore();
    code
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

/// Parses the options and the script name.
///
/// # Arguments
/// * `args` - the command line without the program name.
/// * `color` - receives the `--color` value as soon as it is read, so a later
///   usage error is already reported in the requested mode.
///
/// # Returns
/// The action to take. `process.argv` mirrors Node (`[runtime, script, ...args]`)
/// so scripts written for Node read their arguments from index 2 unchanged.
///
/// # Errors
/// A [`UsageError`] for an unknown option, a missing value or a bad `--color` value.
fn parse(mut args: impl Iterator<Item = String>, color: &mut ColorChoice) -> Result<Action, UsageError> {
    let fail = |message: String, hint: Option<String>| Err(UsageError { message, hint });
    let (script, name) = loop {
        let Some(arg) = args.next() else { return Ok(Action::Missing) };
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag.to_owned(), Some(value.to_owned())),
            _ => (arg.clone(), None),
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Action::Help),
            "-v" | "--version" => return Ok(Action::Version),
            "--color" => {
                let Some(value) = inline.or_else(|| args.next()) else {
                    return fail(format!("{flag} needs a value"), Some("use auto, always or never".into()));
                };
                match ColorChoice::parse(&value) {
                    Some(choice) => *color = choice,
                    None => {
                        return fail(format!("invalid value '{value}' for --color"), Some("use auto, always or never".into()))
                    }
                }
            }
            "-e" | "--eval" => match inline.or_else(|| args.next()) {
                Some(code) => break (Source::Text { name: String::from("[eval]"), code }, String::from("[eval]")),
                None => return fail(format!("{flag} needs an argument"), None),
            },
            _ if arg.starts_with('-') && arg.len() > 1 => {
                let hint = screen::suggest(&flag, &OPTIONS).map(|option| format!("did you mean '{option}'?"));
                return fail(format!("unknown option '{flag}'"), hint);
            }
            _ => break (Source::File(PathBuf::from(&arg)), arg),
        }
    };
    let mut argv = vec![String::from("lunarstorm"), name];
    argv.extend(args);
    Ok(Action::Run(script, argv))
}

/// Builds the version line.
///
/// # Arguments
/// * `pen` - stdout's pen.
///
/// # Returns
/// `lunarstorm <version>` on a plain stream; the brand mark plus engine and target
/// on a terminal.
fn version(pen: Pen) -> String {
    if pen.0 == Depth::Plain {
        return format!("lunarstorm {}", BRAND.version);
    }
    let target = format!("{} · {}-{}", QuickJs::descriptor().engine, std::env::consts::ARCH, std::env::consts::OS);
    format!("{}  {}", screen::brand(pen, &BRAND), pen.sgr("2", &target))
}
