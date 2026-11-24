//! Machine-readable views of the command tree: the `describe` JSON and shell
//! completion.
//!
//! Decision: completion is dynamic. The shell script only forwards the words typed so
//! far to `lunarstorm __complete`, which answers from the live tree, so mounting a
//! package never requires regenerating a completion file.

use lunarstorm_cli_contract::{Arg, ArgKind, Command, ValueType};
use serde_json::{json, Value};

/// Describes the whole tree as JSON.
///
/// # Arguments
/// * `program` - the program name.
/// * `version` - the program version.
/// * `root` - the host's root command.
///
/// # Returns
/// `{ program, version, options, commands }`; see `doc/cli.md` for the schema.
pub(crate) fn describe(program: &str, version: &str, root: &Command) -> Value {
    json!({
        "program": program,
        "version": version,
        "options": root.args.iter().map(arg).collect::<Vec<_>>(),
        "commands": root.children.iter().map(command).collect::<Vec<_>>(),
    })
}

/// Describes one command and its children.
///
/// # Arguments
/// * `command` - the command.
///
/// # Returns
/// The command's JSON object.
fn command(command: &Command) -> Value {
    json!({
        "name": command.name,
        "about": command.about,
        "aliases": command.aliases,
        "default": command.default,
        "options_first": command.options_first,
        "runs": command.run.is_some(),
        "args": command.args.iter().map(arg).collect::<Vec<_>>(),
        "commands": command.children.iter().map(self::command).collect::<Vec<_>>(),
        "examples": command.examples,
    })
}

/// Describes one argument.
///
/// # Arguments
/// * `arg` - the argument.
///
/// # Returns
/// The argument's JSON object.
fn arg(arg: &Arg) -> Value {
    let kind = match arg.kind {
        ArgKind::Flag => "flag",
        ArgKind::Option => "option",
        ArgKind::Positional => "positional",
        ArgKind::Rest => "rest",
    };
    let (value, choices) = match &arg.value {
        ValueType::Text => ("text", Vec::new()),
        ValueType::Path => ("path", Vec::new()),
        ValueType::Int => ("int", Vec::new()),
        ValueType::Choice(choices) => ("choice", choices.clone()),
    };
    json!({
        "name": arg.name,
        "kind": kind,
        "short": arg.short.map(String::from),
        "value": value,
        "choices": choices,
        "placeholder": arg.shown_value(),
        "about": arg.about,
        "default": arg.default,
        "env": arg.env,
        "required": arg.required,
        "global": arg.global,
        "ends_options": arg.ends_options,
    })
}

/// Lists completion candidates.
///
/// # Arguments
/// * `root` - the host's root command.
/// * `words` - the words after the program name up to the cursor; the last one is
///   the word being completed (empty for a new word).
///
/// # Returns
/// The candidates starting with the word being completed: the choices of the option
/// before it, options when it starts with `-`, else subcommand names. Empty when the
/// shell should complete files.
pub(crate) fn complete(root: &Command, words: &[String]) -> Vec<String> {
    let (current, done) = words.split_last().map_or(("", &[][..]), |(last, done)| (last.as_str(), done));
    let mut path = vec![root];
    let mut pending: Option<&Arg> = None;
    for word in done {
        if pending.take().is_some() || word == "=" {
            continue;
        }
        let scope = scope(&path);
        if let Some(long) = word.strip_prefix("--") {
            pending = scope.into_iter().find(|arg| arg.name == long && arg.kind == ArgKind::Option);
        } else if word.starts_with('-') {
            pending = scope.into_iter().find(|arg| arg.kind == ArgKind::Option && arg.short.is_some_and(|short| word.ends_with(short)));
        } else if let Some(child) = path[path.len() - 1].children.iter().find(|child| child.is_named(word)) {
            path.push(child);
        }
    }
    let candidates: Vec<String> = match pending {
        Some(Arg { value: ValueType::Choice(choices), .. }) => choices.clone(),
        Some(_) => Vec::new(),
        None if current.starts_with('-') => {
            let mut names: Vec<String> = scope(&path).iter().map(|arg| format!("--{}", arg.name)).collect();
            names.extend(["--help".to_owned(), "--version".to_owned()]);
            names
        }
        None => path[path.len() - 1].children.iter().map(|child| child.name.clone()).collect(),
    };
    candidates.into_iter().filter(|candidate| candidate.starts_with(current)).collect()
}

/// The flags and options in scope at the end of a path.
///
/// # Arguments
/// * `path` - the commands from the root down.
///
/// # Returns
/// The leaf's flags and options plus the globals above it.
fn scope<'a>(path: &[&'a Command]) -> Vec<&'a Arg> {
    let last = path.len() - 1;
    path.iter()
        .enumerate()
        .flat_map(|(level, command)| command.args.iter().filter(move |arg| level == last || arg.global))
        .filter(|arg| matches!(arg.kind, ArgKind::Flag | ArgKind::Option))
        .collect()
}

/// Builds a completion script.
///
/// # Arguments
/// * `program` - the program name the script completes.
/// * `shell` - `bash` or `zsh`.
///
/// # Returns
/// The script. zsh uses the same function through `bashcompinit`. `-o default` makes
/// bash fall back to file names when there are no candidates.
pub(crate) fn script(program: &str, shell: &str) -> String {
    let function = program.replace('-', "_");
    let bash = format!(
        "_{function}() {{\n  local IFS=$'\\n'\n  COMPREPLY=($({program} __complete \"${{COMP_WORDS[@]:1:COMP_CWORD}}\"))\n}}\ncomplete -o default -F _{function} {program}\n"
    );
    match shell {
        "zsh" => format!("autoload -U +X bashcompinit && bashcompinit\n{bash}"),
        _ => bash,
    }
}
