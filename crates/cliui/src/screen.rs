//! Static screens: the brand mark, help, and usage errors with a suggestion.
//!
//! Decision: the tool supplies its name, version and help rows as data
//! ([`Brand`], [`Section`]); layout and color live here, so every Lunarstorm tool
//! gets the same look.

use crate::term::{Pen, Rgb, CYAN, RED, VIOLET};

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

/// Colors of a help screen's tables, one per table in order, repeating after the
/// last.
///
/// Decision: six steps along the brand gradient ([`VIOLET`] to [`CYAN`], then one
/// teal past it), so every block has its own color and all of them match the mark.
pub const PALETTE: [Rgb; 6] = [(167, 139, 250), (129, 140, 248), (96, 165, 250), (56, 189, 248), (34, 211, 238), (45, 212, 191)];

/// Percent of each channel a table's arguments column keeps of the table's color.
const SHADE: u16 = 55;

/// One row of a three-column table: command, arguments, description.
pub type Row<'a> = (&'a str, &'a str, &'a str);

/// A three-column table with a title: the title and its rows.
pub type Table<'a> = (&'a str, &'a [Row<'a>]);

/// One part of a help screen.
pub enum Part<'a> {
    /// A two-column section in the accent color.
    Rows(Section<'a>),
    /// A command table in its own palette color.
    Table(Table<'a>),
}

/// Darkens a color for a table's arguments column.
///
/// # Arguments
/// * `color` - the table's color.
///
/// # Returns
/// The color with each channel at [`SHADE`] percent.
fn shade((r, g, b): Rgb) -> Rgb {
    let scale = |c: u8| u8::try_from(u16::from(c) * SHADE / 100).unwrap_or(c);
    (scale(r), scale(g), scale(b))
}

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
/// Tables share their column widths, so the columns line up from one table to the
/// next. Widths are measured on the plain text, before any color is added. Each
/// table takes the next [`PALETTE`] color for its title and commands; its arguments
/// are faint and in a darker shade of that color.
///
/// # Arguments
/// * `pen` - the target stream's pen.
/// * `identity` - the tool's identity.
/// * `parts` - the sections and tables, top to bottom.
///
/// # Returns
/// The screen, newline-terminated.
pub fn help(pen: Pen, identity: &Brand, parts: &[Part]) -> String {
    let rows = || parts.iter().filter_map(|part| if let Part::Table((_, rows)) = part { Some(*rows) } else { None }).flatten();
    let name_width = rows().map(|(name, _, _)| name.chars().count()).max().unwrap_or(0);
    let args_width = rows().map(|(_, args, _)| args.chars().count()).max().unwrap_or(0);
    let mut out = format!("\n  {}\n  {}\n", brand(pen, identity), pen.sgr("2", identity.tagline));
    let mut tables = 0;
    for part in parts {
        match part {
            Part::Rows((title, rows)) => {
                out.push_str(&format!("\n  {}\n", pen.sgr("1", title)));
                for (left, right) in *rows {
                    // Fix: a label of LEFT_WIDTH columns or more got no padding and ran
                    // into its description. Decision: such a label keeps a two-space gap
                    // (the table gap) and its description starts after it, off the column.
                    let width = LEFT_WIDTH.max(left.chars().count() + 2);
                    let left = if right.is_empty() { (*left).to_owned() } else { format!("{left:<width$}") };
                    out.push_str(&format!("    {}{right}\n", pen.rgb(CYAN, &left)));
                }
            }
            Part::Table((title, rows)) => {
                let color = PALETTE[tables % PALETTE.len()];
                tables += 1;
                out.push_str(&format!("\n  {}\n", pen.sgr(&format!("1;{}", pen.fg(color)), title)));
                for (name, args, about) in *rows {
                    let name = pen.rgb(color, &format!("{name:<name_width$}"));
                    let args = pen.sgr(&format!("2;{}", pen.fg(shade(color))), &format!("{args:<args_width$}"));
                    out.push_str(&format!("    {name}  {args}  {about}\n"));
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::Depth;

    /// Each table takes the next palette color for its title and commands, and its
    /// arguments are faint in a darker shade of that color.
    #[test]
    fn each_table_has_its_own_palette_color_and_faint_darker_arguments() {
        let identity = Brand { mark: "m", version: "1", tagline: "t" };
        let first: &[Row] = &[("run", "<file>", "runs")];
        let second: &[Row] = &[("build", "[options]", "builds")];
        let screen = help(Pen(Depth::True), &identity, &[Part::Table(("A", first)), Part::Table(("B", second))]);
        let fg = |(r, g, b): Rgb| format!("38;2;{r};{g};{b}");
        assert!(screen.contains(&format!("\x1b[1;{}mA\x1b[0m", fg(PALETTE[0]))), "{screen:?}");
        assert!(screen.contains(&format!("\x1b[1;{}mB\x1b[0m", fg(PALETTE[1]))), "{screen:?}");
        assert!(screen.contains(&format!("\x1b[2;{}m<file>", fg(shade(PALETTE[0])))), "{screen:?}");
        assert!(screen.contains(&format!("\x1b[2;{}m[options]", fg(shade(PALETTE[1])))), "{screen:?}");
    }

    /// A row label as wide as the left column or wider still has a gap before its
    /// description (it ran into it: `--template <product/name|folder>a template`).
    #[test]
    fn a_row_label_wider_than_the_left_column_keeps_a_gap() {
        let identity = Brand { mark: "m", version: "1", tagline: "t" };
        let wide = "w".repeat(LEFT_WIDTH);
        let rows: &[(&str, &str)] = &[(&wide, "about")];
        let screen = help(Pen(Depth::Plain), &identity, &[Part::Rows(("Options", rows))]);
        assert!(screen.contains(&format!("{wide}  about")), "{screen}");
    }

    /// The columns line up across tables, and a plain stream gets no escape codes.
    #[test]
    fn table_columns_line_up_across_tables_on_a_plain_stream() {
        let identity = Brand { mark: "m", version: "1", tagline: "t" };
        let first: &[Row] = &[("run", "<file>", "runs")];
        let second: &[Row] = &[("transpile", "[options] <file>", "transpiles")];
        let screen = help(Pen(Depth::Plain), &identity, &[Part::Table(("A", first)), Part::Table(("B", second))]);
        assert!(!screen.contains('\x1b'), "{screen:?}");
        let column = |text: &str| screen.lines().find_map(|line| line.find(text)).unwrap();
        assert_eq!(column("runs"), column("transpiles"), "{screen}");
        assert_eq!(column("<file>"), column("[options] <file>"), "{screen}");
    }
}
