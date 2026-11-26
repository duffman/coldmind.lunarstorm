//! The contract between the `lunarstorm` command line and the packages it mounts.
//!
//! A *package* is any crate that wants commands on the `lunarstorm` command line:
//! the runtime exports `run`, the build pipeline exports `build`. The product binary
//! (`apps/lunarstorm`) chooses which packages it mounts; the host
//! (`lunarstorm-cli-host`) parses the command line against their specs and calls
//! their handlers.
//!
//! | part             | role                                                        |
//! |------------------|-------------------------------------------------------------|
//! | [`Package`]      | what a package exports: its top-level commands, help sections |
//! | [`Command`]      | one node of a command tree: args, children, handlers        |
//! | [`Arg`]          | one flag, option, positional or rest argument               |
//! | [`Invocation`]   | what a handler receives: the path, the values, the context  |
//! | [`Context`]      | typed values handed from parent commands to their children  |
//! | [`Failure`]      | how a handler reports a usage error or a failed run         |
//!
//! # Multi-level commands and context
//!
//! A command can have children (`build types`) and, at any level, an
//! [`Command::enter`] handler. When the parsed path passes through a node, the host
//! calls its `enter` with the invocation so far; `enter` reads the arguments of its
//! level and inserts a typed value into [`Invocation::context`]. Handlers further
//! down read it back with [`Context::get`] or [`Context::require`]. `enter`
//! handlers run root to leaf, then the leaf's [`Command::run`].
//!
//! ```text
//! lunarstorm build --project app types
//!            │                  └─ run:   ctx.require::<BuildContext>()  → writes app/src/desktop.d.ts
//!            └─ enter: ctx.insert(BuildContext { project: "app" })
//! ```
//!
//! Decisions:
//! - Specs are plain data built with methods, not derive macros and not a parser
//!   library's types: a package does not depend on a parser version, and the host can
//!   render help, completions and a JSON description from the same data.
//! - A package names its own top-level commands. A namespace (`runtime eval`) is just
//!   a command with children; a root command (`run`) is a command without them. The
//!   host refuses two packages that claim the same name.
//! - Handlers write their own output (they own what is said); the host only renders
//!   help screens and [`Failure`]s.

mod invocation;
mod spec;

pub use invocation::{Context, Invocation, Values};
pub use spec::{Arg, ArgKind, Command, Enter, Run, ValueType};

/// One help section a package adds to the top-level help screen: a title and
/// `(left column, description)` rows, as `lunarstorm-cliui` renders them. A row with
/// an empty description is printed unpadded (usage lines, examples).
pub type Section = (String, Vec<(String, String)>);

/// What a crate exports to be mounted on the `lunarstorm` command line.
///
/// Implementations are usually a unit struct returned by a `package()` function in
/// the crate, e.g. `lunarstorm_host_command_line::lunar_runtime_commands::package()`, in a module named after
/// the commands it holds (`src/lunar_runtime_commands.rs`).
pub trait Package {
    /// Identifies the package in diagnostics (`"runtime"`, `"build"`), e.g. when two
    /// packages claim the same command name.
    ///
    /// # Returns
    /// A short, stable, lowercase id.
    fn id(&self) -> &'static str;

    /// Names the package's block on the top-level help screen.
    ///
    /// # Returns
    /// The block's title, shown in capitals (`"runtime"` → `RUNTIME`); the id by
    /// default.
    fn title(&self) -> &'static str {
        self.id()
    }

    /// Describes the package in one line, next to its title on the top-level help screen.
    ///
    /// # Returns
    /// The headline; empty by default, which shows the title alone.
    fn headline(&self) -> &'static str {
        ""
    }

    /// Describes the package's commands.
    ///
    /// # Returns
    /// The top-level commands, in the order the help screen lists them. Called once
    /// per process.
    fn commands(&self) -> Vec<Command>;

    /// Extra sections for the top-level help screen, after the host's own
    /// (`USAGE`, `COMMANDS`, `OPTIONS`).
    ///
    /// # Returns
    /// The sections; none by default.
    fn sections(&self) -> Vec<Section> {
        Vec::new()
    }
}

/// Why a command did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The command line cannot be acted on. The host prints `error: <message>`, the
    /// hint, and the `--help` command of the path the user typed; exit status 2.
    Usage {
        /// What is wrong, e.g. `unknown option '--verison'`.
        message: String,
        /// The suggested fix, e.g. `did you mean '--version'?`.
        hint: Option<String>,
    },
    /// The command ran and failed. The host prints `error: <message>` and exits with
    /// `status`. Handlers that already reported the failure themselves return
    /// `Ok(status)` instead.
    Failed {
        /// What went wrong.
        message: String,
        /// The exit status, 1 or more.
        status: i32,
    },
}

impl Failure {
    /// Builds a usage error without a hint.
    ///
    /// # Arguments
    /// * `message` - what is wrong.
    ///
    /// # Returns
    /// [`Failure::Usage`].
    pub fn usage(message: impl Into<String>) -> Self {
        Failure::Usage { message: message.into(), hint: None }
    }

    /// Builds a failed run with exit status 1.
    ///
    /// # Arguments
    /// * `message` - what went wrong.
    ///
    /// # Returns
    /// [`Failure::Failed`] with status 1.
    pub fn failed(message: impl Into<String>) -> Self {
        Failure::Failed { message: message.into(), status: 1 }
    }
}
