//! The command-line interface: argument parsing, help, version and usage errors.
//!
//! Everything a person sees that is not their script's own output lives in this
//! directory:
//!
//! | file        | role                                                         |
//! |-------------|--------------------------------------------------------------|
//! | `mod.rs`    | options, help screen, version line, usage errors              |
//! | `term.rs`   | color detection, styling pens, terminal width, cursor restore |
//! | `report.rs` | uncaught-error reports with source code frames                |
//! | `ui.js`     | the `lunarstorm:ui` module: styles, progress bars, spinners   |
//!
//! Decisions:
//! - Options are read only up to the script name; everything after it belongs to
//!   the script, as in Node.
//! - Output that programs parse stays byte-stable when piped: `--version` prints
//!   exactly `lunarstorm <version>` and nothing contains escape sequences.
//! - Usage errors exit with status 2 and suggest the nearest option.

pub mod report;
pub mod term;

use crate::reactor::{self, Script};
use std::path::PathBuf;
use term::{ColorChoice, Pen, CYAN, RED, VIOLET};

/// Specifier scripts import the terminal UI module by.
pub const UI_NAME: &str = "lunarstorm:ui";
/// Source of the terminal UI module, compiled on first import only.
pub const UI_SOURCE: &str = include_str!("ui.js");

/// The crate version.
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Long options, the candidates for "did you mean".
const OPTIONS: [&str; 4] = ["--eval", "--color", "--version", "--help"];

/// What the command line asks for.
enum Action {
    /// Print the help screen to stdout.
    Help,
    /// Print the version to stdout.
    Version,
    /// No script was given: print the help screen to stderr, exit 2.
    Missing,
    /// Run a script with this `process.argv`.
    Run(Script, Vec<String>),
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
            print!("{}", help(Pen(term::depth(1))));
            0
        }
        Ok(Action::Version) => {
            println!("{}", version(Pen(term::depth(1))));
            0
        }
        Ok(Action::Missing) => {
            eprint!("{}", help(Pen(term::depth(2))));
            2
        }
        Ok(Action::Run(script, argv)) => reactor::run(script, argv),
        Err(error) => {
            eprint!("{}", usage_error(Pen(term::depth(2)), &error));
            2
        }
    };
    term::restore();
    code
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
                Some(code) => break (Script::Eval(code), String::from("[eval]")),
                None => return fail(format!("{flag} needs an argument"), None),
            },
            _ if arg.starts_with('-') && arg.len() > 1 => {
                let hint = suggest(&flag).map(|option| format!("did you mean '{option}'?"));
                return fail(format!("unknown option '{flag}'"), hint);
            }
            _ => break (Script::File(PathBuf::from(&arg)), arg),
        }
    };
    let mut argv = vec![String::from("lunarstorm"), name];
    argv.extend(args);
    Ok(Action::Run(script, argv))
}

/// Finds the known option closest to a mistyped one.
///
/// # Arguments
/// * `flag` - the unknown option.
///
/// # Returns
/// The nearest long option when it is at most two edits away.
fn suggest(flag: &str) -> Option<&'static str> {
    OPTIONS
        .iter()
        .map(|option| (distance(flag, option), *option))
        .filter(|(edits, _)| *edits <= 2)
        .min_by_key(|(edits, _)| *edits)
        .map(|(_, option)| option)
}

/// Levenshtein distance between two ASCII-ish strings, one row at a time.
///
/// # Arguments
/// * `a` - first string.
/// * `b` - second string.
///
/// # Returns
/// The number of single-byte insertions, deletions and substitutions.
fn distance(a: &str, b: &str) -> usize {
    let b = b.as_bytes();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.bytes().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let substitute = diagonal + usize::from(x != *y);
            diagonal = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(diagonal + 1);
        }
    }
    row[b.len()]
}

/// # Arguments
/// * `pen` - the target stream's pen.
///
/// # Returns
/// The brand mark: moon, name in the violet-to-cyan gradient, dimmed version.
fn brand(pen: Pen) -> String {
    format!("{} {}", pen.gradient("☾ lunarstorm", VIOLET, CYAN), pen.sgr("2", &format!("v{VERSION}")))
}

/// Builds the help screen.
///
/// # Arguments
/// * `pen` - the target stream's pen.
///
/// # Returns
/// The screen, newline-terminated.
fn help(pen: Pen) -> String {
    /// Help sections: a title and `(left column, description)` rows.
    const SECTIONS: [(&str, &[(&str, &str)]); 5] = [
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
    let mut out = format!("\n  {}\n  {}\n", brand(pen), pen.sgr("2", "Node.js-inspired JavaScript runtime on QuickJS"));
    for (title, rows) in SECTIONS {
        out.push_str(&format!("\n  {}\n", pen.sgr("1", title)));
        for (left, right) in rows {
            let left = if right.is_empty() { (*left).to_owned() } else { format!("{left:<22}") };
            out.push_str(&format!("    {}{right}\n", pen.rgb(CYAN, &left)));
        }
    }
    out + "\n"
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
    if pen.0 == term::Depth::Plain {
        return format!("lunarstorm {VERSION}");
    }
    let target = format!("quickjs-ng · {}-{}", std::env::consts::ARCH, std::env::consts::OS);
    format!("{}  {}", brand(pen), pen.sgr("2", &target))
}

/// Builds a usage-error report.
///
/// # Arguments
/// * `pen` - stderr's pen.
/// * `error` - what is wrong and the suggested fix.
///
/// # Returns
/// The report, newline-terminated.
fn usage_error(pen: Pen, error: &UsageError) -> String {
    let mut out = format!("{} {}\n", pen.sgr(&format!("1;{}", pen.fg(RED)), "error:"), pen.sgr("1", &error.message));
    if let Some(hint) = &error.hint {
        out.push_str(&format!("  {} {hint}\n", pen.rgb(CYAN, "hint:")));
    }
    out + &format!("  {} lunarstorm --help\n", pen.sgr("2", "usage:"))
}
