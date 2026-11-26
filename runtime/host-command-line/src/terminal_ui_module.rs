//! `lunarstorm:hostapi/ui` (also `lunarstorm:ui`) as natives: terminal styles,
//! progress bars and spinners.
//!
//! ```js
//! import { style, progress, spinner } from 'lunarstorm:ui';
//! ```
//!
//! Decisions:
//! - Replaces the source module `crates/cliui/js/ui.js` (archived); the output is
//!   byte for byte what it drew. No script runs for it.
//! - Bars and spinners draw on stderr, so a script's stdout stays clean for pipes.
//! - All live items share one region at the bottom of the terminal. Each line of it
//!   ends in a newline and the cursor rests below it, so anything else that prints
//!   (an uncaught error, the shell prompt) starts on a clean line.
//! - Redraws are throttled to one per [`FRAME_MS`]. `update()` redraws on its own
//!   when a frame is due, so bars advance inside synchronous loops too; a timer
//!   redraws while the script is waiting on I/O, which is what animates spinners.
//! - `console.*` output while items are live is printed above the region (the
//!   region is erased, the line printed, the region redrawn), through the
//!   console's live-region slot `live.wrap`.
//! - When stderr is not a terminal nothing is animated: each item prints exactly
//!   one line when it finishes, without escape sequences unless color is forced.
//! - The 16-color tier and NO_COLOR terminals get block glyphs for the bar, because
//!   filled and empty cells cannot be told apart by color there.
//! - The module's state (`ops`, `live`, the live items, the redraw timer) lives on
//!   one JS object, and every native that needs it is bound to it
//!   (`Function.prototype.bind`): closures given to the engine hold no engine
//!   handles, so the collector sees every reference.
//! - Printing goes through `ops.print`/`ops.flush`/`ops.now`, as the source module
//!   did, so stdout batching and the host's stream policy apply unchanged. The
//!   terminal width and the cursor come from [`term`] directly.

use lunarstorm_cliui::term;
use lunarstorm_event_loop_runtime::declared;
use lunarstorm_event_loop_runtime::vm::{Call, ErrorKind, Field, FnDecl, Item, Kind, ModuleDecl, Outcome, Param, Scope, Sig, Ty, Vm};

/// Milliseconds between redraws of the live region.
const FRAME_MS: f64 = 50.0;
/// Milliseconds each spinner frame is shown.
const SPIN_MS: f64 = 80.0;
/// Spinner animation frames.
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Bar gradient start (violet), matching the CLI brand.
const FROM: [f64; 3] = [167.0, 139.0, 250.0];
/// Bar gradient end (cyan).
const TO: [f64; 3] = [34.0, 211.0, 238.0];
/// Widest bar, in cells.
const MAX_BAR: f64 = 40.0;
/// Narrowest bar, in cells.
const MIN_BAR: f64 = 10.0;

// ---------------------------------------------------------------- declarations

/// A function taking text and returning it styled.
const TEXT_SIG: Sig = Sig { params: &[Param { name: "text", ty: Ty::Any, about: "the text" }], rest: None, returns: Ty::String, about: "Styles text." };
/// A function taking no argument and returning nothing.
const NOTHING_SIG: Sig = Sig { params: &[], rest: None, returns: Ty::Undefined, about: "" };
/// A function taking an optional summary message.
const MESSAGE_SIG: Sig = Sig {
    params: &[Param { name: "message", ty: Ty::Optional(&Ty::String), about: "text for the line; the label by default" }],
    rest: None,
    returns: Ty::Undefined,
    about: "",
};
/// An optional `[r, g, b]` color.
const COLOR: Ty = Ty::Optional(&Ty::Array(&Ty::Number));

/// The members of `style`.
const STYLE_FIELDS: &[Field] = &[
    Field { name: "enabled", ty: Ty::Bool, about: "Whether styling is active on this stream." },
    Field { name: "bold", ty: Ty::Function(&TEXT_SIG), about: "Bold." },
    Field { name: "dim", ty: Ty::Function(&TEXT_SIG), about: "Dim." },
    Field { name: "italic", ty: Ty::Function(&TEXT_SIG), about: "Italic." },
    Field { name: "underline", ty: Ty::Function(&TEXT_SIG), about: "Underlined." },
    Field { name: "red", ty: Ty::Function(&TEXT_SIG), about: "Bright red." },
    Field { name: "green", ty: Ty::Function(&TEXT_SIG), about: "Bright green." },
    Field { name: "yellow", ty: Ty::Function(&TEXT_SIG), about: "Bright yellow." },
    Field { name: "blue", ty: Ty::Function(&TEXT_SIG), about: "Bright blue." },
    Field { name: "magenta", ty: Ty::Function(&TEXT_SIG), about: "Bright magenta." },
    Field { name: "cyan", ty: Ty::Function(&TEXT_SIG), about: "Bright cyan." },
    Field { name: "gray", ty: Ty::Function(&TEXT_SIG), about: "Gray." },
    Field {
        name: "rgb",
        ty: Ty::Function(&Sig {
            params: &[Param { name: "r", ty: Ty::Number, about: "red, 0-255" }, Param { name: "g", ty: Ty::Number, about: "green, 0-255" }, Param { name: "b", ty: Ty::Number, about: "blue, 0-255" }],
            rest: None,
            returns: Ty::Function(&TEXT_SIG),
            about: "",
        }),
        about: "Colors text with a 24-bit color, degraded to what the terminal supports.",
    },
    Field {
        name: "gradient",
        ty: Ty::Function(&Sig {
            params: &[
                Param { name: "text", ty: Ty::Any, about: "the text" },
                Param { name: "from", ty: COLOR, about: "start color [r, g, b]; violet by default" },
                Param { name: "to", ty: COLOR, about: "end color [r, g, b]; cyan by default" },
            ],
            rest: None,
            returns: Ty::String,
            about: "",
        }),
        about: "Colors text with a left-to-right gradient.",
    },
];

