//! Command trees and argument specs: what a package says its command line accepts.
//!
//! Everything here is built with consuming builder methods:
//!
//! ```text
//! Command::new("build", "build a desktop project")
//!     .arg(Arg::option("project").short('p').placeholder("<dir>").value(ValueType::Path).default(".").global())
//!     .enter(|inv| { inv.context.insert(Project(inv.values.path("project").unwrap())); Ok(()) })
//!     .run(|inv| { ... Ok(0) })
//!     .child(Command::new("types", "write src/desktop.d.ts").run(|inv| { ... }))
//! ```

use crate::{Failure, Invocation};

/// The leaf handler of a command.
///
/// # Arguments
/// The invocation: the command path, the parsed values, the context built by the
/// `enter` handlers above it.
///
/// # Returns
/// The process exit status.
///
/// # Errors
/// A [`Failure`] for the host to render.
pub type Run = Box<dyn Fn(&mut Invocation) -> Result<i32, Failure>>;

/// The context producer of a command level: runs when the parsed path passes through
/// the command, before any handler below it.
///
/// # Arguments
/// The invocation so far; the handler typically inserts into `context`.
///
/// # Errors
/// A [`Failure`]; the command line ends there.
pub type Enter = Box<dyn Fn(&mut Invocation) -> Result<(), Failure>>;

/// One node of a command tree.
pub struct Command {
    /// The word that selects the command, e.g. `build`.
    pub name: String,
    /// One line for the help screen.
    pub about: String,
    /// Other words that select the command.
    pub aliases: Vec<String>,
    /// The arguments read at this level.
    pub args: Vec<Arg>,
    /// Subcommands.
    pub children: Vec<Command>,
    /// The context producer of this level, see [`Enter`].
    pub enter: Option<Enter>,
    /// The handler when this command is the last word. A command without one prints
    /// its own help to stderr and exits 2 when nothing below it was named.
    pub run: Option<Run>,
    /// Options are read only up to the first positional word (or an option marked
    /// [`Arg::ends_options`]); every word after that is a positional, as Node does for
    /// `node [options] script.js [args]`. Off by default: options anywhere, `--` ends
    /// them.
    pub options_first: bool,
    /// The command parsed when the first word of the command line names no command
    /// (and is not a host option), so `lunarstorm app.js` means `lunarstorm run
    /// app.js`. At most one top-level command in the whole CLI may set it.
    pub default: bool,
    /// Example command lines for the command's help screen.
    pub examples: Vec<String>,
}

impl Command {
    /// Starts a command.
    ///
    /// # Arguments
    /// * `name` - the word that selects it.
    /// * `about` - one line for the help screen.
    ///
    /// # Returns
    /// A command with no args, children or handlers.
    pub fn new(name: impl Into<String>, about: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            about: about.into(),
            aliases: Vec::new(),
            args: Vec::new(),
            children: Vec::new(),
            enter: None,
            run: None,
            options_first: false,
            default: false,
            examples: Vec::new(),
        }
    }

    /// Adds an alias.
    ///
    /// # Arguments
    /// * `alias` - another word that selects the command.
    ///
    /// # Returns
    /// The command.
    pub fn alias(mut self, alias: impl Into<String>) -> Self {
        self.aliases.push(alias.into());
        self
    }

    /// Adds an argument. Positionals fill in the order they are added.
    ///
    /// # Arguments
    /// * `arg` - the argument.
    ///
    /// # Returns
    /// The command.
    pub fn arg(mut self, arg: Arg) -> Self {
        self.args.push(arg);
        self
    }

    /// Adds a subcommand.
    ///
    /// # Arguments
    /// * `child` - the subcommand.
    ///
    /// # Returns
    /// The command.
    pub fn child(mut self, child: Command) -> Self {
        self.children.push(child);
        self
    }

    /// Sets the context producer, see [`Enter`].
    ///
    /// # Arguments
    /// * `enter` - the producer.
    ///
    /// # Returns
    /// The command.
    pub fn enter(mut self, enter: impl Fn(&mut Invocation) -> Result<(), Failure> + 'static) -> Self {
        self.enter = Some(Box::new(enter));
        self
    }

    /// Sets the leaf handler, see [`Run`].
    ///
    /// # Arguments
    /// * `run` - the handler.
    ///
    /// # Returns
    /// The command.
    pub fn run(mut self, run: impl Fn(&mut Invocation) -> Result<i32, Failure> + 'static) -> Self {
        self.run = Some(Box::new(run));
        self
    }

    /// Reads options only before the first positional, see [`Command::options_first`].
    ///
    /// # Returns
    /// The command.
    pub fn options_first(mut self) -> Self {
        self.options_first = true;
        self
    }

    /// Makes this the default command, see [`Command::default`].
    ///
    /// # Returns
    /// The command.
    pub fn default_command(mut self) -> Self {
        self.default = true;
        self
    }

    /// Adds an example command line.
    ///
    /// # Arguments
    /// * `example` - the full command line, e.g. `lunarstorm build -p app`.
    ///
    /// # Returns
    /// The command.
    pub fn example(mut self, example: impl Into<String>) -> Self {
        self.examples.push(example.into());
        self
    }

    /// Tells whether a word selects this command.
    ///
    /// # Arguments
    /// * `word` - a command-line word.
    ///
    /// # Returns
    /// `true` for the name or an alias.
    pub fn is_named(&self, word: &str) -> bool {
        self.name == word || self.aliases.iter().any(|alias| alias == word)
    }
}

/// How an argument appears on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// `--name` / `-n`: present or not.
    Flag,
    /// `--name <value>`, `--name=<value>` or `-n <value>`; given again, the last one
    /// wins for [`crate::Values::text`] and all are kept for [`crate::Values::all`].
    Option,
    /// A word in its position.
    Positional,
    /// Every remaining positional word; must be the last positional of its command.
    Rest,
}

