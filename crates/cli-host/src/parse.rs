//! Parses a command line against a command tree.
//!
//! Rules, in the order they apply to each word:
//! 1. While options are open, `--` closes them and is dropped.
//! 2. While options are open, a word starting with `-` is an option: `--name`,
//!    `--name=value`, `--name value`, `-n`, `-n value`. `-h`/`--help` and
//!    `-v`/`--version` belong to the host at every level. The options in scope are
//!    the current command's plus the `global` ones of every command above it.
//! 3. Any other word names a child of the current command when no positional of that
//!    command was taken yet; otherwise it fills the next positional, and a `Rest`
//!    argument takes all that remain.
//! 4. At the top level, a word or option that names nothing goes, in this order, to an
//!    external `lunarstorm-<word>` program on `PATH`, then to the default command
//!    (re-read there), else it is an unknown command or option.
//!
//! Decision: values are checked against their [`ValueType`] here, so handlers read
//! them with infallible getters and every bad value gets the same error format.

use lunarstorm_cli_contract::{Arg, ArgKind, Command, Failure, ValueType, Values};
use lunarstorm_cliui::screen;

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Print the help screen of the command path to stdout.
    Help,
    /// Print the version to stdout.
    Version,
    /// Nothing runnable was named: print the help of the path to stderr, exit 2.
    Missing,
    /// Run the leaf of the path.
    Run,
    /// Run `lunarstorm-<name>` with these arguments.
    External(String, Vec<String>),
}

/// The result of parsing, kept whole even when the command line is wrong, so the
/// host still knows the `--color` value and the path to print a usage line for.
pub(crate) struct Parsed<'a> {
    /// The commands from the root down; the root is the host's own.
    pub path: Vec<&'a Command>,
    /// The command words the user typed, for the `usage:` line of an error. A default
    /// command entered implicitly is not among them.
    pub typed: Vec<String>,
    /// Every value read so far.
    pub values: Values,
    /// What to do, or why the command line cannot be acted on.
    pub action: Result<Action, Failure>,
}

/// Parses a command line.
///
/// # Arguments
/// * `root` - the host's root command; its children are the mounted top-level
///   commands.
/// * `words` - the command line without the program name.
/// * `external` - tells whether `lunarstorm-<word>` exists on `PATH`.
///
/// # Returns
/// The parse; see [`Parsed`].
pub(crate) fn parse<'a>(root: &'a Command, words: &[String], external: &dyn Fn(&str) -> bool) -> Parsed<'a> {
    let mut parser = Parser { path: vec![root], typed: Vec::new(), values: Values::default(), taken: 0, open: true };
    let action = parser.run(words, external);
    Parsed { path: parser.path, typed: parser.typed, values: parser.values, action }
}

/// The parser's state while it walks the words.
struct Parser<'a> {
    /// See [`Parsed::path`].
    path: Vec<&'a Command>,
    /// See [`Parsed::typed`].
    typed: Vec<String>,
    /// See [`Parsed::values`].
    values: Values,
    /// How many positional words the current command has taken.
    taken: usize,
    /// Whether words starting with `-` are still options.
    open: bool,
}