/// The members of a progress bar.
const BAR_FIELDS: &[Field] = &[
    Field { name: "value", ty: Ty::Number, about: "Current value." },
    Field {
        name: "update",
        ty: Ty::Function(&Sig { params: &[Param { name: "value", ty: Ty::Number, about: "new value" }], rest: None, returns: Ty::Undefined, about: "" }),
        about: "Sets the value. Reaching `total` completes the bar.",
    },
    Field {
        name: "tick",
        ty: Ty::Function(&Sig { params: &[Param { name: "n", ty: Ty::Optional(&Ty::Number), about: "amount; default 1" }], rest: None, returns: Ty::Undefined, about: "" }),
        about: "Advances the value.",
    },
    Field { name: "done", ty: Ty::Function(&MESSAGE_SIG), about: "Completes the bar and leaves a summary line." },
    Field { name: "fail", ty: Ty::Function(&MESSAGE_SIG), about: "Stops the bar as failed and leaves a summary line." },
];

/// The members of a spinner.
const SPINNER_FIELDS: &[Field] = &[
    Field {
        name: "text",
        ty: Ty::Function(&Sig { params: &[Param { name: "next", ty: Ty::String, about: "new label" }], rest: None, returns: Ty::Undefined, about: "" }),
        about: "Changes the label.",
    },
    Field { name: "succeed", ty: Ty::Function(&MESSAGE_SIG), about: "Stops the spinner with a success mark." },
    Field { name: "fail", ty: Ty::Function(&MESSAGE_SIG), about: "Stops the spinner with a failure mark." },
    Field { name: "stop", ty: Ty::Function(&NOTHING_SIG), about: "Stops the spinner and leaves no line behind." },
];

/// `progress::<V>(options?)`.
const PROGRESS: FnDecl = FnDecl {
    name: "progress",
    sig: Sig {
        params: &[Param {
            name: "options",
            ty: Ty::Optional(&Ty::Object(&[
                Field { name: "total", ty: Ty::Optional(&Ty::Number), about: "value at which the bar is complete; default 100" },
                Field { name: "label", ty: Ty::Optional(&Ty::String), about: "text shown before the bar" },
                Field { name: "width", ty: Ty::Optional(&Ty::Number), about: "bar width in cells; fitted to the terminal by default" },
                Field { name: "bytes", ty: Ty::Optional(&Ty::Bool), about: "show values and rate as byte sizes" },
            ])),
            about: "settings",
        }],
        rest: None,
        returns: Ty::Object(BAR_FIELDS),
        about: "Starts a progress bar.",
    },
};

/// `spinner::<V>(label?)`.
const SPINNER: FnDecl = FnDecl {
    name: "spinner",
    sig: Sig {
        params: &[Param { name: "label", ty: Ty::Optional(&Ty::String), about: "text shown after the spinner" }],
        rest: None,
        returns: Ty::Object(SPINNER_FIELDS),
        about: "Starts a spinner for work of unknown length.",
    },
};

/// # Arguments
/// * `specifier` - the module's specifier.
///
/// # Returns
/// The module's declaration: `style`, `progress`, `spinner` and `default`.
pub fn declaration(specifier: &'static str) -> ModuleDecl {
    ModuleDecl {
        specifier,
        items: vec![
            Item::Value(Field { name: "style", ty: Ty::Object(STYLE_FIELDS), about: "Styles for text the script prints to stdout (`console.log`)." }),
            Item::Function(PROGRESS),
            Item::Function(SPINNER),
            Item::Value(Field { name: "default", ty: Ty::Object(&[]), about: "`{ style, progress, spinner }`." }),
        ],
        about: "Terminal styling, progress bars and spinners.",
    }
}

// ---------------------------------------------------------------- text, as JS formats it

/// `Math.round`: halves round up.
///
/// # Arguments
/// * `x` - the number.
///
/// # Returns
/// The nearest integer.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// `Math.min` of two numbers: `NaN` when either is.
///
/// # Arguments
/// * `a` - one number.
/// * `b` - the other.
///
/// # Returns
/// The smaller.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

