//! The `lunarstorm` command-line host.
//!
//! A product binary names its packages and hands over the command line:
//!
//! ```text
//! Cli::new("lunarstorm", "☾ lunarstorm", VERSION, "runtime and build tools")
//!     .package(lunarstorm_host_cli::command::package())
//!     .package(lunarstorm_pipeline::command::package())
//!     .run(std::env::args().skip(1).collect())
//! ```
//!
//! | module       | role                                                           |
//! |--------------|----------------------------------------------------------------|
//! | [`Cli`]      | mounting, validation, dispatch, version, errors                |
//! | `parse`      | the command line against the tree                              |
//! | `help`       | help screens from the specs                                    |
//! | `describe`   | the `describe` JSON and shell completion                       |
//!
//! What the host adds to every CLI:
//! - `--color <when>` (global), `-h/--help` and `-v/--version` at every level;
//! - `help [command...]`: the help of a command path, as `<path> --help` prints it;
//! - `describe`: the whole tree as JSON; `completions <bash|zsh>`: a completion script
//!   backed by the hidden `__complete` command;
//! - external commands: `lunarstorm foo ...` runs `lunarstorm-foo ...` from `PATH`
//!   when no mounted command is named `foo`, as cargo and git do;
//! - exit statuses: 0 success, 2 usage error (or nothing runnable named), 70 a broken
//!   mount (two packages claim a name), otherwise what the handler returns.

mod describe;
mod help;
mod parse;

use std::path::PathBuf;

use lunarstorm_cli_contract::{Arg, ArgKind, Command, Context, Failure, Invocation, Package, Section};
use lunarstorm_cliui::screen::{self, Brand};
use lunarstorm_cliui::term::{self, ColorChoice, Depth, Pen, RED};
use parse::Action;

/// Top-level names the host keeps for itself.
const BUILTINS: [&str; 4] = ["help", "describe", "completions", "__complete"];
/// Long option names the host keeps for itself at every level.
const RESERVED_NAMES: [&str; 3] = ["help", "version", "color"];
/// Short option letters the host keeps for itself at every level.
const RESERVED_SHORTS: [char; 2] = ['h', 'v'];
/// Exit status of a usage error.
const USAGE: i32 = 2;
/// Exit status of a broken mount (`EX_SOFTWARE`).
const BROKEN: i32 = 70;

/// A command line made of mounted packages.
pub struct Cli {
    /// The program name, used in usage lines and for external commands.
    program: String,
    /// The prefix of external commands: `<prefix>-<name>` on `PATH`. Defaults to the
    /// program name; the `lunar` binary sets `lunarstorm` so `lunar pack` runs
    /// `lunarstorm-pack`.
    prefix: String,
    /// The brand mark at the top of help screens, e.g. `☾ lunarstorm`.
    mark: String,
    /// The program version.
    version: String,
    /// One line under the mark on the top-level help screen.
    tagline: String,
    /// Shown after the version on a terminal, e.g. the engine and target.
    detail: String,
    /// The mounted packages, in help order.
    packages: Vec<Box<dyn Package>>,
}

/// The mounted tree: the host's root command and the packages' help sections.
pub struct Tree {
    /// The root: the host's options; children are the packages' top-level commands
    /// followed by the host's built-in commands.
    pub root: Command,
    /// The packages' extra help sections.
    pub sections: Vec<Section>,
    /// One block per mounted package, in mount order, for the top-level help screen.
    pub groups: Vec<Group>,
}

/// One package's block on the top-level help screen.
///
/// Decision: the top-level screen lists commands per package (title, headline, then a
/// command / arguments / description table) so a reader sees which crate contributes
/// what; the host's own commands and external programs get blocks of their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// The block title, e.g. `RUNTIME`.
    pub title: String,
    /// One line next to the title; may be empty.
    pub headline: String,
    /// The names of the block's top-level commands, in help order.
    pub commands: Vec<String>,
}

impl Cli {
    /// Starts a CLI with no packages.
    ///
    /// # Arguments
    /// * `program` - the program name, e.g. `lunarstorm`.
    /// * `mark` - the brand mark on help screens.
    /// * `version` - the version, without a `v` prefix.
    /// * `tagline` - one line under the mark.
    ///
    /// # Returns
    /// The CLI.
    pub fn new(program: impl Into<String>, mark: impl Into<String>, version: impl Into<String>, tagline: impl Into<String>) -> Self {
        let program = program.into();
        Self {
            prefix: program.clone(),
            program,
            mark: mark.into(),
            version: version.into(),
            tagline: tagline.into(),
            detail: String::new(),
            packages: Vec::new(),
        }
    }

