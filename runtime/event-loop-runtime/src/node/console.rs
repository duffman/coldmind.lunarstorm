//! The `console` global and the value inspector behind it, which also formats
//! uncaught errors (the glue's `describe`).
//!
//! A line-for-line port of the glue's `inspect` and `format`; `node_surface`
//! pins their output. Conversions a script could observe (`String(x)`,
//! `Number(x)`, `JSON.stringify`, `Object.keys`) go through the builtins
//! [`Node`] captured, so numbers, keys and JSON print exactly as before.
//!
//! Decisions:
//! - Colors are decided once at install from the host binding `ops.color`
//!   (`[stdout depth, stderr depth]`, set by the CLI host); a host without it
//!   gets uncolored output.
//! - The live-region slot `{ wrap }` is a plain JS object `lunarstorm:ui` writes
//!   to. While `wrap` is `null` a line goes straight to the stream; otherwise
//!   `wrap(fd, text)` prints it above the live region. `console.trace` bypasses
//!   it, as in the glue.

use super::{f, p, Node, Slot};
use crate::bindings::declared;
use crate::host::Host;
use crate::io::Shared;
use crate::ops::print;
use lunarstorm_engine_contract::{Call, ErrorKind, Field, FnDecl, Item, Kind, Outcome, Scope, Sig, Ty, Vm};
use std::rc::Rc;

/// What every console method takes.
const PRINT_SIG: Sig = Sig {
    params: &[],
    rest: Some(p("args", Ty::Any, "values; a first string may hold %s %d %i %f %j %o %O %% placeholders")),
    returns: Ty::Undefined,
    about: "Writes one formatted line.",
};
const LOG: FnDecl = f("log", &[], PRINT_SIG.rest, Ty::Undefined, "Writes one formatted line to stdout.");
const INFO: FnDecl = f("info", &[], PRINT_SIG.rest, Ty::Undefined, "Writes one formatted line to stdout.");
const DEBUG: FnDecl = f("debug", &[], PRINT_SIG.rest, Ty::Undefined, "Writes one formatted line to stdout.");
const ERROR: FnDecl = f("error", &[], PRINT_SIG.rest, Ty::Undefined, "Writes one formatted line to stderr.");
const WARN: FnDecl = f("warn", &[], PRINT_SIG.rest, Ty::Undefined, "Writes one formatted line to stderr.");
const TRACE: FnDecl = f("trace", &[], PRINT_SIG.rest, Ty::Undefined, "Writes a message and the current stack to stderr.");

/// The console's methods, in its key order, with the stream each writes to.
const METHODS: [(FnDecl, i32); 5] = [(LOG, 1), (INFO, 1), (DEBUG, 1), (ERROR, 2), (WARN, 2)];

/// The members of `console`, for its type.
const MEMBERS: [Field; 6] = [
    Field { name: "log", ty: Ty::Function(&PRINT_SIG), about: "Writes one formatted line to stdout." },
    Field { name: "info", ty: Ty::Function(&PRINT_SIG), about: "Writes one formatted line to stdout." },
    Field { name: "debug", ty: Ty::Function(&PRINT_SIG), about: "Writes one formatted line to stdout." },
    Field { name: "error", ty: Ty::Function(&PRINT_SIG), about: "Writes one formatted line to stderr." },
    Field { name: "warn", ty: Ty::Function(&PRINT_SIG), about: "Writes one formatted line to stderr." },
    Field { name: "trace", ty: Ty::Function(&PRINT_SIG), about: "Writes a message and the current stack to stderr." },
];

/// # Returns
/// The names this module puts on the global object: `console`.
pub(crate) fn globals() -> Vec<Item> {
    vec![Item::Value(Field { name: "console", ty: Ty::Object(&MEMBERS), about: "Formatted output to stdout and stderr." })]
}

/// Renders values the way `console.log` shows them.
struct Inspector<'a, 's, V: Vm> {
    /// The kept builtins.
    node: &'a Node<V>,
    /// The context handle.
    scope: &'a V::Scope<'s>,
    /// Whether to color the output.
    color: bool,
    /// The objects being printed, outermost first, for cycle detection.
    seen: Vec<usize>,
}