/// `String(x)` for a number.
///
/// # Arguments
/// * `x` - the number.
///
/// # Returns
/// Its text: integers without a fraction, `NaN`, `Infinity`.
fn js_num(x: f64) -> String {
    if x.is_nan() {
        String::from("NaN")
    } else if x.is_infinite() {
        String::from(if x > 0.0 { "Infinity" } else { "-Infinity" })
    } else if x.fract() == 0.0 && x.abs() < 1e21 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

/// `x.toFixed(1)`: halves round up, as JS does.
///
/// # Arguments
/// * `x` - the number.
///
/// # Returns
/// The text with one decimal.
fn fixed1(x: f64) -> String {
    if !x.is_finite() {
        return js_num(x);
    }
    format!("{:.1}", js_round(x * 10.0) / 10.0)
}

/// `text.length`: UTF-16 code units.
///
/// # Arguments
/// * `text` - the text.
///
/// # Returns
/// Its JS length.
fn js_len(text: &str) -> f64 {
    text.encode_utf16().count() as f64
}

/// `text.padStart(width)` with spaces.
///
/// # Arguments
/// * `text` - the text.
/// * `width` - the wanted JS length.
///
/// # Returns
/// The padded text.
fn pad_start(text: &str, width: usize) -> String {
    let length = text.encode_utf16().count();
    format!("{}{text}", " ".repeat(width.saturating_sub(length)))
}

/// Converts a color to the SGR foreground parameter a color depth supports.
///
/// # Arguments
/// * `depth` - 1 basic, 2 256 colors, 3 truecolor.
/// * `color` - red, green, blue, 0-255.
///
/// # Returns
/// The SGR parameter.
fn fg(depth: i32, [r, g, b]: [f64; 3]) -> String {
    if depth == 3 {
        return format!("38;2;{};{};{}", js_num(r), js_num(g), js_num(b));
    }
    if depth == 2 {
        let cube = |c: f64| js_round((c / 255.0) * 5.0);
        return format!("38;5;{}", js_num(16.0 + 36.0 * cube(r) + 6.0 * cube(g) + cube(b)));
    }
    let max = r.max(g).max(b) * 0.75;
    let bit = |c: f64, value: i32| if c > max { value } else { 0 };
    format!("{}", 90 + bit(r, 1) + bit(g, 2) + bit(b, 4))
}

/// Styles for one stream.
#[derive(Clone, Copy)]
struct Ink(i32);

impl Ink {
    /// Wraps text in one SGR code.
    ///
    /// # Arguments
    /// * `code` - the SGR parameters.
    /// * `text` - the text.
    ///
    /// # Returns
    /// The styled text; the text itself on a plain stream.
    fn wrap(self, code: &str, text: &str) -> String {
        if self.0 != 0 {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    /// # Returns
    /// The text in bold.
    fn bold(self, text: &str) -> String {
        self.wrap("1", text)
    }

    /// # Returns
    /// The text dimmed.
    fn dim(self, text: &str) -> String {
        self.wrap("2", text)
    }

    /// # Returns
    /// The text in gray.
    fn gray(self, text: &str) -> String {
        self.wrap("90", text)
    }

    /// # Returns
    /// The text in bright green.
    fn green(self, text: &str) -> String {
        self.wrap("92", text)
    }

    /// # Returns
    /// The text in bright red.
    fn red(self, text: &str) -> String {
        self.wrap("91", text)
    }

    /// Colors text with a left-to-right gradient.
    ///
    /// # Arguments
    /// * `text` - the text.
    /// * `from` - the first character's color.
    /// * `to` - the last character's color.
    ///
    /// # Returns
    /// The styled text; one flat color below 256 colors.
    fn gradient(self, text: &str, from: [f64; 3], to: [f64; 3]) -> String {
        let chars: Vec<char> = text.chars().collect();
        if self.0 < 2 {
            return self.wrap(&fg(self.0, from), text);
        }
        let last = (chars.len() as f64 - 1.0).max(1.0);
        let mut out = String::new();
        for (i, ch) in chars.iter().enumerate() {
            let mix = |k: usize| js_round(from[k] + ((to[k] - from[k]) * i as f64) / last);
            out.push_str(&format!("\x1b[{}m{ch}", fg(self.0, [mix(0), mix(1), mix(2)])));
        }
        out + "\x1b[0m"
    }
}

/// `6.2 MB` for a byte count.
///
/// # Arguments
/// * `n` - the count.
///
/// # Returns
/// The size in decimal units.
fn bytes(mut n: f64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut unit = 0;
    while n >= 1000.0 && unit < UNITS.len() - 1 {
        n /= 1000.0;
        unit += 1;
    }
    let number = if unit > 0 { fixed1(n) } else { js_num(js_round(n)) };
    format!("{number} {}", UNITS[unit])
}

/// `0.4s`, `12s`, `3m07s` for a duration.
///
/// # Arguments
/// * `seconds` - the duration.
///
/// # Returns
/// The text.
fn duration(seconds: f64) -> String {
    if seconds < 10.0 {
        return fixed1(seconds) + "s";
    }
    if seconds < 60.0 {
        return js_num(js_round(seconds)) + "s";
    }
    let whole = js_round(seconds);
    format!("{}m{}s", js_num((whole / 60.0).floor()), pad_start(&js_num(whole % 60.0), 2).replace(' ', "0"))
}

/// Draws the bar itself.
///
/// # Arguments
/// * `ink` - stderr's styles.
/// * `fraction` - progress, 0 to 1.
/// * `width` - cells.
///
/// # Returns
/// The bar.
fn bar(ink: Ink, fraction: f64, width: f64) -> String {
    let exact = fraction * width;
    let full = exact.floor().max(0.0);
    let cells = |glyph: &str, n: f64| glyph.repeat(n.max(0.0) as usize);
    if ink.0 < 2 {
        return cells("█", full) + &cells("░", width - full);
    }
    let half = full < width && exact - full >= 0.5;
    let rest = width - full - if half { 1.0 } else { 0.0 };
    // The gradient spans the whole track, so the visible part keeps its colors as it grows.
    let count = width.max(0.0) as usize;
    let last = (count as f64 - 1.0).max(1.0);
    let cell = |i: usize, glyph: &str| {
        let mix = |k: usize| js_round(FROM[k] + ((TO[k] - FROM[k]) * i as f64) / last);
        format!("\x1b[{}m{glyph}", fg(ink.0, [mix(0), mix(1), mix(2)]))
    };
    let full = full as usize;
    let mut out: String = (0..full.min(count)).map(|i| cell(i, "━")).collect();
    if half {
        out.push_str(&cell(full, "╸"));
    }
    out + "\x1b[0m" + &ink.gray(&cells("━", rest))
}

// ---------------------------------------------------------------- engine helpers

/// What the module knows about the terminal, read once at the first import.
#[derive(Clone, Copy)]
struct Tty {
    /// stdout's color depth.
    out: i32,
    /// stderr's styles.
    ink: Ink,
    /// Whether stderr is a terminal, i.e. whether anything animates.
    animated: bool,
}

/// Calls `receiver[name](...args)`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `receiver` - the object.
/// * `name` - the method.
/// * `args` - the arguments.
///
/// # Returns
/// The result.
///
/// # Errors
/// What the method throws; a `TypeError` when it is not a function.
fn call_method<'s, V: Vm>(scope: &V::Scope<'s>, receiver: &V::Value<'s>, name: &str, args: &[V::Value<'s>]) -> Outcome<V::Value<'s>> {
    let function = scope.get(receiver, name)?;
    if scope.kind(&function) != Kind::Function {
        return Err(scope.throw_error(ErrorKind::Type, &format!("{name} is not a function")));
    }
    scope.call_with(&function, receiver, args)
}

