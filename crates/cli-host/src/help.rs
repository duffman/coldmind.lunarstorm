//! Help screens built from the command specs and rendered by `lunarstorm-cliui`.
//!
//! Decision: the host writes the content (usage lines, rows for commands, arguments
//! and options) from the same specs it parses with, so help never disagrees with
//! the parser; layout and color stay in `lunarstorm-cliui::screen`.

use lunarstorm_cli_contract::{Arg, ArgKind, Command, Section};
use lunarstorm_cliui::screen::{self, Brand};
use lunarstorm_cliui::term::Pen;

use crate::Group;

/// The host's rows for `--help` and `--version`, shown in every OPTIONS section.
const HOST_OPTIONS: [(&str, &str); 2] = [("-h, --help", "print help"), ("-v, --version", "print the version")];

/// One block of the top-level command table: its title and `(command, arguments,
/// description)` rows.
type Block = (String, Vec<(String, String, String)>);

/// One owned part of a help screen, as [`screen::Part`] borrows it.
pub(crate) enum Part {
    /// A two-column section.
    Rows(Section),
    /// A command table; `lunarstorm-cliui` gives each its own palette color.
    Table(Block),
}

/// Renders owned parts with `lunarstorm-cliui`.
///
/// # Arguments
/// * `pen` - the target stream's pen.
/// * `brand` - the identity at the top of the screen.
/// * `parts` - the sections and tables, top to bottom; empty ones are skipped.
///
/// # Returns
/// The screen.
pub(crate) fn render(pen: Pen, brand: &Brand, parts: &[Part]) -> String {
    let parts: Vec<&Part> = parts
        .iter()
        .filter(|part| match part {
            Part::Rows((_, rows)) => !rows.is_empty(),
            Part::Table((_, rows)) => !rows.is_empty(),
        })
        .collect();
    // The borrowed rows first, then the parts that point into them.
    let pairs: Vec<Vec<(&str, &str)>> = parts
        .iter()
        .map(|part| match part {
            Part::Rows((_, rows)) => rows.iter().map(|(left, right)| (left.as_str(), right.as_str())).collect(),
            Part::Table(_) => Vec::new(),
        })
        .collect();
    let triples: Vec<Vec<screen::Row>> = parts
        .iter()
        .map(|part| match part {
            Part::Table((_, rows)) => rows.iter().map(|(name, args, about)| (name.as_str(), args.as_str(), about.as_str())).collect(),
            Part::Rows(_) => Vec::new(),
        })
        .collect();
    let borrowed: Vec<screen::Part> = parts
        .iter()
        .zip(pairs.iter().zip(&triples))
        .map(|(part, (pairs, triples))| match part {
            Part::Rows((title, _)) => screen::Part::Rows((title.as_str(), pairs.as_slice())),
            Part::Table((title, _)) => screen::Part::Table((title.as_str(), triples.as_slice())),
        })
        .collect();
    screen::help(pen, brand, &borrowed)
}

/// Builds the top-level help screen.
///
/// # Arguments
/// * `program` - the program name, e.g. `lunarstorm`.
/// * `root` - the host's root command.
/// * `groups` - one block per package.
/// * `extra` - the packages' own sections.
/// * `prefix` - the prefix of external commands, e.g. `lunarstorm`.
/// * `externals` - names of `<prefix>-<name>` programs found on `PATH`.
///
/// # Returns
/// The parts: USAGE, one table per package, the COMMANDS table, then `extra`.
pub(crate) fn top(
    program: &str,
    prefix: &str,
    root: &Command,
    groups: &[Group],
    extra: &[Section],
    externals: &[String],
) -> Vec<Part> {
    let mut usage = vec![(format!("{program} [options] <command> [args...]"), String::new())];
    if let Some(default) = root.children.iter().find(|child| child.default) {
        usage.push((format!("{program} {}", usage_tail(default)), String::new()));
    }
    let mut blocks: Vec<Block> = groups
        .iter()
        .map(|group| {
            let title = if group.headline.is_empty() { group.title.clone() } else { format!("{} — {}", group.title, group.headline) };
            let rows = group
                .commands
                .iter()
                .filter_map(|name| root.children.iter().find(|child| &child.name == name))
                .map(|child| {
                    let about = if child.default { format!("{} (default)", child.about) } else { child.about.clone() };
                    (child.name.clone(), arguments(child), about)
                })
                .collect();
            (title, rows)
        })
        .collect();
    // Fix: the label used the program name, so `lunar` listed `lunar-hello` for the
    // `lunarstorm-hello` it runs. Decision: label with the prefix the lookup uses.
    // The host's own commands and the external programs, as the COMMANDS list showed
    // them before the screen was split per package.
    let mut host: Vec<(String, String, String)> = root
        .children
        .iter()
        .filter(|child| !groups.iter().any(|group| group.commands.contains(&child.name)))
        .map(|child| (child.name.clone(), arguments(child), child.about.clone()))
        .collect();
    host.extend(externals.iter().map(|name| (name.clone(), String::new(), format!("external: {prefix}-{name}"))));
    blocks.push(("COMMANDS".to_owned(), host));
    // Decision: the blocks go to `lunarstorm-cliui` as tables, which aligns the three
    // columns across them and colors each block from one palette. Joining the
    // command and arguments into one padded column here was dropped: it left no way
    // to color the arguments apart from the command.
    let mut parts = vec![Part::Rows(("USAGE".to_owned(), usage))];
    parts.extend(blocks.into_iter().map(Part::Table));
    // Decision: the top-level screen is the package blocks only; the OPTIONS and
    // ENVIRONMENT sections are not shown here (`--color`, `NO_COLOR` and
    // `FORCE_COLOR` still work, and every command's own help lists its options).
    parts.extend(extra.iter().cloned().map(Part::Rows));
    parts
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
    parts.extend(operands(command));
    parts.join(" ")
}

/// The words after a command's options in its usage line.
///
/// # Arguments
/// * `command` - the command.
///
/// # Returns
/// Its positionals' placeholders (optional ones in brackets), then `<command>` when it
/// has children, in brackets when it also runs on its own.
fn operands(command: &Command) -> Vec<String> {
    let mut parts = Vec::new();
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
    parts
}

/// The arguments column of a top-level command row.
///
/// # Arguments
/// * `command` - the command.
///
/// # Returns
/// `[options]` when the command declares flags or options of its own, its
/// positionals' placeholders (optional ones in brackets), and `<command>` when it has
/// children (in brackets when it also runs on its own). Empty when it takes nothing.
fn arguments(command: &Command) -> String {
    let mut parts = Vec::new();
    if command.args.iter().any(|arg| matches!(arg.kind, ArgKind::Flag | ArgKind::Option)) {
        parts.push("[options]".to_owned());
    }
    parts.extend(operands(command));
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