impl<'s, V: Vm> Inspector<'_, 's, V> {
    /// Wraps text in one SGR color when coloring is on.
    ///
    /// # Arguments
    /// * `code` - the SGR parameter.
    /// * `text` - the text.
    ///
    /// # Returns
    /// The text, colored or not.
    fn paint(&self, code: &str, text: String) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text
        }
    }

    /// # Arguments
    /// * `value` - any value.
    ///
    /// # Returns
    /// `String(value)`.
    ///
    /// # Errors
    /// What `String()` throws.
    fn text(&self, value: &V::Value<'s>) -> Outcome<String> {
        self.node.text(self.scope, value)
    }

    /// # Arguments
    /// * `value` - a value of kind [`Kind::Other`].
    ///
    /// # Returns
    /// Whether it is a big integer (else a symbol).
    ///
    /// # Errors
    /// None in practice; `Object.prototype.toString` does not throw for primitives.
    fn is_bigint(&self, value: &V::Value<'s>) -> Outcome<bool> {
        let tag = self.scope.call_with(&self.node.get(self.scope, Slot::ObjectToString), value, &[])?;
        Ok(self.text(&tag)? == "[object BigInt]")
    }

    /// Formats an error with its name, message and stack.
    ///
    /// # Arguments
    /// * `error` - the error.
    ///
    /// # Returns
    /// `Name: message`, followed by the stack when it does not already start so.
    ///
    /// # Errors
    /// What a getter or `String()` throws.
    fn error_text(&self, error: &V::Value<'s>) -> Outcome<String> {
        let scope = self.scope;
        let mut head = self.text(&scope.get(error, "name")?)?;
        let message = scope.get(error, "message")?;
        if self.node.truthy(scope, &message) {
            head = format!("{head}: {}", self.text(&message)?);
        }
        let stack = scope.get(error, "stack")?;
        let stack = if scope.kind(&stack) == Kind::String { scope.with_str(&stack, |text| text.trim_end().to_owned())? } else { String::new() };
        Ok(if stack.is_empty() {
            head
        } else if stack.starts_with(&head) {
            stack
        } else {
            format!("{head}\n{stack}")
        })
    }

    /// Renders one value.
    ///
    /// # Arguments
    /// * `value` - the value.
    /// * `depth` - nesting level; 0 is the top, where strings print bare.
    ///
    /// # Returns
    /// The text.
    ///
    /// # Errors
    /// What a getter, a conversion or an iterator throws.
    fn inspect(&mut self, value: &V::Value<'s>, depth: usize) -> Outcome<String> {
        let (node, scope) = (self.node, self.scope);
        match scope.kind(value) {
            Kind::String => {
                let text = self.text(value)?;
                return Ok(if depth == 0 { text } else { self.paint("32", format!("'{text}'")) });
            }
            Kind::Function => {
                let name = scope.get(value, "name")?;
                let text = if node.truthy(scope, &name) { format!("[Function: {}]", self.text(&name)?) } else { String::from("[Function (anonymous)]") };
                return Ok(self.paint("36", text));
            }
            Kind::Other if self.is_bigint(value)? => return Ok(self.paint("33", format!("{}n", self.text(value)?))),
            Kind::Other => return Ok(self.paint("32", self.text(value)?)),
            Kind::Undefined => return Ok(self.paint("90", String::from("undefined"))),
            Kind::Null => return Ok(self.paint("1", String::from("null"))),
            Kind::Bool | Kind::Number => return Ok(self.paint("33", self.text(value)?)),
            Kind::Object | Kind::Bytes => {}
        }
        if node.instance_of(scope, Slot::ErrorProto, value)? {
            return self.error_text(value);
        }
        if node.instance_of(scope, Slot::DateProto, value)? {
            let time = node.call(scope, Slot::Number, std::slice::from_ref(value))?;
            let text = if scope.as_number(&time).is_none_or(f64::is_nan) {
                String::from("Invalid Date")
            } else {
                self.text(&scope.call_with(&scope.get(value, "toISOString")?, value, &[])?)?
            };
            return Ok(self.paint("35", text));
        }
        if node.instance_of(scope, Slot::RegExpProto, value)? {
            return Ok(self.paint("31", self.text(value)?));
        }
        let id = scope.identity(value);
        if self.seen.contains(&id) {
            return Ok(self.paint("36", String::from("[Circular]")));
        }
        let array = scope.as_bool(&node.call(scope, Slot::IsArray, std::slice::from_ref(value))?) == Some(true);
        let list = array || scope.as_bool(&node.call(scope, Slot::IsView, std::slice::from_ref(value))?) == Some(true);
        if depth > 2 {
            return Ok(self.paint("36", String::from(if list { "[Array]" } else { "[Object]" })));
        }
        self.seen.push(id);
        // `v.constructor && v.constructor.name`: a falsy constructor is the name itself.
        let constructor = scope.get(value, "constructor")?;
        let name = if node.truthy(scope, &constructor) { scope.get(&constructor, "name")? } else { constructor };
        let out = if list {
            let length = node.length(scope, value)?;
            let shown = length.min(100);
            let mut parts = Vec::with_capacity(shown + 1);
            for index in 0..shown {
                let item = node.at(scope, value, index)?;
                parts.push(self.inspect(&item, depth + 1)?);
            }
            if length > shown {
                parts.push(format!("... {} more items", length - shown));
            }
            let prefix = if array { String::new() } else { format!("{}({length}) ", self.text(&name)?) };
            let body = if parts.is_empty() { String::from("[]") } else { format!("[ {} ]", parts.join(", ")) };
            prefix + &body
        } else if node.instance_of(scope, Slot::MapProto, value)? || node.instance_of(scope, Slot::SetProto, value)? {
            let map = node.instance_of(scope, Slot::MapProto, value)?;
            let entries = node.items(scope, &node.call(scope, Slot::ArrayFrom, std::slice::from_ref(value))?)?;
            let mut parts = Vec::with_capacity(entries.len());
            for entry in &entries {
                if map {
                    let (key, item) = (node.at(scope, entry, 0)?, node.at(scope, entry, 1)?);
                    parts.push(format!("{} => {}", self.inspect(&key, depth + 1)?, self.inspect(&item, depth + 1)?));
                } else {
                    parts.push(self.inspect(entry, depth + 1)?);
                }
            }
            let size = self.text(&scope.get(value, "size")?)?;
            let body = if parts.is_empty() { String::from("{}") } else { format!("{{ {} }}", parts.join(", ")) };
            format!("{}({size}) {body}", self.text(&name)?)
        } else {
            let keys = node.items(scope, &node.call(scope, Slot::Keys, std::slice::from_ref(value))?)?;
            let mut parts = Vec::with_capacity(keys.len());
            for key in &keys {
                let key = self.text(key)?;
                let mut chars = key.chars();
                let plain = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
                    && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
                let item = scope.get(value, &key)?;
                let shown = self.inspect(&item, depth + 1)?;
                parts.push(if plain { format!("{key}: {shown}") } else { format!("'{key}': {shown}") });
            }
            let tag = if scope.kind(&name) == Kind::Undefined {
                String::from("[Object: null prototype] ")
            } else if node.truthy(scope, &name) {
                let name = self.text(&name)?;
                if name == "Object" { String::new() } else { format!("{name} ") }
            } else {
                String::new()
            };
            let body = if parts.is_empty() { String::from("{}") } else { format!("{{ {} }}", parts.join(", ")) };
            tag + &body
        };
        self.seen.pop();
        Ok(out)
    }

    /// Renders one value from the top, with no ancestors.
    ///
    /// # Arguments
    /// * `value` - the value.
    /// * `depth` - the starting level.
    ///
    /// # Returns
    /// The text.
    ///
    /// # Errors
    /// See [`Inspector::inspect`].
    fn fresh(&mut self, value: &V::Value<'s>, depth: usize) -> Outcome<String> {
        self.seen.clear();
        self.inspect(value, depth)
    }

    /// Joins console arguments, honouring printf-style placeholders in a first
    /// string argument when more arguments follow.
    ///
    /// # Arguments
    /// * `args` - the arguments.
    ///
    /// # Returns
    /// The line, without its newline.
    ///
    /// # Errors
    /// What a conversion or getter throws.
    fn format(&mut self, args: &[V::Value<'s>]) -> Outcome<String> {
        let (node, scope) = (self.node, self.scope);
        let mut next = 0;
        let mut out = String::new();
        let template = match args.first() {
            Some(first) if args.len() > 1 && scope.kind(first) == Kind::String => Some(self.text(first)?).filter(|text| text.contains('%')),
            _ => None,
        };
        if let Some(template) = template {
            next = 1;
            let mut chars = template.chars().peekable();
            while let Some(c) = chars.next() {
                let kind = match chars.peek() {
                    Some(&kind) if c == '%' && "sdifjoO%".contains(kind) => kind,
                    _ => {
                        out.push(c);
                        continue;
                    }
                };
                chars.next();
                if kind == '%' {
                    out.push('%');
                    continue;
                }
                let Some(value) = args.get(next) else {
                    out.push('%');
                    out.push(kind);
                    continue;
                };
                next += 1;
                let piece = match kind {
                    's' if scope.kind(value) == Kind::String => self.text(value)?,
                    's' => self.fresh(value, 1)?,
                    'd' if scope.kind(value) == Kind::Other && self.is_bigint(value)? => format!("{}n", self.text(value)?),
                    'd' => self.text(&node.call(scope, Slot::Number, std::slice::from_ref(value))?)?,
                    'i' => self.text(&node.call(scope, Slot::ParseInt, &[value.clone(), scope.int(10)])?)?,
                    'f' => self.text(&node.call(scope, Slot::ParseFloat, std::slice::from_ref(value))?)?,
                    'j' => self.text(&node.call(scope, Slot::Stringify, std::slice::from_ref(value))?)?,
                    _ => self.fresh(value, 0)?,
                };
                out.push_str(&piece);
            }
        } else if let Some(first) = args.first() {
            next = 1;
            out = self.fresh(first, 0)?;
        }
        for value in &args[next.min(args.len())..] {
            out.push(' ');
            out.push_str(&self.fresh(value, 0)?);
        }
        Ok(out)
    }
}