/// Reads a number property; `NaN` when it is not a number.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `object` - the object.
/// * `key` - the property.
///
/// # Returns
/// The number.
///
/// # Errors
/// What the property read throws.
fn number<'s, V: Vm>(scope: &V::Scope<'s>, object: &V::Value<'s>, key: &str) -> Outcome<f64> {
    Ok(scope.as_number(&scope.get(object, key)?).unwrap_or(f64::NAN))
}

/// Reads a property as text, `String(value)`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `object` - the object.
/// * `key` - the property.
///
/// # Returns
/// The text.
///
/// # Errors
/// What the read or the conversion throws.
fn string<'s, V: Vm>(scope: &V::Scope<'s>, object: &V::Value<'s>, key: &str) -> Outcome<String> {
    scope.with_str(&scope.get(object, key)?, str::to_owned)
}

/// Whether a value is truthy, as JS tests it.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `value` - the value.
///
/// # Returns
/// `false` for undefined, null, false, 0, NaN and ''.
fn truthy<'s, V: Vm>(scope: &V::Scope<'s>, value: &V::Value<'s>) -> bool {
    match scope.kind(value) {
        Kind::Undefined | Kind::Null => false,
        Kind::Bool => scope.as_bool(value).unwrap_or(false),
        Kind::Number => scope.as_number(value).is_some_and(|n| n != 0.0 && !n.is_nan()),
        Kind::String => scope.with_str(value, |text| !text.is_empty()).unwrap_or(true),
        _ => true,
    }
}

/// The argument at `index`, or `undefined`.
///
/// # Arguments
/// * `call` - the call.
/// * `index` - the position.
///
/// # Returns
/// The argument.
fn arg<'a, V: Vm>(call: &Call<'a, V>, index: usize) -> V::Value<'a> {
    call.args().get(index).cloned().unwrap_or_else(|| call.scope().undefined())
}

/// Text of an optional argument: `String(value)`, or `fallback` when it is `undefined`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `value` - the argument.
/// * `fallback` - the default.
///
/// # Returns
/// The text.
///
/// # Errors
/// What the conversion throws.
fn text_or<'s, V: Vm>(scope: &V::Scope<'s>, value: &V::Value<'s>, fallback: &str) -> Outcome<String> {
    if scope.kind(value) == Kind::Undefined {
        Ok(fallback.to_owned())
    } else {
        scope.with_str(value, str::to_owned)
    }
}

/// Creates a native function bound to the module state (`this`) and, when given,
/// an item record (its first argument).
///
/// # Arguments
/// * `scope` - the context handle.
/// * `name` - the function's name.
/// * `state` - the module state.
/// * `record` - the item's record, if the function belongs to an item.
/// * `f` - the implementation.
///
/// # Returns
/// The bound function.
///
/// # Errors
/// [`lunarstorm_event_loop_runtime::vm::Thrown`] when allocation fails.
fn bound<'s, V: Vm, F>(scope: &V::Scope<'s>, name: &str, state: &V::Value<'s>, record: Option<&V::Value<'s>>, f: F) -> Outcome<V::Value<'s>>
where
    F: for<'a> Fn(&Call<'a, V>) -> Outcome<V::Value<'a>> + 'static,
{
    let function = scope.function(name, f)?;
    bind::<V>(scope, function, state, record)
}

/// `function.bind(state, record?)`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `function` - the function.
/// * `state` - the module state.
/// * `record` - the item's record, if any.
///
/// # Returns
/// The bound function.
///
/// # Errors
/// What `bind` throws.
fn bind<'s, V: Vm>(scope: &V::Scope<'s>, function: V::Value<'s>, state: &V::Value<'s>, record: Option<&V::Value<'s>>) -> Outcome<V::Value<'s>> {
    let mut args = vec![state.clone()];
    args.extend(record.cloned());
    call_method::<V>(scope, &function, "bind", &args)
}

/// `ops.now()`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `state` - the module state.
///
/// # Returns
/// Milliseconds on the host's monotonic clock.
///
/// # Errors
/// What `ops.now` throws.
fn now<'s, V: Vm>(scope: &V::Scope<'s>, state: &V::Value<'s>) -> Outcome<f64> {
    let ops = scope.get(state, "ops")?;
    Ok(scope.as_number(&call_method::<V>(scope, &ops, "now", &[])?).unwrap_or(f64::NAN))
}

/// `ops.print(2, text)`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `state` - the module state.
/// * `text` - what to write to stderr.
///
/// # Errors
/// What `ops.print` throws.
fn print<'s, V: Vm>(scope: &V::Scope<'s>, state: &V::Value<'s>, text: &str) -> Outcome<()> {
    let ops = scope.get(state, "ops")?;
    call_method::<V>(scope, &ops, "print", &[scope.int(2), scope.string(text)?]).map(drop)
}

// ---------------------------------------------------------------- the live region

