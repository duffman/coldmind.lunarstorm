//! Help screens built from the command specs and rendered by `lunarstorm-cliui`.
//!
//! Decision: the host writes the content (usage lines, rows for commands, arguments
//! and options) from the same specs it parses with, so help never disagrees with
//! the parser; layout and color stay in `lunarstorm-cliui::screen`.

use lunarstorm_cli_contract::{Arg, ArgKind, Command, Section};
use lunarstorm_cliui::screen::{self, Brand};
use lunarstorm_cliui::term::Pen;

/// The host's rows for `--help` and `--version`, shown in every OPTIONS section.
const HOST_OPTIONS: [(&str, &str); 2] = [("-h, --help", "print help"), ("-v, --version", "print the version")];

/// Renders owned sections with `lunarstorm-cliui`.
///
/// # Arguments
/// * `pen` - the target stream's pen.
/// * `brand` - the identity at the top of the screen.
/// * `sections` - the sections, top to bottom; empty ones are skipped.
///
/// # Returns
/// The screen.
pub(crate) fn render(pen: Pen, brand: &Brand, sections: &[Section]) -> String {
    let sections: Vec<&Section> = sections.iter().filter(|(_, rows)| !rows.is_empty()).collect();
    let rows: Vec<Vec<(&str, &str)>> =
        sections.iter().map(|(_, rows)| rows.iter().map(|(left, right)| (left.as_str(), right.as_str())).collect()).collect();
    let borrowed: Vec<screen::Section> =
        sections.iter().zip(&rows).map(|((title, _), rows)| (title.as_str(), rows.as_slice())).collect();
    screen::help(pen, brand, &borrowed)
}

/// Builds the top-level help screen.
///
/// # Arguments
/// * `program` - the program name, e.g. `lunarstorm`.
/// * `root` - the host's root command.
/// * `extra` - the packages' own sections.
/// * `externals` - names of `lunarstorm-<name>` programs found on `PATH`.
///
/// # Returns
/// The sections.
pub(crate) fn top(program: &str, root: &Command, extra: &[Section], externals: &[String]) -> Vec<Section> {
    let mut usage = vec![(format!("{program} [options] <command> [args...]"), String::new())];
    if let Some(default) = root.children.iter().find(|child| child.default) {
        usage.push((format!("{program} {}", usage_tail(default)), String::new()));
    }
    let mut commands = command_rows(root);
    commands.extend(externals.iter().map(|name| (name.clone(), format!("external: {program}-{name}"))));
    let mut sections = vec![
        ("USAGE".to_owned(), usage),
        ("COMMANDS".to_owned(), commands),
        ("OPTIONS".to_owned(), option_rows(&root.args.iter().collect::<Vec<_>>())),
        (
            "ENVIRONMENT".to_owned(),
            vec![
                ("NO_COLOR".to_owned(), "disable color".to_owned()),
                ("FORCE_COLOR".to_owned(), "0 off, 1 basic, 2 256 colors, 3 truecolor".to_owned()),
            ],
        ),
    ];
    sections.extend(extra.iter().cloned());
    sections
}

/// Builds the help screen of one command.
///
/// # Arguments
/// * `program` - the program name.
/// * `path` - the commands from the host's root down to the one described.
///
/// # Returns
/// The sections.
pub(crate) fn command(program: &str, path: &[&Command]) -> Vec<Section> {
    let leaf = path[path.len() - 1];
    let names: Vec<&str> = path[1..].iter().map(|command| command.name.as_str()).collect();
    let arguments = leaf
        .args
        .iter()
        .filter(|arg| matches!(arg.kind, ArgKind::Positional | ArgKind::Rest))
        .map(|arg| (arg.shown_value(), annotated(arg)))
        .collect();
    // The leaf's own options, then the globals it inherits, innermost first.
    let mut options: Vec<&Arg> = leaf.args.iter().collect();
    for ancestor in path[..path.len() - 1].iter().rev() {
        options.extend(ancestor.args.iter().filter(|arg| arg.global));
    }
    let mut sections = vec![
        ("USAGE".to_owned(), vec![(format!("{program} {} {}", names.join(" "), usage_tail(leaf)), String::new())]),
        ("COMMANDS".to_owned(), command_rows(leaf)),
        ("ARGUMENTS".to_owned(), arguments),
        ("OPTIONS".to_owned(), option_rows(&options)),
    ];
    if let Some(aliases) = (!leaf.aliases.is_empty()).then(|| leaf.aliases.join(", ")) {
        sections.push(("ALIASES".to_owned(), vec![(aliases, String::new())]));
    }
    sections.push(("EXAMPLES".to_owned(), leaf.examples.iter().map(|example| (example.clone(), String::new())).collect()));
    sections
}

/// The part of a usage line after the command names.
///
/// # Arguments
/// * `command` - the command.
///
/// # Returns
/// `[options]` when it has any, its positionals' placeholders, and `<command>` when it
/// has children (in brackets when it also runs on its own).
fn usage_tail(command: &Command) -> String {
    let mut parts = vec!["[options]".to_owned()];
    for arg in &command.args {
        match arg.kind {
            ArgKind::Positional if !arg.required => parts.push(format!("[{}]", arg.shown_value())),
            ArgKind::Positional | ArgKind::Rest => parts.push(arg.shown_value()),
            _ => {}
        }
    }
    if !command.children.is_empty() {
        parts.push(if command.run.is_some() { "[<command>]".to_owned() } else { "<command>".to_owned() });
    }
    parts.join(" ")
}

/// Rows for a command's visible children.
///
/// # Arguments
/// * `command` - the parent.
///
/// # Returns
/// `(name, about)` per child; the default command is marked.
fn command_rows(command: &Command) -> Vec<(String, String)> {
    command
        .children
        .iter()
        .map(|child| {
            let about = if child.default { format!("{} (default)", child.about) } else { child.about.clone() };
            (child.name.clone(), about)
        })
        .collect()
}

/// Rows for flags and options, followed by the host's `--help` and `--version`.
///
/// # Arguments
/// * `args` - the arguments; positionals among them are skipped.
///
/// # Returns
/// `(-s, --name <value>, about)` rows.
fn option_rows(args: &[&Arg]) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = args
        .iter()
        .filter(|arg| matches!(arg.kind, ArgKind::Flag | ArgKind::Option))
        .map(|arg| {
            let short = arg.short.map_or_else(|| "    ".to_owned(), |short| format!("-{short}, "));
            let value = if arg.kind == ArgKind::Option { format!(" {}", arg.shown_value()) } else { String::new() };
            (format!("{short}--{}{value}", arg.name), annotated(arg))
        })
        .collect();
    rows.extend(HOST_OPTIONS.iter().map(|(left, right)| ((*left).to_owned(), (*right).to_owned())));
    rows
}

/// An argument's help line with its default and env variable.
///
/// # Arguments
/// * `arg` - the argument.
///
/// # Returns
/// `about [default: x] [env: X]`.
fn annotated(arg: &Arg) -> String {
    let mut text = arg.about.clone();
    if let Some(default) = &arg.default {
        text.push_str(&format!(" [default: {default}]"));
    }
    if let Some(env) = &arg.env {
        text.push_str(&format!(" [env: {env}]"));
    }
    text
}
