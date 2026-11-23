//! Renders uncaught errors for the terminal: a headline, a source code frame with a
//! caret at the failing column, and the dimmed stack.
//!
//! Decision: on a plain stream the report is exactly `<label> <engine text>`, the
//! format scripts and tests already parse; the code frame is terminal-only.

use crate::term::{self, Pen, CYAN, RED};

/// Lines of context shown above the failing line.
const BEFORE: usize = 2;
/// Lines of context shown below the failing line.
const AFTER: usize = 1;

/// Formats an uncaught error.
///
/// # Arguments
/// * `label` - `"Uncaught"` or `"Uncaught (in promise)"`.
/// * `text` - the engine's description: `Name: message`, then `    at ...` lines.
///
/// # Returns
/// The report, without a trailing newline.
pub fn uncaught(label: &str, text: &str) -> String {
    let pen = Pen(term::depth(2));
    if pen.0 == term::Depth::Plain {
        return format!("{label} {text}");
    }
    let is_frame = |line: &&str| line.trim_start().starts_with("at ");
    let head = text.lines().take_while(|line| !is_frame(line)).collect::<Vec<_>>().join("\n");
    let frames: Vec<&str> = text.lines().filter(is_frame).collect();
    let (name, message) = head.split_once(": ").unwrap_or((&head, ""));

    let bold_red = format!("1;{}", pen.fg(RED));
    let mut out = format!("\n{}", pen.sgr(&bold_red, &format!("✖ {label} {name}")));
    if !message.is_empty() {
        out.push_str(&format!(": {}", pen.sgr("1", message)));
    }
    out.push('\n');
    if let Some(frame) = frames.first().and_then(|frame| locate(frame)).and_then(|at| code_frame(pen, at)) {
        out.push_str(&frame);
    }
    for frame in frames {
        out.push_str(&pen.sgr("2", frame));
        out.push('\n');
    }
    out.truncate(out.trim_end().len());
    out
}

/// Extracts the source position from one stack line.
///
/// The engine writes `at name (path:line:col)` for calls and `at path:line:col` for
/// syntax errors.
///
/// # Arguments
/// * `frame` - one stack line.
///
/// # Returns
/// `(path, line, column)`, both 1-based, or `None` when the line has no position.
fn locate(frame: &str) -> Option<(&str, usize, usize)> {
    let at = frame.trim_start().strip_prefix("at ")?;
    let position = match at.strip_suffix(')') {
        Some(inner) => &inner[inner.rfind('(')? + 1..],
        None => at,
    };
    let mut parts = position.rsplitn(3, ':');
    let column = parts.next()?.parse().ok()?;
    let line = parts.next()?.parse().ok()?;
    Some((parts.next()?, line, column))
}

/// Draws the source lines around a position.
///
/// # Arguments
/// * `pen` - stderr's pen.
/// * `at` - `(path, line, column)` from [`locate`].
///
/// # Returns
/// The frame, newline-terminated, or `None` when the file cannot be read (`[eval]`
/// code, builtins) or the line does not exist.
fn code_frame(pen: Pen, (path, line, column): (&str, usize, usize)) -> Option<String> {
    let source = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = source.lines().collect();
    if line == 0 || line > lines.len() {
        return None;
    }
    let last = (line + AFTER).min(lines.len());
    let width = last.to_string().len();
    let shown = std::env::current_dir()
        .ok()
        .and_then(|dir| path.strip_prefix(dir.to_str()?)?.strip_prefix('/'))
        .unwrap_or(path);
    let red = pen.fg(RED);
    let rule = |text: &str| pen.sgr("2", text);

    let mut out = format!("{} {}\n", rule(&format!("{:width$} ╭─", "")), pen.rgb(CYAN, &format!("{shown}:{line}:{column}")));
    for number in line.saturating_sub(BEFORE).max(1)..=last {
        // Tabs become one space so the caret lines up under the reported column.
        let text = lines[number - 1].replace('\t', " ");
        if number == line {
            out.push_str(&format!("{} {} {text}\n", pen.sgr(&format!("1;{red}"), &format!("{number:width$}")), rule("│")));
            out.push_str(&format!("{} {}{}\n", rule(&format!("{:width$} │", "")), " ".repeat(column.saturating_sub(1)), pen.sgr(&format!("1;{red}"), "^")));
        } else {
            out.push_str(&format!("{} {}\n", rule(&format!("{number:width$} │")), rule(&text)));
        }
    }
    out.push_str(&format!("{}\n", rule(&format!("{:width$} ╰─", ""))));
    Some(out)
}