/// Draws one item's line.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `state` - the module state.
/// * `record` - the item's record.
/// * `cols` - the terminal width.
/// * `ink` - stderr's styles.
///
/// # Returns
/// One line of the live region.
///
/// # Errors
/// What a property read throws.
fn render<'s, V: Vm>(scope: &V::Scope<'s>, state: &V::Value<'s>, record: &V::Value<'s>, cols: f64, ink: Ink) -> Outcome<String> {
    let started = number::<V>(scope, record, "started")?;
    let label = string::<V>(scope, record, "label")?;
    let elapsed = (now::<V>(scope, state)? - started) / 1000.0;
    if string::<V>(scope, record, "kind")? == "spinner" {
        let frame = FRAMES[(((now::<V>(scope, state)? - started) / SPIN_MS).floor().max(0.0) as usize) % FRAMES.len()];
        return Ok(format!("{} {label} {}", ink.wrap(&fg(ink.0, TO), frame), ink.dim(&duration(elapsed))));
    }
    let (total, width) = (number::<V>(scope, record, "total")?, number::<V>(scope, record, "width")?);
    let as_bytes = truthy::<V>(scope, &scope.get(record, "bytes")?);
    let value = number::<V>(scope, &scope.get(record, "bar")?, "value")?;
    let show = |n: f64| if as_bytes { bytes(n) } else { js_num(js_round(n)) };
    let fraction = if total > 0.0 { js_min(value / total, 1.0) } else { 0.0 };
    let rate = if elapsed > 0.0 { value / elapsed } else { 0.0 };
    let eta = if rate > 0.0 { duration((total - value) / rate) } else { String::from("--") };
    let percent = pad_start(&js_num(js_round(fraction * 100.0)), 3) + "%";
    let stats = format!("{}/{}  {}/s  eta {eta}", show(value), show(total), show(rate));
    let cells = if width != 0.0 && !width.is_nan() { width } else { MIN_BAR.max(MAX_BAR.min(cols - js_len(&label) - js_len(&stats) - 10.0)) };
    let label = if label.is_empty() { String::new() } else { ink.bold(&label) + " " };
    Ok(format!("{label}{} {}  {}", bar(ink, fraction, cells), ink.bold(&percent), ink.dim(&stats)))
}

/// Erases the live region, leaving the cursor where its first line was.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `state` - the module state.
///
/// # Errors
/// What `ops.print` throws.
fn erase<'s, V: Vm>(scope: &V::Scope<'s>, state: &V::Value<'s>) -> Outcome<()> {
    let drawn = number::<V>(scope, state, "drawn")?;
    if drawn > 0.0 {
        print::<V>(scope, state, &format!("\x1b[{}A\x1b[J", js_num(drawn)))?;
    }
    scope.set(state, "drawn", scope.int(0))
}

/// Draws every live item, overwriting the previous frame in place.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `state` - the module state.
/// * `ink` - stderr's styles.
///
/// # Errors
/// What `ops.print` or a property read throws.
fn paint<'s, V: Vm>(scope: &V::Scope<'s>, state: &V::Value<'s>, ink: Ink) -> Outcome<()> {
    let cols = f64::from(term::columns());
    let drawn = number::<V>(scope, state, "drawn")?;
    let mut out = if drawn > 0.0 { format!("\x1b[{}A", js_num(drawn)) } else { String::new() };
    let items = scope.get(state, "items")?;
    let length = number::<V>(scope, &items, "length")?.max(0.0) as usize;
    for index in 0..length {
        let record = scope.get(&items, &index.to_string())?;
        out.push_str("\r\x1b[2K");
        out.push_str(&render::<V>(scope, state, &record, cols, ink)?);
        out.push('\n');
    }
    print::<V>(scope, state, &out)?;
    scope.set(state, "drawn", scope.number(length as f64))?;
    let painted = now::<V>(scope, state)?;
    scope.set(state, "painted", scope.number(painted))
}

/// Redraws if a frame is due.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `state` - the module state.
/// * `ink` - stderr's styles.
///
/// # Errors
/// What [`paint`] throws.
fn refresh<'s, V: Vm>(scope: &V::Scope<'s>, state: &V::Value<'s>, ink: Ink) -> Outcome<()> {
    let timer = scope.get(state, "timer")?;
    if scope.kind(&timer) != Kind::Null && now::<V>(scope, state)? - number::<V>(scope, state, "painted")? >= FRAME_MS {
        paint::<V>(scope, state, ink)?;
    }
    Ok(())
}

/// Adds an item to the live region, starting live mode for the first one.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `state` - the module state.
/// * `record` - the item's record.
/// * `tty` - the terminal.
///
/// # Errors
/// What a property access, `setInterval` or [`paint`] throws.
fn attach<'s, V: Vm>(scope: &V::Scope<'s>, state: &V::Value<'s>, record: &V::Value<'s>, tty: Tty) -> Outcome<()> {
    if !tty.animated {
        return Ok(());
    }
    let items = scope.get(state, "items")?;
    call_method::<V>(scope, &items, "push", std::slice::from_ref(record))?;
    if scope.kind(&scope.get(state, "timer")?) == Kind::Null {
        term::live(true);
        let ink = tty.ink;
        // While items are live, the console prints through this: the region is
        // erased, the line printed, the region redrawn.
        let wrap = bound::<V, _>(scope, "wrap", state, None, move |call: &Call<'_, V>| {
            let (scope, state) = (call.scope(), call.this());
            erase::<V>(scope, state)?;
            let ops = scope.get(state, "ops")?;
            call_method::<V>(scope, &ops, "print", &[arg::<V>(call, 0), arg::<V>(call, 1)])?;
            call_method::<V>(scope, &ops, "flush", &[])?;
            paint::<V>(scope, state, ink)?;
            Ok(scope.undefined())
        })?;
        scope.set(&scope.get(state, "live")?, "wrap", wrap)?;
        let repaint = bound::<V, _>(scope, "paint", state, None, move |call: &Call<'_, V>| {
            paint::<V>(call.scope(), call.this(), ink)?;
            Ok(call.scope().undefined())
        })?;
        let set_interval = scope.get(state, "setInterval")?;
        let timer = scope.call(&set_interval, &[repaint, scope.number(FRAME_MS)])?;
        scope.set(state, "timer", timer)?;
    }
    paint::<V>(scope, state, tty.ink)
}

