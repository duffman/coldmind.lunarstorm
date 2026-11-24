//! Terminal capabilities: color depth per stream, styling pens, width and cursor state.
//!
//! Decisions:
//! - Color is decided once at startup, separately for stdout and stderr, so
//!   `lunarstorm app.js > log.txt` keeps colored diagnostics on the terminal and a
//!   plain log file.
//! - Precedence, highest first: `--color`, `FORCE_COLOR`, `NO_COLOR`, then "is the
//!   stream a terminal and is `TERM` not `dumb`". This is Node's order.
//! - Three color tiers. Truecolor needs `COLORTERM=truecolor|24bit`; a `TERM`
//!   containing `256` gets the 256-color cube; anything else gets the 16 named
//!   colors. Gradients degrade to one named color on the lowest tier.
//! - Every styled run ends in a full reset (`ESC[0m`); runs are never nested.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Once, OnceLock};

/// The value of `--color`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorChoice {
    /// Decide from the environment and the stream.
    Auto,
    /// Color even when the stream is not a terminal.
    Always,
    /// Never color.
    Never,
}

impl ColorChoice {
    /// Parses a `--color` value.
    ///
    /// # Arguments
    /// * `text` - the value as written.
    ///
    /// # Returns
    /// The choice, or `None` when the value is not `auto`, `always` or `never`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

/// How many colors a stream can show. The discriminants are what JS sees in `ops.color`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Depth {
    /// No escape sequences at all.
    Plain = 0,
    /// The 16 named colors.
    Basic = 1,
    /// The 256-color palette.
    Indexed = 2,
    /// 24-bit color.
    True = 3,
}

/// A 24-bit color: red, green, blue.
pub type Rgb = (u8, u8, u8);

/// Brand gradient start.
pub const VIOLET: Rgb = (167, 139, 250);
/// Brand gradient end; also the accent for option names and paths.
pub const CYAN: Rgb = (34, 211, 238);
/// Error accent.
pub const RED: Rgb = (248, 113, 113);

/// Hides the cursor.
const HIDE_CURSOR: &[u8] = b"\x1b[?25l";
/// Shows the cursor.
const SHOW_CURSOR: &[u8] = b"\x1b[?25h";

/// Color depth of stdout and stderr, set once by [`init`].
static DEPTHS: OnceLock<[Depth; 2]> = OnceLock::new();
/// Whether a live region (progress bars, spinners) currently hides the cursor.
static LIVE: AtomicBool = AtomicBool::new(false);

/// Detects the color depth of both streams. Call once, before any output.
///
/// # Arguments
/// * `choice` - the `--color` value.
pub fn init(choice: ColorChoice) {
    let _ = DEPTHS.set([detect(choice, 1), detect(choice, 2)]);
}

/// # Arguments
/// * `fd` - 1 for stdout, 2 for stderr.
///
/// # Returns
/// The stream's color depth; [`Depth::Plain`] before [`init`].
pub fn depth(fd: i32) -> Depth {
    DEPTHS.get().map_or(Depth::Plain, |depths| depths[usize::from(fd == 2)])
}

/// Applies the precedence rules in the module docs to one stream.
///
/// # Arguments
/// * `choice` - the `--color` value.
/// * `fd` - the stream's file descriptor.
///
/// # Returns
/// The depth to use for that stream.
fn detect(choice: ColorChoice, fd: i32) -> Depth {
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let best = || match var("COLORTERM").as_deref() {
        Some("truecolor" | "24bit") => Depth::True,
        _ if var("TERM").is_some_and(|term| term.contains("256")) => Depth::Indexed,
        _ => Depth::Basic,
    };
    match choice {
        ColorChoice::Never => Depth::Plain,
        ColorChoice::Always => best(),
        ColorChoice::Auto => match var("FORCE_COLOR").as_deref() {
            Some("0" | "false") => Depth::Plain,
            Some("1" | "true") => Depth::Basic,
            Some("2") => Depth::Indexed,
            Some("3") => Depth::True,
            Some(_) => best(),
            None if var("NO_COLOR").is_some() || !is_tty(fd) || var("TERM").as_deref() == Some("dumb") => {
                Depth::Plain
            }
            None => best(),
        },
    }
}

/// # Arguments
/// * `fd` - a file descriptor.
///
/// # Returns
/// Whether it refers to a terminal.
pub fn is_tty(fd: i32) -> bool {
    // SAFETY: `isatty` only inspects the descriptor.
    unsafe { libc::isatty(fd) == 1 }
}

/// # Returns
/// The terminal width in columns: stderr's window size, else `COLUMNS`, else 80.
pub fn columns() -> u16 {
    // SAFETY: `winsize` is plain data and the ioctl only writes into it.
    let size = unsafe {
        let mut size: libc::winsize = std::mem::zeroed();
        (libc::ioctl(2, libc::TIOCGWINSZ, &mut size) == 0).then_some(size.ws_col)
    };
    size.filter(|&cols| cols > 0)
        .or_else(|| std::env::var("COLUMNS").ok()?.parse().ok())
        .unwrap_or(80)
}