    /// Sets the text shown after the version on a terminal.
    ///
    /// # Arguments
    /// * `detail` - e.g. `quickjs-ng · aarch64-macos`.
    ///
    /// # Returns
    /// The CLI.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    /// Sets the prefix of external commands.
    ///
    /// # Arguments
    /// * `prefix` - external commands are `<prefix>-<name>` on `PATH`.
    ///
    /// # Returns
    /// The CLI.
    pub fn external_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Mounts a package.
    ///
    /// # Arguments
    /// * `package` - the package; its top-level commands join the root.
    ///
    /// # Returns
    /// The CLI.
    pub fn package(mut self, package: Box<dyn Package>) -> Self {
        self.packages.push(package);
        self
    }

    /// Builds and checks the mounted tree.
    ///
    /// # Returns
    /// The tree.
    ///
    /// # Errors
    /// The first problem found: two commands with one name, two default commands, a
    /// name or letter the host reserves, an argument name repeated along a path, or a
    /// `Rest` argument that is not the last positional.
    pub fn tree(&self) -> Result<Tree, String> {
        let color = Arg::option("color")
            .placeholder("<when>")
            .choices(&["auto", "always", "never"])
            .about("color output: auto, always or never")
            .global();
        let mut root = Command::new(&self.program, &self.tagline).arg(color);
        let mut owners: Vec<(String, &str)> = BUILTINS.iter().map(|name| ((*name).to_owned(), "the host")).collect();
        let mut sections = Vec::new();
        let mut groups = Vec::new();
        for package in &self.packages {
            let mut group =
                Group { title: package.title().to_uppercase(), headline: package.headline().to_owned(), commands: Vec::new() };
            for command in package.commands() {
                group.commands.push(command.name.clone());
                for name in std::iter::once(&command.name).chain(&command.aliases) {
                    if let Some((_, owner)) = owners.iter().find(|(taken, _)| taken == name) {
                        return Err(format!("command '{name}' of package '{}' is already defined by {owner}", package.id()));
                    }
                    owners.push((name.clone(), package.id()));
                }
                if command.default && root.children.iter().any(|child| child.default) {
                    return Err(format!("package '{}' declares a second default command '{}'", package.id(), command.name));
                }
                root.children.push(command);
            }
            sections.extend(package.sections());
            groups.push(group);
        }
        // The built-ins' handlers are placeholders that make them runnable for the
        // parser; `act` answers them itself because they need the whole tree.
        // Fix: `lunarstorm help` reached the default command, which ran a script named
        // `help`. Decision: `help` is a built-in that prints the named path's help, as
        // git and cargo do; a script called `help` still runs as `lunarstorm ./help`.
        root.children.push(
            Command::new("help", "print the help of a command")
                .arg(Arg::rest("command").placeholder("<command>...").about("the command path, e.g. build types"))
                .run(|_| Ok(0)),
        );
        root.children.push(Command::new("describe", "print every command and argument as JSON").run(|_| Ok(0)));
        root.children.push(
            Command::new("completions", "print a shell completion script")
                .arg(Arg::positional("shell").placeholder("<shell>").choices(&["bash", "zsh"]).about("bash or zsh").required())
                .example(format!("source <({} completions zsh)", self.program))
                .run(|_| Ok(0)),
        );
        for child in &root.children {
            check(child, &[&root], &self.program)?;
        }
        Ok(Tree { root, sections, groups })
    }

    /// Runs a command line to completion.
    ///
    /// # Arguments
    /// * `words` - the command line without the program name.
    ///
    /// # Returns
    /// The process exit status.
    pub fn run(self, words: Vec<String>) -> i32 {
        let tree = match self.tree() {
            Ok(tree) => tree,
            Err(problem) => {
                eprintln!("{}: internal: {problem}", self.program);
                return BROKEN;
            }
        };
        if words.first().is_some_and(|word| word == "__complete") {
            for candidate in describe::complete(&tree.root, &words[1..]) {
                println!("{candidate}");
            }
            return 0;
        }
        let parsed = parse::parse(&tree.root, &words, &|name| self.external(name).is_some());
        // Applied before anything is printed, so a usage error is already in the
        // requested mode.
        term::init(parsed.values.text("color").and_then(ColorChoice::parse).unwrap_or(ColorChoice::Auto));
        let status = self.act(&tree, parsed, words.is_empty());
        term::restore();
        status
    }