/// Removes an item and prints its final line above what remains of the region.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `state` - the module state.
/// * `record` - the item's record.
/// * `line` - the permanent line to leave behind; `None` for none.
/// * `ink` - stderr's styles.
///
/// # Errors
/// What `ops.print`, `clearInterval` or a property access throws.
fn detach<'s, V: Vm>(scope: &V::Scope<'s>, state: &V::Value<'s>, record: &V::Value<'s>, line: Option<String>, ink: Ink) -> Outcome<()> {
    let items = scope.get(state, "items")?;
    let at = scope.as_number(&call_method::<V>(scope, &items, "indexOf", std::slice::from_ref(record))?).unwrap_or(-1.0);
    if at >= 0.0 {
        call_method::<V>(scope, &items, "splice", &[scope.number(at), scope.int(1)])?;
    }
    erase::<V>(scope, state)?;
    if let Some(line) = line {
        print::<V>(scope, state, &(line + "\n"))?;
    }
    if number::<V>(scope, &items, "length")? > 0.0 {
        return paint::<V>(scope, state, ink);
    }
    let timer = scope.get(state, "timer")?;
    if scope.kind(&timer) != Kind::Null {
        let clear_interval = scope.get(state, "clearInterval")?;
        scope.call(&clear_interval, &[timer])?;
        scope.set(state, "timer", scope.null())?;
        scope.set(&scope.get(state, "live")?, "wrap", scope.null())?;
        term::live(false);
    }
    Ok(())
}

/// Whether an item is still open, closing it.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `record` - the item's record.
///
/// # Returns
/// `true` when it was open (and is now closed).
///
/// # Errors
/// What the property access throws.
fn close<'s, V: Vm>(scope: &V::Scope<'s>, record: &V::Value<'s>) -> Outcome<bool> {
    let open = truthy::<V>(scope, &scope.get(record, "open")?);
    if open {
        scope.set(record, "open", scope.boolean(false))?;
    }
    Ok(open)
}

// ---------------------------------------------------------------- style

/// Builds the styling functions for one stream.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `depth` - the stream's color depth; 0 makes every function return its text.
///
/// # Returns
/// The style object.
///
/// # Errors
/// [`lunarstorm_event_loop_runtime::vm::Thrown`] when allocation fails.
fn style<'s, V: Vm>(scope: &V::Scope<'s>, depth: i32) -> Outcome<V::Value<'s>> {
    let ink = Ink(depth);
    let object = scope.object()?;
    scope.set(&object, "enabled", scope.boolean(depth > 0))?;
    for (name, code) in [("bold", "1"), ("dim", "2"), ("italic", "3"), ("underline", "4"), ("red", "91"), ("green", "92"), ("yellow", "93"), ("blue", "94"), ("magenta", "95"), ("cyan", "96"), ("gray", "90")] {
        scope.set(&object, name, styler::<V>(scope, name, ink, code.to_owned())?)?;
    }
    let rgb = scope.function("rgb", move |call: &Call<'_, V>| {
        let scope = call.scope();
        let channel = |index: usize| call.args().get(index).and_then(|value| scope.as_number(value)).unwrap_or(f64::NAN);
        styler::<V>(scope, "rgb", ink, fg(depth, [channel(0), channel(1), channel(2)]))
    })?;
    scope.set(&object, "rgb", rgb)?;
    let gradient = scope.function("gradient", move |call: &Call<'_, V>| {
        let scope = call.scope();
        let text = text_or::<V>(scope, &arg::<V>(call, 0), "undefined")?;
        let color = |index: usize, fallback: [f64; 3]| -> Outcome<[f64; 3]> {
            let value = arg::<V>(call, index);
            if scope.kind(&value) == Kind::Undefined {
                return Ok(fallback);
            }
            Ok([number::<V>(scope, &value, "0")?, number::<V>(scope, &value, "1")?, number::<V>(scope, &value, "2")?])
        };
        let (from, to) = (color(1, FROM)?, color(2, TO)?);
        scope.string(&ink.gradient(&text, from, to))
    })?;
    scope.set(&object, "gradient", gradient)?;
    Ok(object)
}

/// One styling function: `(text) => styled text`.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `name` - the function's name.
/// * `ink` - the stream's styles.
/// * `code` - the SGR parameters.
///
/// # Returns
/// The function.
///
/// # Errors
/// [`lunarstorm_event_loop_runtime::vm::Thrown`] when allocation fails.
fn styler<'s, V: Vm>(scope: &V::Scope<'s>, name: &str, ink: Ink, code: String) -> Outcome<V::Value<'s>> {
    scope.function(name, move |call: &Call<'_, V>| {
        let scope = call.scope();
        let text = text_or::<V>(scope, &arg::<V>(call, 0), "undefined")?;
        scope.string(&ink.wrap(&code, &text))
    })
}

// ---------------------------------------------------------------- progress and spinner