impl<'a> Parser<'a> {
    /// Walks the words, then applies env variables, defaults and requirements.
    ///
    /// # Arguments
    /// * `words` - the command line without the program name.
    /// * `external` - see [`parse`].
    ///
    /// # Returns
    /// The action.
    ///
    /// # Errors
    /// A [`Failure::Usage`] for the first word that cannot be read.
    fn run(&mut self, words: &[String], external: &dyn Fn(&str) -> bool) -> Result<Action, Failure> {
        let mut index = 0;
        while let Some(word) = words.get(index) {
            index += 1;
            if self.open && word == "--" {
                self.open = false;
            } else if self.open && word.len() > 1 && word.starts_with('-') {
                let (flag, inline) = match word.split_once('=') {
                    Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
                    _ => (word.as_str(), None),
                };
                match flag {
                    "-h" | "--help" => return Ok(Action::Help),
                    "-v" | "--version" => return Ok(Action::Version),
                    _ => {}
                }
                let Some(arg) = self.option(flag) else {
                    if self.enter_default() {
                        index -= 1;
                        continue;
                    }
                    let hint = screen::suggest(flag, &self.option_names().iter().map(String::as_str).collect::<Vec<_>>())
                        .map(|option| format!("did you mean '{option}'?"));
                    return Err(Failure::Usage { message: format!("unknown option '{flag}'"), hint });
                };
                if arg.kind == ArgKind::Flag {
                    if inline.is_some() {
                        return Err(Failure::usage(format!("{flag} takes no value")));
                    }
                    self.values.set_flag(&arg.name);
                    continue;
                }
                let value = match inline {
                    Some(value) => value,
                    None => {
                        let Some(value) = words.get(index) else {
                            return Err(Failure::usage(format!("{flag} needs an argument")));
                        };
                        index += 1;
                        value.clone()
                    }
                };
                check(arg, flag, &value)?;
                self.values.push(&arg.name, value);
                if arg.ends_options {
                    self.open = false;
                }
            } else {
                let current = self.current();
                if self.taken == 0 {
                    if let Some(child) = current.children.iter().find(|child| child.is_named(word)) {
                        self.descend(child);
                        self.typed.push(word.clone());
                        continue;
                    }
                }
                if self.path.len() == 1 {
                    if external(word) {
                        return Ok(Action::External(word.clone(), words[index..].to_vec()));
                    }
                    let names: Vec<&str> = current.children.iter().map(|child| child.name.as_str()).collect();
                    let near = screen::suggest(word, &names);
                    // A near-miss of a command name that is not a file is a typo, not a
                    // script for the default command: `lunarstorm biuld` says so instead
                    // of failing to load a module named `biuld`.
                    if (near.is_none() || std::path::Path::new(word).exists()) && self.enter_default() {
                        index -= 1;
                        continue;
                    }
                    return Err(Failure::Usage {
                        message: format!("unknown command '{word}'"),
                        hint: near.map(|name| format!("did you mean '{name}'?")),
                    });
                }
                self.positional(word)?;
            }
        }
        self.finish()
    }

