//! Static screens: the brand mark, help, and usage errors with a suggestion.
//!
//! Decision: the tool supplies its name, version and help rows as data
//! ([`Brand`], [`Section`]); layout and color live here, so every Lunarstorm tool
//! gets the same look.

use crate::term::{Pen, CYAN, RED, VIOLET};

/// Width the left column of a help row is padded to.
const LEFT_WIDTH: usize = 22;
/// Largest edit distance at which an option is still suggested.
const MAX_EDITS: usize = 2;

/// Identity of the tool, shown at the top of its screens.
pub struct Brand<'a> {
    /// The mark, drawn in the brand gradient, e.g. `☾ lunarstorm`.
    pub mark: &'a str,
    /// The version, without a `v` prefix.
    pub version: &'a str,
    /// One line describing the tool.
    pub tagline: &'a str,
}

/// One help section: a title and `(left column, description)` rows. A row with an
/// empty description is printed unpadded (usage lines, examples).
pub type Section<'a> = (&'a str, &'a [(&'a str, &'a str)]);

/// # Arguments
/// * `pen` - the target stream's pen.
/// * `brand` - the tool's identity.
///
/// # Returns
/// The mark in the violet-to-cyan gradient followed by the dimmed version.
pub fn brand(pen: Pen, brand: &Brand) -> String {
    format!("{} {}", pen.gradient(brand.mark, VIOLET, CYAN), pen.sgr("2", &format!("v{}", brand.version)))
}

/// Builds a help screen.
///
/// # Arguments
/// * `pen` - the target stream's pen.
/// * `identity` - the tool's identity.
/// * `sections` - the sections, top to bottom.
///
/// # Returns
/// The screen, newline-terminated.
pub fn help(pen: Pen, identity: &Brand, sections: &[Section]) -> String {
    let mut out = format!("\n  {}\n  {}\n", brand(pen, identity), pen.sgr("2", identity.tagline));
    for (title, rows) in sections {
        out.push_str(&format!("\n  {}\n", pen.sgr("1", title)));
        for (left, right) in *rows {
            let left = if right.is_empty() { (*left).to_owned() } else { format!("{left:<LEFT_WIDTH$}") };
            out.push_str(&format!("    {}{right}\n", pen.rgb(CYAN, &left)));
        }
    }
    out + "\n"
}

/// Builds a usage-error report.
///
/// # Arguments
/// * `pen` - stderr's pen.
/// * `message` - what is wrong.
/// * `hint` - the suggested fix, if any.
/// * `help_command` - the command that prints the help screen.
///
/// # Returns
/// The report, newline-terminated.
pub fn usage_error(pen: Pen, message: &str, hint: Option<&str>, help_command: &str) -> String {
    let mut out = format!("{} {}\n", pen.sgr(&format!("1;{}", pen.fg(RED)), "error:"), pen.sgr("1", message));
    if let Some(hint) = hint {
        out.push_str(&format!("  {} {hint}\n", pen.rgb(CYAN, "hint:")));
    }
    out + &format!("  {} {help_command}\n", pen.sgr("2", "usage:"))
}

/// Finds the known option closest to a mistyped one.
///
/// # Arguments
/// * `flag` - the unknown option.
/// * `options` - the options the tool accepts.
///
/// # Returns
/// The nearest option when it is at most [`MAX_EDITS`] edits away.
pub fn suggest<'a>(flag: &str, options: &[&'a str]) -> Option<&'a str> {
    options
        .iter()
        .map(|option| (distance(flag, option), *option))
        .filter(|(edits, _)| *edits <= MAX_EDITS)
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