/// `progress::<V>(options?)`: starts a progress bar.
///
/// # Arguments
/// * `call` - bound to the module state; `(options)`.
/// * `tty` - the terminal.
///
/// # Returns
/// The bar: `value`, `render`, `update`, `tick`, `done`, `fail`.
///
/// # Errors
/// What reading the options or drawing throws.
fn progress<'a, V: Vm>(call: &Call<'a, V>, tty: Tty) -> Outcome<V::Value<'a>> {
    let (scope, state) = (call.scope(), call.this());
    let options = arg::<V>(call, 0);
    let option = |key: &str| -> Outcome<V::Value<'a>> {
        if matches!(scope.kind(&options), Kind::Undefined | Kind::Null) {
            Ok(scope.undefined())
        } else {
            scope.get(&options, key)
        }
    };
    let total = option("total")?;
    let total = if scope.kind(&total) == Kind::Undefined { 100.0 } else { scope.as_number(&total).unwrap_or(f64::NAN) };
    let label = text_or::<V>(scope, &option("label")?, "")?;
    let width = option("width")?;
    let width = if scope.kind(&width) == Kind::Undefined { 0.0 } else { scope.as_number(&width).unwrap_or(f64::NAN) };
    let as_bytes = truthy::<V>(scope, &option("bytes")?);

    let record = scope.object()?;
    scope.set(&record, "kind", scope.string("progress")?)?;
    scope.set(&record, "label", scope.string(&label)?)?;
    scope.set(&record, "total", scope.number(total))?;
    scope.set(&record, "width", scope.number(width))?;
    scope.set(&record, "bytes", scope.boolean(as_bytes))?;
    scope.set(&record, "started", scope.number(now::<V>(scope, state)?))?;
    scope.set(&record, "open", scope.boolean(true))?;

    let ink = tty.ink;
    let bar = scope.object()?;
    scope.set(&bar, "value", scope.int(0))?;
    let render_fn = bound::<V, _>(scope, "render", state, Some(&record), move |call: &Call<'_, V>| {
        let scope = call.scope();
        let cols = scope.as_number(&arg::<V>(call, 1)).unwrap_or(f64::NAN);
        let line = render::<V>(scope, call.this(), &arg::<V>(call, 0), cols, ink)?;
        scope.string(&line)
    })?;
    scope.set(&bar, "render", render_fn)?;
    let update = bound::<V, _>(scope, "update", state, Some(&record), move |call: &Call<'_, V>| {
        let value = call.scope().as_number(&arg::<V>(call, 1)).unwrap_or(f64::NAN);
        update_bar::<V>(call.scope(), call.this(), &arg::<V>(call, 0), value, ink)?;
        Ok(call.scope().undefined())
    })?;
    scope.set(&bar, "update", update)?;
    let tick = bound::<V, _>(scope, "tick", state, Some(&record), move |call: &Call<'_, V>| {
        let scope = call.scope();
        let n = arg::<V>(call, 1);
        let n = if scope.kind(&n) == Kind::Undefined { 1.0 } else { scope.as_number(&n).unwrap_or(f64::NAN) };
        let record = arg::<V>(call, 0);
        let value = number::<V>(scope, &scope.get(&record, "bar")?, "value")?;
        update_bar::<V>(scope, call.this(), &record, value + n, ink)?;
        Ok(scope.undefined())
    })?;
    scope.set(&bar, "tick", tick)?;
    let done = bound::<V, _>(scope, "done", state, Some(&record), move |call: &Call<'_, V>| {
        let scope = call.scope();
        let record = arg::<V>(call, 0);
        let message = text_or::<V>(scope, &arg::<V>(call, 1), &string::<V>(scope, &record, "label")?)?;
        finish_bar::<V>(scope, call.this(), &record, &ink.green("✔"), &message, ink)?;
        Ok(scope.undefined())
    })?;
    scope.set(&bar, "done", done)?;
    let fail = bound::<V, _>(scope, "fail", state, Some(&record), move |call: &Call<'_, V>| {
        let scope = call.scope();
        let record = arg::<V>(call, 0);
        let message = text_or::<V>(scope, &arg::<V>(call, 1), &string::<V>(scope, &record, "label")?)?;
        finish_bar::<V>(scope, call.this(), &record, &ink.red("✖"), &message, ink)?;
        Ok(scope.undefined())
    })?;
    scope.set(&bar, "fail", fail)?;
    scope.set(&record, "bar", bar.clone())?;
    attach::<V>(scope, state, &record, tty)?;
    Ok(bar)
}

/// A bar's `update`: sets the value; reaching `total` completes the bar.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `state` - the module state.
/// * `record` - the bar's record.
/// * `value` - the new value.
/// * `ink` - stderr's styles.
///
/// # Errors
/// What drawing throws.
fn update_bar<'s, V: Vm>(scope: &V::Scope<'s>, state: &V::Value<'s>, record: &V::Value<'s>, value: f64, ink: Ink) -> Outcome<()> {
    if !truthy::<V>(scope, &scope.get(record, "open")?) {
        return Ok(());
    }
    let total = number::<V>(scope, record, "total")?;
    let value = js_min(value, total);
    scope.set(&scope.get(record, "bar")?, "value", scope.number(value))?;
    if value >= total {
        let label = string::<V>(scope, record, "label")?;
        finish_bar::<V>(scope, state, record, &ink.green("✔"), &label, ink)
    } else {
        refresh::<V>(scope, state, ink)
    }
}

/// Closes a bar once and leaves its summary line.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `state` - the module state.
/// * `record` - the bar's record.
/// * `mark` - the status glyph.
/// * `message` - the summary text; empty for none.
/// * `ink` - stderr's styles.
///
/// # Errors
/// What drawing throws.
fn finish_bar<'s, V: Vm>(scope: &V::Scope<'s>, state: &V::Value<'s>, record: &V::Value<'s>, mark: &str, message: &str, ink: Ink) -> Outcome<()> {
    if !close::<V>(scope, record)? {
        return Ok(());
    }
    let as_bytes = truthy::<V>(scope, &scope.get(record, "bytes")?);
    let value = number::<V>(scope, &scope.get(record, "bar")?, "value")?;
    let shown = if as_bytes { bytes(value) } else { js_num(js_round(value)) };
    let elapsed = (now::<V>(scope, state)? - number::<V>(scope, record, "started")?) / 1000.0;
    let summary = format!("{shown} in {}", duration(elapsed));
    let message = if message.is_empty() { String::new() } else { format!("{message}  ") };
    detach::<V>(scope, state, record, Some(format!("{mark} {message}{}", ink.dim(&summary))), ink)
}

