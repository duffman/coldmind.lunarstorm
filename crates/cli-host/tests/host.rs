//! Host behaviour against a probe package whose handlers record what they receive:
//! parsing rules, the context chain, value checks, defaults, and mount validation.

use std::cell::RefCell;

use lunarstorm_cli_contract::{Arg, Command, Failure, Package, ValueType};
use lunarstorm_cli_host::Cli;

thread_local! {
    /// What the probe's handlers saw, one line per call. Thread-local because the
    /// test harness runs each test on its own thread.
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Appends one line to the log.
///
/// # Arguments
/// * `line` - what a handler saw.
fn log(line: String) {
    LOG.with(|log| log.borrow_mut().push(line));
}

/// Runs the probe CLI.
///
/// # Arguments
/// * `words` - the command line without the program name.
///
/// # Returns
/// `(exit status, log lines)`.
fn run(words: &[&str]) -> (i32, Vec<String>) {
    LOG.with(|log| log.borrow_mut().clear());
    let status = cli(vec![Box::new(Probe)]).run(words.iter().map(|word| (*word).to_owned()).collect());
    (status, LOG.with(|log| log.borrow().clone()))
}

/// Builds the probe CLI with a set of packages.
///
/// # Arguments
/// * `packages` - the packages to mount.
///
/// # Returns
/// The CLI.
fn cli(packages: Vec<Box<dyn Package>>) -> Cli {
    packages.into_iter().fold(Cli::new("probe", "probe", "1.0.0", "test CLI"), Cli::package)
}

/// The value `build`'s `enter` hands to its children.
struct Project(String);

/// A package exercising every spec feature.
struct Probe;

impl Package for Probe {
    /// # Returns
    /// `"probe"`.
    fn id(&self) -> &'static str {
        "probe"
    }

    /// # Returns
    /// `go` (default, options first), `build` (context, children), `pick` (choice,
    /// env, default).
    fn commands(&self) -> Vec<Command> {
        vec![
            Command::new("go", "run a script")
                .default_command()
                .options_first()
                .arg(Arg::option("eval").short('e').ends_options())
                .arg(Arg::rest("script"))
                .run(|inv| {
                    log(format!("go eval={:?} script={:?}", inv.values.text("eval"), inv.values.all("script")));
                    Ok(0)
                }),
            Command::new("build", "build")
                .alias("b")
                .arg(Arg::option("project").short('p').value(ValueType::Path).default(".").global())
                .arg(Arg::flag("release"))
                .enter(|inv| {
                    let project = inv.values.text("project").unwrap_or_default().to_owned();
                    inv.context.insert(Project(project));
                    Ok(())
                })
                .run(|inv| {
                    log(format!("build release={}", inv.values.flag("release")));
                    Ok(0)
                })
                .child(Command::new("types", "types").run(|inv| {
                    log(format!("types project={}", inv.context.require::<Project>()?.0));
                    Ok(0)
                }))
                .child(Command::new("count", "count").arg(Arg::positional("n").value(ValueType::Int).required()).run(|inv| {
                    log(format!("count n={:?}", inv.values.int("n")));
                    Ok(3)
                }))
                .child(Command::new("fail", "fail").run(|_| Err(Failure::Failed { message: "no".into(), status: 9 }))),
            Command::new("pick", "pick").arg(Arg::option("mode").choices(&["a", "b"]).env("PROBE_TEST_MODE").default("a")).run(
                |inv| {
                    log(format!("pick mode={:?}", inv.values.text("mode")));
                    Ok(0)
                },
            ),
        ]
    }
}