/// Styles text for a stream of a given depth; a no-op on a plain stream.
#[derive(Clone, Copy)]
pub struct Pen(pub Depth);

impl Pen {
    /// Wraps text in SGR codes.
    ///
    /// # Arguments
    /// * `codes` - SGR parameters, e.g. `"1;31"`.
    /// * `text` - the text to style.
    ///
    /// # Returns
    /// The styled text, or `text` unchanged on a plain stream.
    pub fn sgr(self, codes: &str, text: &str) -> String {
        match self.0 {
            Depth::Plain => text.to_owned(),
            _ => format!("\x1b[{codes}m{text}\x1b[0m"),
        }
    }

    /// Converts a color to the SGR foreground parameter this depth supports.
    ///
    /// # Arguments
    /// * `color` - the wanted color.
    ///
    /// # Returns
    /// `38;2;r;g;b`, `38;5;n` or a bright named color (`90`-`97`). The named color
    /// keeps the channels within 75% of the strongest one, which maps the palette
    /// above to bright blue, cyan and red.
    pub fn fg(self, (r, g, b): Rgb) -> String {
        match self.0 {
            Depth::True => format!("38;2;{r};{g};{b}"),
            Depth::Indexed => {
                let cube = |c: u8| (u16::from(c) * 5 + 127) / 255;
                format!("38;5;{}", 16 + 36 * cube(r) + 6 * cube(g) + cube(b))
            }
            _ => {
                let max = u16::from(r.max(g).max(b));
                let bit = |c: u8, value: u8| if u16::from(c) * 4 > max * 3 { value } else { 0 };
                format!("{}", 90 + bit(r, 1) + bit(g, 2) + bit(b, 4))
            }
        }
    }

    /// Colors text with one foreground color.
    ///
    /// # Arguments
    /// * `color` - the color.
    /// * `text` - the text.
    ///
    /// # Returns
    /// The styled text.
    pub fn rgb(self, color: Rgb, text: &str) -> String {
        self.sgr(&self.fg(color), text)
    }

    /// Colors bold text with a left-to-right gradient.
    ///
    /// # Arguments
    /// * `text` - the text.
    /// * `from` - color of the first character.
    /// * `to` - color of the last character.
    ///
    /// # Returns
    /// The styled text; one flat color on the 16-color tier.
    pub fn gradient(self, text: &str, from: Rgb, to: Rgb) -> String {
        if matches!(self.0, Depth::Plain | Depth::Basic) {
            return self.sgr(&format!("1;{}", self.fg(from)), text);
        }
        let last = text.chars().count().saturating_sub(1).max(1) as i32;
        let mix = |a: u8, b: u8, i: i32| (i32::from(a) + (i32::from(b) - i32::from(a)) * i / last) as u8;
        let mut out = String::from("\x1b[1m");
        for (i, ch) in text.chars().enumerate() {
            let i = i as i32;
            let color = (mix(from.0, to.0, i), mix(from.1, to.1, i), mix(from.2, to.2, i));
            out.push_str(&format!("\x1b[{}m{ch}", self.fg(color)));
        }
        out + "\x1b[0m"
    }
}

/// Writes raw bytes to stderr with `write(2)`, which is async-signal-safe.
///
/// # Arguments
/// * `bytes` - what to write.
fn write_stderr(bytes: &[u8]) {
    // SAFETY: the pointer and length describe a live slice.
    unsafe { libc::write(2, bytes.as_ptr().cast(), bytes.len()) };
}

/// Enters or leaves live mode, in which the cursor is hidden while bars redraw.
///
/// Decision: the first entry installs handlers for SIGINT, SIGTERM and SIGHUP so
/// Ctrl-C during a progress bar does not leave the terminal without a cursor.
/// Scripts that never draw a live region keep the default signal dispositions.
///
/// # Arguments
/// * `on` - `true` to hide the cursor, `false` to show it again.
pub fn live(on: bool) {
    static HOOKED: Once = Once::new();
    if !on {
        return restore();
    }
    HOOKED.call_once(|| {
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: `on_signal` only calls async-signal-safe functions.
            unsafe { libc::signal(signal, on_signal as extern "C" fn(i32) as libc::sighandler_t) };
        }
    });
    LIVE.store(true, Ordering::SeqCst);
    write_stderr(HIDE_CURSOR);
}

/// Shows the cursor again if live mode hid it. Called on every exit path.
pub fn restore() {
    if LIVE.swap(false, Ordering::SeqCst) {
        write_stderr(SHOW_CURSOR);
    }
}

/// Signal handler: restores the cursor and exits with the conventional status.
///
/// # Arguments
/// * `signal` - the signal number.
extern "C" fn on_signal(signal: i32) {
    restore();
    // SAFETY: `_exit` is async-signal-safe and does not return.
    unsafe { libc::_exit(128 + signal) }
}