/// What an option's or positional's value must be. Checked while parsing, so a
/// handler's typed getter never fails on a present value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueType {
    /// Any text.
    Text,
    /// A file system path; any text, completed as a file by the shell.
    Path,
    /// A signed 64-bit integer.
    Int,
    /// One of these words.
    Choice(Vec<String>),
}

/// One argument of a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    /// The key in [`crate::Values`], and the long option name (`--name`) for flags
    /// and options.
    pub name: String,
    /// The short option letter (`-n`), for flags and options.
    pub short: Option<char>,
    /// How the argument appears.
    pub kind: ArgKind,
    /// What its value must be; ignored for flags.
    pub value: ValueType,
    /// The value's name on the help screen, e.g. `<dir>`; derived from `name` when
    /// empty.
    pub placeholder: String,
    /// One line for the help screen.
    pub about: String,
    /// The value used when the argument is absent and `env` is unset.
    pub default: Option<String>,
    /// The environment variable read when the argument is absent.
    pub env: Option<String>,
    /// The command line is a usage error without it (after `env` and `default`).
    pub required: bool,
    /// Also accepted by every command below the one that declares it.
    pub global: bool,
    /// Reading this option ends option parsing: every later word is a positional
    /// (`-e <code>` followed by the script's own arguments).
    pub ends_options: bool,
}

impl Arg {
    /// Starts an argument of any kind.
    ///
    /// # Arguments
    /// * `name` - key and long name.
    /// * `kind` - how it appears.
    ///
    /// # Returns
    /// A text argument with no short name, default or env.
    fn new(name: impl Into<String>, kind: ArgKind) -> Self {
        Self {
            name: name.into(),
            short: None,
            kind,
            value: ValueType::Text,
            placeholder: String::new(),
            about: String::new(),
            default: None,
            env: None,
            required: false,
            global: false,
            ends_options: false,
        }
    }

    /// Starts a flag.
    ///
    /// # Arguments
    /// * `name` - key and long name.
    ///
    /// # Returns
    /// The flag.
    pub fn flag(name: impl Into<String>) -> Self {
        Self::new(name, ArgKind::Flag)
    }

    /// Starts an option that takes a value.
    ///
    /// # Arguments
    /// * `name` - key and long name.
    ///
    /// # Returns
    /// The option.
    pub fn option(name: impl Into<String>) -> Self {
        Self::new(name, ArgKind::Option)
    }

    /// Starts a positional argument.
    ///
    /// # Arguments
    /// * `name` - key.
    ///
    /// # Returns
    /// The positional.
    pub fn positional(name: impl Into<String>) -> Self {
        Self::new(name, ArgKind::Positional)
    }

    /// Starts the argument that takes every remaining positional word.
    ///
    /// # Arguments
    /// * `name` - key.
    ///
    /// # Returns
    /// The rest argument.
    pub fn rest(name: impl Into<String>) -> Self {
        Self::new(name, ArgKind::Rest)
    }

    /// Sets the short option letter.
    ///
    /// # Arguments
    /// * `short` - the letter, used as `-<short>`.
    ///
    /// # Returns
    /// The argument.
    pub fn short(mut self, short: char) -> Self {
        self.short = Some(short);
        self
    }

    /// Sets the value type.
    ///
    /// # Arguments
    /// * `value` - the type.
    ///
    /// # Returns
    /// The argument.
    pub fn value(mut self, value: ValueType) -> Self {
        self.value = value;
        self
    }

    /// Restricts the value to a set of words.
    ///
    /// # Arguments
    /// * `choices` - the accepted words.
    ///
    /// # Returns
    /// The argument.
    pub fn choices(self, choices: &[&str]) -> Self {
        self.value(ValueType::Choice(choices.iter().map(|choice| (*choice).to_owned()).collect()))
    }

    /// Sets the value's name on the help screen.
    ///
    /// # Arguments
    /// * `placeholder` - e.g. `<dir>` or `<file.js> [args...]`.
    ///
    /// # Returns
    /// The argument.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Sets the help line.
    ///
    /// # Arguments
    /// * `about` - one line.
    ///
    /// # Returns
    /// The argument.
    pub fn about(mut self, about: impl Into<String>) -> Self {
        self.about = about.into();
        self
    }

    /// Sets the default value.
    ///
    /// # Arguments
    /// * `default` - used when the argument and its env variable are absent.
    ///
    /// # Returns
    /// The argument.
    pub fn default(mut self, default: impl Into<String>) -> Self {
        self.default = Some(default.into());
        self
    }

    /// Sets the environment variable read when the argument is absent.
    ///
    /// # Arguments
    /// * `env` - the variable's name.
    ///
    /// # Returns
    /// The argument.
    pub fn env(mut self, env: impl Into<String>) -> Self {
        self.env = Some(env.into());
        self
    }

    /// Makes the argument required.
    ///
    /// # Returns
    /// The argument.
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Makes the argument global, see [`Arg::global`].
    ///
    /// # Returns
    /// The argument.
    pub fn global(mut self) -> Self {
        self.global = true;
        self
    }

    /// Makes reading this option end option parsing, see [`Arg::ends_options`].
    ///
    /// # Returns
    /// The argument.
    pub fn ends_options(mut self) -> Self {
        self.ends_options = true;
        self
    }

    /// The value's name as the help screen and errors show it.
    ///
    /// # Returns
    /// [`Arg::placeholder`], or `<name>` when it is empty.
    pub fn shown_value(&self) -> String {
        if self.placeholder.is_empty() {
            format!("<{}>", self.name)
        } else {
            self.placeholder.clone()
        }
    }
}