#[test]
fn default_command_takes_the_script_and_everything_after_it() {
    let (status, log) = run(&["--color=never", "app.js", "--help", "-q"]);
    assert_eq!((status, log), (0, vec![r#"go eval=None script=["app.js", "--help", "-q"]"#.to_owned()]));
}

#[test]
fn an_ends_options_option_hands_the_rest_to_positionals() {
    let (status, log) = run(&["-e", "code", "--help", "x"]);
    assert_eq!((status, log), (0, vec![r#"go eval=Some("code") script=["--help", "x"]"#.to_owned()]));
}

#[test]
fn enter_context_reaches_children_and_globals_follow_the_subcommand() {
    assert_eq!(run(&["build", "-p", "app", "types"]), (0, vec!["types project=app".to_owned()]));
    assert_eq!(run(&["b", "types", "--project", "other"]), (0, vec!["types project=other".to_owned()]));
    assert_eq!(run(&["build", "types"]), (0, vec!["types project=.".to_owned()]));
    assert_eq!(run(&["build", "--release"]), (0, vec!["build release=true".to_owned()]));
}

#[test]
fn values_are_checked_while_parsing() {
    assert_eq!(run(&["build", "count", "12"]), (3, vec!["count n=Some(12)".to_owned()]));
    assert_eq!(run(&["build", "count", "x"]).0, 2);
    assert_eq!(run(&["build", "count"]).0, 2);
    assert_eq!(run(&["build", "count", "1", "2"]).0, 2);
    assert_eq!(run(&["build", "--release=yes"]).0, 2);
    assert_eq!(run(&["pick", "--mode", "c"]).0, 2);
    assert_eq!(run(&["pick"]), (0, vec![r#"pick mode=Some("a")"#.to_owned()]));
    assert_eq!(run(&["pick", "--mode=b"]), (0, vec![r#"pick mode=Some("b")"#.to_owned()]));
}

#[test]
fn env_variables_fill_absent_options() {
    // The only test that reads this variable, so setting it cannot race another test.
    std::env::set_var("PROBE_TEST_MODE", "b");
    let (status, log) = run(&["pick"]);
    std::env::remove_var("PROBE_TEST_MODE");
    assert_eq!((status, log), (0, vec![r#"pick mode=Some("b")"#.to_owned()]));
}

#[test]
fn usage_errors_and_failures_set_the_status() {
    assert_eq!(run(&["biuld"]), (2, vec![]));
    assert_eq!(run(&["build", "--nope"]), (2, vec![]));
    assert_eq!(run(&["build", "fail"]), (9, vec![]));
    assert_eq!(run(&[]), (2, vec![]));
    assert_eq!(run(&["--help"]), (0, vec![]));
    assert_eq!(run(&["build", "types", "--help"]), (0, vec![]));
}

/// A package that declares whatever commands it is given.
struct Raw(fn() -> Vec<Command>);

impl Package for Raw {
    /// # Returns
    /// `"raw"`.
    fn id(&self) -> &'static str {
        "raw"
    }

    /// # Returns
    /// The commands from the function.
    fn commands(&self) -> Vec<Command> {
        (self.0)()
    }
}

/// Mounts the probe and a raw package and returns the validation error.
///
/// # Arguments
/// * `commands` - the raw package's commands.
///
/// # Returns
/// The error text, or an empty string when the tree is valid.
fn problem(commands: fn() -> Vec<Command>) -> String {
    cli(vec![Box::new(Probe), Box::new(Raw(commands))]).tree().err().unwrap_or_default()
}

#[test]
fn broken_mounts_are_refused() {
    assert!(problem(|| vec![Command::new("build", "")]).contains("already defined by probe"));
    assert!(problem(|| vec![Command::new("x", "").alias("b")]).contains("'b'"));
    assert!(problem(|| vec![Command::new("describe", "")]).contains("the host"));
    assert!(problem(|| vec![Command::new("x", "").default_command()]).contains("second default"));
    assert!(problem(|| vec![Command::new("x", "").arg(Arg::flag("verbose").short('v'))]).contains("reserves"));
    assert!(problem(|| vec![Command::new("x", "").arg(Arg::option("color"))]).contains("twice"));
    assert!(problem(|| vec![Command::new("x", "").arg(Arg::flag("a").global()).child(Command::new("y", "").arg(Arg::flag("a")))])
        .contains("twice"));
    assert!(problem(|| vec![Command::new("x", "").arg(Arg::rest("a")).arg(Arg::positional("b"))]).contains("rest"));
    assert!(problem(|| vec![Command::new("x", "").child(Command::new("y", "")).child(Command::new("y", ""))]).contains("two subcommands"));
    assert_eq!(problem(|| vec![Command::new("x", "")]), "");
}