/// `spinner::<V>(label?)`: starts a spinner for work of unknown length.
///
/// # Arguments
/// * `call` - bound to the module state; `(label)`.
/// * `tty` - the terminal.
///
/// # Returns
/// The spinner: `render`, `text`, `succeed`, `fail`, `stop`.
///
/// # Errors
/// What drawing throws.
fn spinner<'a, V: Vm>(call: &Call<'a, V>, tty: Tty) -> Outcome<V::Value<'a>> {
    let (scope, state) = (call.scope(), call.this());
    let label = text_or::<V>(scope, &arg::<V>(call, 0), "")?;
    let record = scope.object()?;
    scope.set(&record, "kind", scope.string("spinner")?)?;
    scope.set(&record, "label", scope.string(&label)?)?;
    scope.set(&record, "started", scope.number(now::<V>(scope, state)?))?;
    scope.set(&record, "open", scope.boolean(true))?;

    let ink = tty.ink;
    let object = scope.object()?;
    let render_fn = bound::<V, _>(scope, "render", state, Some(&record), move |call: &Call<'_, V>| {
        let line = render::<V>(call.scope(), call.this(), &arg::<V>(call, 0), 0.0, ink)?;
        call.scope().string(&line)
    })?;
    scope.set(&object, "render", render_fn)?;
    let text = bound::<V, _>(scope, "text", state, Some(&record), move |call: &Call<'_, V>| {
        let scope = call.scope();
        let next = text_or::<V>(scope, &arg::<V>(call, 1), "undefined")?;
        scope.set(&arg::<V>(call, 0), "label", scope.string(&next)?)?;
        refresh::<V>(scope, call.this(), ink)?;
        Ok(scope.undefined())
    })?;
    scope.set(&object, "text", text)?;
    let succeed = bound::<V, _>(scope, "succeed", state, Some(&record), move |call: &Call<'_, V>| {
        let scope = call.scope();
        let record = arg::<V>(call, 0);
        let message = text_or::<V>(scope, &arg::<V>(call, 1), &string::<V>(scope, &record, "label")?)?;
        finish_spinner::<V>(scope, call.this(), &record, format!("{} {message}", ink.green("✔")), ink)?;
        Ok(scope.undefined())
    })?;
    scope.set(&object, "succeed", succeed)?;
    let fail = bound::<V, _>(scope, "fail", state, Some(&record), move |call: &Call<'_, V>| {
        let scope = call.scope();
        let record = arg::<V>(call, 0);
        let message = text_or::<V>(scope, &arg::<V>(call, 1), &string::<V>(scope, &record, "label")?)?;
        finish_spinner::<V>(scope, call.this(), &record, format!("{} {message}", ink.red("✖")), ink)?;
        Ok(scope.undefined())
    })?;
    scope.set(&object, "fail", fail)?;
    let stop = bound::<V, _>(scope, "stop", state, Some(&record), move |call: &Call<'_, V>| {
        let scope = call.scope();
        let record = arg::<V>(call, 0);
        if close::<V>(scope, &record)? {
            detach::<V>(scope, call.this(), &record, None, ink)?;
        }
        Ok(scope.undefined())
    })?;
    scope.set(&object, "stop", stop)?;
    attach::<V>(scope, state, &record, tty)?;
    Ok(object)
}

/// Closes a spinner once and leaves its line with the elapsed time.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `state` - the module state.
/// * `record` - the spinner's record.
/// * `line` - the permanent line.
/// * `ink` - stderr's styles.
///
/// # Errors
/// What drawing throws.
fn finish_spinner<'s, V: Vm>(scope: &V::Scope<'s>, state: &V::Value<'s>, record: &V::Value<'s>, line: String, ink: Ink) -> Outcome<()> {
    if !close::<V>(scope, record)? {
        return Ok(());
    }
    let elapsed = (now::<V>(scope, state)? - number::<V>(scope, record, "started")?) / 1000.0;
    detach::<V>(scope, state, record, Some(format!("{line} {}", ink.dim(&duration(elapsed)))), ink)
}

// ---------------------------------------------------------------- the module

/// The property of the live-region slot that keeps the module's `default` object
/// once built.
const SHARED: &str = "uiModule";

/// Builds the module's exports at its first import: `style`, `progress`,
/// `spinner`, `default`, in [`declaration`]'s order. A second specifier of the
/// module receives the same objects.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `ops` - the Runtime's natives (`print`, `flush`, `now`).
/// * `live` - the console's live-region slot.
/// * `exports` - receives the values.
///
/// # Errors
/// [`lunarstorm_event_loop_runtime::vm::Thrown`] when allocation fails or a
/// global it keeps (`setInterval`, `clearInterval`) is missing.
pub fn exports<'s, V: Vm>(scope: &V::Scope<'s>, ops: &V::Value<'s>, live: &V::Value<'s>, exports: &mut Vec<V::Value<'s>>) -> Outcome<()> {
    // Decision: the first build is kept on the console's live-region slot (the
    // one object both specifiers' builders receive), so `lunarstorm:ui` and
    // `lunarstorm:hostapi/ui` export the same objects and share one live region.
    let built = scope.get(live, SHARED)?;
    if scope.kind(&built) == Kind::Object {
        for key in ["style", "progress", "spinner"] {
            exports.push(scope.get(&built, key)?);
        }
        exports.push(built);
        return Ok(());
    }
    let tty = Tty { out: term::depth(1) as i32, ink: Ink(term::depth(2) as i32), animated: term::is_tty(2) };
    let global = scope.global();
    let state = scope.object()?;
    scope.set(&state, "ops", ops.clone())?;
    scope.set(&state, "live", live.clone())?;
    scope.set(&state, "items", scope.array(&[])?)?;
    scope.set(&state, "drawn", scope.int(0))?;
    scope.set(&state, "painted", scope.int(0))?;
    scope.set(&state, "timer", scope.null())?;
    scope.set(&state, "setInterval", scope.get(&global, "setInterval")?)?;
    scope.set(&state, "clearInterval", scope.get(&global, "clearInterval")?)?;

    let style = style::<V>(scope, tty.out)?;
    let progress = declared::<V, _>(scope, PROGRESS.name, PROGRESS.sig, move |call: &Call<'_, V>| progress::<V>(call, tty))?;
    let progress = bind::<V>(scope, progress, &state, None)?;
    let spinner = declared::<V, _>(scope, SPINNER.name, SPINNER.sig, move |call: &Call<'_, V>| spinner::<V>(call, tty))?;
    let spinner = bind::<V>(scope, spinner, &state, None)?;
    let default = scope.object()?;
    scope.set(&default, "style", style.clone())?;
    scope.set(&default, "progress", progress.clone())?;
    scope.set(&default, "spinner", spinner.clone())?;
    scope.set(live, SHARED, default.clone())?;
    exports.extend([style, progress, spinner, default]);
    Ok(())
}