    /// The command the next word belongs to.
    ///
    /// # Returns
    /// The last command on the path.
    fn current(&self) -> &'a Command {
        self.path[self.path.len() - 1]
    }

    /// Moves down to a child command.
    ///
    /// # Arguments
    /// * `child` - the command entered.
    fn descend(&mut self, child: &'a Command) {
        self.path.push(child);
        self.taken = 0;
    }

    /// Enters the default command implicitly when still at the top level.
    ///
    /// # Returns
    /// `true` when it was entered; the caller re-reads the current word there.
    fn enter_default(&mut self) -> bool {
        if self.path.len() != 1 {
            return false;
        }
        match self.path[0].children.iter().find(|child| child.default) {
            Some(default) => {
                self.descend(default);
                true
            }
            None => false,
        }
    }

    /// The flags and options in scope: the current command's and the globals above.
    ///
    /// # Returns
    /// The arguments, innermost command first.
    fn scope(&self) -> impl Iterator<Item = &'a Arg> + '_ {
        let last = self.path.len() - 1;
        self.path.iter().enumerate().rev().flat_map(move |(level, command)| {
            command.args.iter().filter(move |arg| {
                matches!(arg.kind, ArgKind::Flag | ArgKind::Option) && (level == last || arg.global)
            })
        })
    }

    /// Finds the argument an option word names.
    ///
    /// # Arguments
    /// * `flag` - `--name` or `-n`.
    ///
    /// # Returns
    /// The argument in scope, or `None`.
    fn option(&self, flag: &str) -> Option<&'a Arg> {
        let mut found = self.scope().filter(|arg| match flag.strip_prefix("--") {
            Some(long) => arg.name == long,
            None => flag.len() == 2 && arg.short.is_some_and(|short| flag.ends_with(short)),
        });
        found.next()
    }

    /// Lists the long options in scope, for "did you mean".
    ///
    /// # Returns
    /// `--name` for each, plus the host's `--help` and `--version`.
    fn option_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.scope().map(|arg| format!("--{}", arg.name)).collect();
        names.extend(["--help".to_owned(), "--version".to_owned()]);
        names
    }

    /// Fills the next positional of the current command.
    ///
    /// # Arguments
    /// * `word` - the word.
    ///
    /// # Errors
    /// A usage error when the command takes no more positionals or the value is bad.
    fn positional(&mut self, word: &str) -> Result<(), Failure> {
        let command = self.current();
        let positionals: Vec<&Arg> =
            command.args.iter().filter(|arg| matches!(arg.kind, ArgKind::Positional | ArgKind::Rest)).collect();
        // Past the declared positionals, a trailing `Rest` keeps taking words.
        let arg = match positionals.get(self.taken).or(positionals.last()) {
            Some(arg) if self.taken < positionals.len() || arg.kind == ArgKind::Rest => *arg,
            _ => return Err(Failure::usage(format!("unexpected argument '{word}'"))),
        };
        check(arg, &arg.shown_value(), word)?;
        self.values.push(&arg.name, word.to_owned());
        // Counted for every word, so once a positional is taken no later word can
        // select a child command.
        self.taken += 1;
        if command.options_first {
            self.open = false;
        }
        Ok(())
    }

    /// Applies env variables and defaults, then checks required arguments.
    ///
    /// # Returns
    /// [`Action::Run`], or [`Action::Missing`] when the path ends at a command
    /// without a handler.
    ///
    /// # Errors
    /// A usage error for a bad env value or a missing required argument.
    fn finish(&mut self) -> Result<Action, Failure> {
        for command in self.path.clone() {
            for arg in &command.args {
                if self.values.has(&arg.name) || arg.kind == ArgKind::Flag {
                    continue;
                }
                if let Some(value) = arg.env.as_deref().and_then(|name| std::env::var(name).ok()) {
                    check(arg, arg.env.as_deref().unwrap_or_default(), &value)?;
                    self.values.push(&arg.name, value);
                } else if let Some(value) = &arg.default {
                    self.values.push(&arg.name, value.clone());
                }
            }
        }
        if self.current().run.is_none() {
            return Ok(Action::Missing);
        }
        for command in &self.path {
            if let Some(arg) = command.args.iter().find(|arg| arg.required && !self.values.has(&arg.name)) {
                let shown = match arg.kind {
                    ArgKind::Option => format!("--{} {}", arg.name, arg.shown_value()),
                    _ => arg.shown_value(),
                };
                return Err(Failure::usage(format!("missing {shown}")));
            }
        }
        Ok(Action::Run)
    }
}

/// Checks a value against its argument's type.
///
/// # Arguments
/// * `arg` - the argument.
/// * `shown` - how the error names it: `--color`, `<count>` or an env variable.
/// * `value` - the value.
///
/// # Errors
/// `invalid value '<value>' for <shown>` with a hint naming what is accepted.
fn check(arg: &Arg, shown: &str, value: &str) -> Result<(), Failure> {
    let hint = match &arg.value {
        ValueType::Int if value.parse::<i64>().is_err() => "expected an integer".to_owned(),
        ValueType::Choice(choices) if !choices.iter().any(|choice| choice == value) => format!("use {}", listing(choices)),
        _ => return Ok(()),
    };
    Err(Failure::Usage { message: format!("invalid value '{value}' for {shown}"), hint: Some(hint) })
}

/// Joins words as prose.
///
/// # Arguments
/// * `words` - the words.
///
/// # Returns
/// `a`, `a or b`, `a, b or c`.
pub(crate) fn listing(words: &[String]) -> String {
    match words.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}