    /// Carries out a parse.
    ///
    /// # Arguments
    /// * `tree` - the mounted tree.
    /// * `parsed` - the parse of the command line.
    /// * `empty` - the command line had no words: print the top help to stderr.
    ///
    /// # Returns
    /// The exit status.
    fn act(&self, tree: &Tree, parsed: parse::Parsed, empty: bool) -> i32 {
        let (out, err) = (Pen(term::depth(1)), Pen(term::depth(2)));
        let typed = std::iter::once(self.program.clone()).chain(parsed.typed.iter().cloned()).collect::<Vec<_>>().join(" ");
        let action = match parsed.action {
            _ if empty => Action::Missing,
            Ok(action) => action,
            Err(failure) => return self.fail(err, failure, &typed),
        };
        match action {
            Action::Help => {
                print!("{}", self.help(out, tree, &parsed.path));
                0
            }
            Action::Missing => {
                eprint!("{}", self.help(err, tree, &parsed.path));
                USAGE
            }
            Action::Version => {
                println!("{}", self.version_line(out));
                0
            }
            Action::External(name, args) => {
                let Some(program) = self.external(&name) else { return USAGE };
                match std::process::Command::new(&program).args(args).status() {
                    Ok(status) => status.code().unwrap_or(1),
                    Err(error) => self.fail(err, Failure::failed(format!("cannot run {}: {error}", program.display())), &typed),
                }
            }
            Action::Run => match parsed.path[1].name.as_str() {
                "help" => match help_path(&tree.root, parsed.values.all("command")) {
                    Ok(path) => {
                        print!("{}", self.help(out, tree, &path));
                        0
                    }
                    Err(failure) => self.fail(err, failure, &typed),
                },
                "describe" => {
                    let json = describe::describe(&self.program, &self.version, &tree.root);
                    println!("{}", serde_json::to_string_pretty(&json).expect("JSON values always serialize"));
                    0
                }
                "completions" => {
                    print!("{}", describe::script(&self.program, parsed.values.text("shell").unwrap_or("bash")));
                    0
                }
                _ => {
                    let path: Vec<String> = parsed.path[1..].iter().map(|command| command.name.clone()).collect();
                    let mut invocation = Invocation { path, values: parsed.values, context: Context::default() };
                    match dispatch(&parsed.path[1..], &mut invocation) {
                        Ok(status) => status,
                        Err(failure) => self.fail(err, failure, &typed),
                    }
                }
            },
        }
    }

    /// Prints a failure to stderr.
    ///
    /// # Arguments
    /// * `pen` - stderr's pen.
    /// * `failure` - what went wrong.
    /// * `typed` - the program and command words the user typed, for the usage line.
    ///
    /// # Returns
    /// The exit status: 2 for a usage error, else the failure's status.
    fn fail(&self, pen: Pen, failure: Failure, typed: &str) -> i32 {
        match failure {
            Failure::Usage { message, hint } => {
                eprint!("{}", screen::usage_error(pen, &message, hint.as_deref(), &format!("{typed} --help")));
                USAGE
            }
            Failure::Failed { message, status } => {
                eprintln!("{} {}", pen.sgr(&format!("1;{}", pen.fg(RED)), "error:"), pen.sgr("1", &message));
                status
            }
        }
    }

    /// Builds the help screen of a path.
    ///
    /// # Arguments
    /// * `pen` - the target stream's pen.
    /// * `tree` - the mounted tree.
    /// * `path` - the commands from the root down.
    ///
    /// # Returns
    /// The top-level screen for the root alone, else the command's screen.
    fn help(&self, pen: Pen, tree: &Tree, path: &[&Command]) -> String {
        let leaf = path[path.len() - 1];
        let tagline = if path.len() == 1 { self.tagline.as_str() } else { leaf.about.as_str() };
        let brand = Brand { mark: &self.mark, version: &self.version, tagline };
        let sections = if path.len() == 1 {
            help::top(&self.program, &self.prefix, &tree.root, &tree.groups, &tree.sections, &self.externals(&tree.root))
        } else {
            help::command(&self.program, path)
        };
        help::render(pen, &brand, &sections)
    }

    /// Builds the version line.
    ///
    /// # Arguments
    /// * `pen` - stdout's pen.
    ///
    /// # Returns
    /// Exactly `<program> <version>` on a plain stream, so scripts can parse it; the
    /// brand mark and the detail on a terminal.
    fn version_line(&self, pen: Pen) -> String {
        if pen.0 == Depth::Plain {
            return format!("{} {}", self.program, self.version);
        }
        let brand = Brand { mark: &self.mark, version: &self.version, tagline: "" };
        format!("{}  {}", screen::brand(pen, &brand), pen.sgr("2", &self.detail))
    }