/// Renders a value as `console.log` would, uncolored; formats uncaught errors.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `value` - the value.
///
/// # Returns
/// The text.
///
/// # Errors
/// What a getter or conversion throws.
pub(crate) fn inspect<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, value: &V::Value<'s>) -> Outcome<String> {
    Inspector { node, scope, color: false, seen: Vec::new() }.fresh(value, 0)
}

/// Builds `console` and puts it on the global object.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state: the output streams.
/// * `node` - the kept builtins; the live-region slot is kept.
/// * `ops` - the `ops` object; its host binding `color` decides coloring.
/// * `native` - the object the glue receives; `inspect` and `live` are set on it.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when a value cannot be created.
pub(crate) fn install<'s, V: Vm, H: Host>(
    scope: &V::Scope<'s>,
    shared: &Rc<Shared<H>>,
    node: &Rc<Node<V>>,
    ops: &V::Value<'s>,
    native: &V::Value<'s>,
) -> Outcome<()> {
    // `ops.color || [0, 0]`: color depth of stdout and stderr; any depth above 0 colors.
    let depth = scope.get(ops, "color")?;
    let tint = |index: usize| -> Outcome<bool> {
        if !node.truthy(scope, &depth) {
            return Ok(false);
        }
        Ok(scope.as_number(&node.at(scope, &depth, index)?).is_some_and(|depth| depth > 0.0))
    };
    let tint = [false, tint(0)?, tint(1)?];

    let live = scope.object()?;
    scope.set(&live, "wrap", scope.null())?;
    node.keep(scope, Slot::Live, &live);

    let console = scope.object()?;
    for (decl, fd) in METHODS {
        let (s, n, color) = (shared.clone(), node.clone(), tint[fd as usize]);
        let method = declared(scope, decl.name, decl.sig, move |call: &Call<'_, V>| {
            let scope = call.scope();
            let mut line = Inspector { node: &n, scope, color, seen: Vec::new() }.format(call.args())?;
            line.push('\n');
            let wrap = scope.get(&n.get(scope, Slot::Live), "wrap")?;
            if scope.kind(&wrap) == Kind::Null {
                print(&s, fd, line.as_bytes());
            } else {
                scope.call(&wrap, &[scope.int(fd), scope.string(&line)?])?;
            }
            Ok(scope.undefined())
        })?;
        scope.set(&console, decl.name, method)?;
    }
    let (s, n, color) = (shared.clone(), node.clone(), tint[2]);
    let trace = declared(scope, TRACE.name, TRACE.sig, move |call: &Call<'_, V>| {
        let scope = call.scope();
        let line = Inspector { node: &n, scope, color, seen: Vec::new() }.format(call.args())?;
        let stack = scope.get(&scope.error(ErrorKind::Error, "")?, "stack")?;
        let text = format!("Trace: {line}\n{}", n.text(scope, &stack)?);
        print(&s, 2, text.as_bytes());
        Ok(scope.undefined())
    })?;
    scope.set(&console, TRACE.name, trace)?;
    scope.set(&scope.global(), "console", console)?;

    let n = node.clone();
    let inspect_fn = scope.function("inspect", move |call: &Call<'_, V>| {
        let scope = call.scope();
        let value = call.args().first().cloned().unwrap_or_else(|| scope.undefined());
        let text = inspect(&n, scope, &value)?;
        scope.string(&text)
    })?;
    scope.set(native, "inspect", inspect_fn)?;
    scope.set(native, "live", live)?;
    Ok(())
}