    /// Finds the external program for a command name.
    ///
    /// # Arguments
    /// * `name` - the command word.
    ///
    /// # Returns
    /// The first executable `<prefix>-<name>` on `PATH`.
    fn external(&self, name: &str) -> Option<PathBuf> {
        if name.is_empty() || name.contains(std::path::MAIN_SEPARATOR) {
            return None;
        }
        let file = format!("{}-{name}", self.prefix);
        std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(&file)).find(|path| executable(path))
    }

    /// Lists the external programs on `PATH` for the top-level help screen.
    ///
    /// # Arguments
    /// * `root` - the root, whose commands shadow externals of the same name.
    ///
    /// # Returns
    /// The command names, sorted, without duplicates.
    fn externals(&self, root: &Command) -> Vec<String> {
        let prefix = format!("{}-", self.prefix);
        let mut names: Vec<String> = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .unwrap_or_default()
            .iter()
            .filter_map(|dir| std::fs::read_dir(dir).ok())
            .flatten()
            .flatten()
            .filter(|entry| executable(&entry.path()))
            .filter_map(|entry| entry.file_name().to_str()?.strip_prefix(&prefix).map(str::to_owned))
            .filter(|name| !root.children.iter().any(|child| child.is_named(name)))
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

/// Runs the `enter` handlers of a path, then the leaf's `run`.
///
/// # Arguments
/// * `path` - the commands below the root.
/// * `invocation` - the parsed command line.
///
/// # Returns
/// The leaf's exit status.
///
/// # Errors
/// The first failure of a handler.
fn dispatch(path: &[&Command], invocation: &mut Invocation) -> Result<i32, Failure> {
    for command in path {
        if let Some(enter) = &command.enter {
            enter(invocation)?;
        }
    }
    let run = path[path.len() - 1].run.as_ref().expect("the parser returns Run only for a command with a handler");
    run(invocation)
}

/// Checks one command and its children against the host's rules.
///
/// # Arguments
/// * `command` - the command.
/// * `above` - the commands above it, from the root down.
/// * `program` - the program name, for messages.
///
/// # Errors
/// A description of the first problem, naming the command path.
fn check(command: &Command, above: &[&Command], program: &str) -> Result<(), String> {
    let at = || {
        let names: Vec<&str> = above[1..].iter().map(|command| command.name.as_str()).chain([command.name.as_str()]).collect();
        format!("'{program} {}'", names.join(" "))
    };
    let taken: Vec<&str> = above.iter().flat_map(|command| command.args.iter().map(|arg| arg.name.as_str())).collect();
    let mut seen: Vec<&str> = Vec::new();
    for arg in &command.args {
        let options = matches!(arg.kind, ArgKind::Flag | ArgKind::Option);
        if taken.contains(&arg.name.as_str()) || seen.contains(&arg.name.as_str()) {
            return Err(format!("{} declares argument '{}' twice along its path", at(), arg.name));
        }
        if options && (RESERVED_NAMES.contains(&arg.name.as_str()) || arg.short.is_some_and(|short| RESERVED_SHORTS.contains(&short))) {
            return Err(format!("{} uses option '{}', which the host reserves", at(), arg.name));
        }
        seen.push(&arg.name);
    }
    let positionals: Vec<&Arg> = command.args.iter().filter(|arg| matches!(arg.kind, ArgKind::Positional | ArgKind::Rest)).collect();
    if positionals.iter().rev().skip(1).any(|arg| arg.kind == ArgKind::Rest) {
        return Err(format!("{} has a rest argument that is not its last positional", at()));
    }
    if command.default && above.len() > 1 {
        return Err(format!("{} is marked default but is not a top-level command", at()));
    }
    let mut path = above.to_vec();
    path.push(command);
    for (index, child) in command.children.iter().enumerate() {
        let names = || std::iter::once(&child.name).chain(&child.aliases);
        if command.children[..index].iter().any(|other| names().any(|name| other.is_named(name))) {
            return Err(format!("{} has two subcommands named '{}'", at(), child.name));
        }
        check(child, &path, program)?;
    }
    Ok(())
}

/// Tells whether a path is a file the user may execute.
///
/// # Arguments
/// * `path` - the candidate.
///
/// # Returns
/// `true` for a regular file with an execute bit (any file on non-Unix systems).
fn executable(path: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

/// Resolves the words of `help <command>...` to a command path.
///
/// # Arguments
/// * `root` - the host's root command.
/// * `words` - command names or aliases, from the top down.
///
/// # Returns
/// The path from the root; the root alone for no words.
///
/// # Errors
/// A usage error naming the first word that is not a subcommand.
fn help_path<'a>(root: &'a Command, words: &[String]) -> Result<Vec<&'a Command>, Failure> {
    let mut path = vec![root];
    for word in words {
        let parent = path[path.len() - 1];
        let child = parent
            .children
            .iter()
            .find(|child| child.name == *word || child.aliases.contains(word))
            .ok_or_else(|| Failure::usage(format!("no command '{word}'")))?;
        path.push(child);
    }
    Ok(path)
}
