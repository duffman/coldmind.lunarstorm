//! `node:buffer` (`Buffer`, a native class extending `Uint8Array`), and the
//! `TextEncoder` and `TextDecoder` globals. Only UTF-8 is implemented; other
//! encodings are refused rather than mis-decoded.
//!
//! Decision: `Buffer` inherits everything else from `Uint8Array`, as the glue's
//! did: `subarray` shares memory and `slice` copies (unlike Node), and `map` or
//! `subarray` return a `Buffer` through the species constructor.

use super::{arg, f, p, Node, Slot};
use crate::bindings::declared;
use lunarstorm_vm::{Call, ClassDecl, ClassSpec, ErrorKind, Field, FnDecl, Item, Kind, ModuleDecl, Outcome, Scope, Sig, Ty, Vm};
use std::rc::Rc;

/// An encoding argument: only UTF-8 spellings are accepted.
const ENCODING: lunarstorm_vm::Param = p("encoding", Ty::Optional(&Ty::String), "'utf8' or 'utf-8', any case");

const TO_STRING: FnDecl = f("toString", &[ENCODING], None, Ty::String, "Decodes the buffer as UTF-8; invalid sequences become U+FFFD.");
const FROM: FnDecl = f(
    "from",
    &[p("value", Ty::Any, "a string, an array of bytes, a typed array or an ArrayBuffer (shared, not copied)"), ENCODING],
    None,
    Ty::Named("Buffer"),
    "Creates a buffer from a string, array, typed array or ArrayBuffer.",
);
const ALLOC: FnDecl = f("alloc", &[p("size", Ty::Number, "length in bytes")], None, Ty::Named("Buffer"), "Allocates a zero-filled buffer.");
const CONCAT: FnDecl = f(
    "concat",
    &[p("list", Ty::Array(&Ty::Bytes), "the parts"), p("total", Ty::Optional(&Ty::Number), "combined length, when known")],
    None,
    Ty::Named("Buffer"),
    "Joins buffers.",
);
const BYTE_LENGTH: FnDecl = f(
    "byteLength",
    &[p("value", Ty::Union(&[Ty::String, Ty::Object(&[])]), "a string, or anything with a byteLength")],
    None,
    Ty::Number,
    "Byte length of a string in UTF-8, or of a buffer.",
);
const IS_BUFFER: FnDecl = f("isBuffer", &[p("value", Ty::Any, "anything")], None, Ty::Bool, "Whether a value is a Buffer.");

/// The `Buffer` class.
pub(crate) const BUFFER: ClassDecl = ClassDecl {
    name: "Buffer",
    extends: Some("Uint8Array"),
    constructor: Sig {
        params: &[],
        rest: Some(p("args", Ty::Any, "what Uint8Array's constructor takes")),
        returns: Ty::Named("Buffer"),
        about: "A Uint8Array with UTF-8 helpers.",
    },
    methods: &[TO_STRING],
    accessors: &[],
    statics: &[FROM, ALLOC, CONCAT, BYTE_LENGTH, IS_BUFFER],
    about: "Node's Buffer: a Uint8Array with UTF-8 helpers.",
};

/// The `encoding` accessor of both text classes.
const ENCODING_FIELD: Field = Field { name: "encoding", ty: Ty::String, about: "Always 'utf-8'." };
const ENCODE: FnDecl = f("encode", &[p("text", Ty::Any, "converted with String(); '' when left out")], None, Ty::Bytes, "Encodes text as UTF-8.");
const DECODE: FnDecl = f("decode", &[p("input", Ty::Any, "an ArrayBuffer or a view of one; '' when left out")], None, Ty::String, "Decodes UTF-8 bytes.");

/// The `TextEncoder` class.
pub(crate) const TEXT_ENCODER: ClassDecl = ClassDecl {
    name: "TextEncoder",
    extends: None,
    constructor: Sig { params: &[], rest: None, returns: Ty::Named("TextEncoder"), about: "A UTF-8 encoder." },
    methods: &[ENCODE],
    accessors: &[ENCODING_FIELD],
    statics: &[],
    about: "Encodes strings as UTF-8.",
};

/// The `TextDecoder` class.
pub(crate) const TEXT_DECODER: ClassDecl = ClassDecl {
    name: "TextDecoder",
    extends: None,
    constructor: Sig {
        params: &[p("label", Ty::Any, "the encoding; only UTF-8 spellings, else a RangeError")],
        rest: None,
        returns: Ty::Named("TextDecoder"),
        about: "A UTF-8 decoder.",
    },
    methods: &[DECODE],
    accessors: &[ENCODING_FIELD],
    statics: &[],
    about: "Decodes UTF-8 bytes to strings.",
};

/// # Returns
/// The declaration of `node:buffer`.
pub(crate) fn declare() -> ModuleDecl {
    ModuleDecl { specifier: "node:buffer", items: vec![Item::Class(BUFFER)], about: "Node's Buffer." }
}

/// # Returns
/// The names this module puts on the global object.
pub(crate) fn globals() -> Vec<Item> {
    vec![Item::Class(BUFFER), Item::Class(TEXT_ENCODER), Item::Class(TEXT_DECODER)]
}

/// Encodes a JS string as UTF-8.
///
/// Fix (node_fixes `text_encoder_replaces_lone_surrogates`): the glue went
/// through `ops.utf8Encode`, whose string check throws `TypeError` for an
/// unpaired surrogate, so `new TextEncoder().encode("\ud800")` threw; Node and
/// the Encoding Standard give U+FFFD (`EF BF BD`). Decision: take the engine's
/// raw bytes ([`Scope::with_bytes`]), where QuickJS keeps an unpaired surrogate as
/// its 3-byte form (`ED A0..BF xx`, which valid UTF-8 never holds), and replace
/// each such triple with U+FFFD. `Buffer.from` and `Buffer.byteLength` use the
/// same encoder, as Node does.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `value` - a string.
///
/// # Returns
/// The UTF-8 bytes.
///
/// # Errors
/// A `TypeError` when the value is not a string.
pub(crate) fn utf8<'s, V: Vm>(scope: &V::Scope<'s>, value: &V::Value<'s>) -> Outcome<Vec<u8>> {
    scope.with_bytes(value, |bytes| {
        if !bytes.contains(&0xED) {
            return bytes.to_vec();
        }
        let mut out = Vec::with_capacity(bytes.len());
        let mut at = 0;
        while at < bytes.len() {
            if bytes[at] == 0xED && bytes.get(at + 1).is_some_and(|b| (0xA0..=0xBF).contains(b)) && at + 2 < bytes.len() {
                out.extend_from_slice("\u{FFFD}".as_bytes());
                at += 3;
            } else {
                out.push(bytes[at]);
                at += 1;
            }
        }
        out
    })
}

/// Decodes UTF-8, replacing invalid sequences with U+FFFD.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `bytes` - a `Uint8Array`.
///
/// # Returns
/// The string.
///
/// # Errors
/// [`lunarstorm_vm::Thrown`] when the value has no bytes or allocation fails.
fn decode_utf8<'s, V: Vm>(scope: &V::Scope<'s>, bytes: &V::Value<'s>) -> Outcome<V::Value<'s>> {
    let text = scope.with_bytes(bytes, |bytes| String::from_utf8_lossy(bytes).into_owned())?;
    scope.string(&text)
}

/// Refuses encodings the runtime does not implement: anything but `utf8` and
/// `utf-8` in any case (`/^utf-?8$/i`). `undefined` means UTF-8.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `encoding` - the argument.
/// * `kind` - the error to throw: `TypeError` for Buffer, `RangeError` for TextDecoder.
///
/// # Errors
/// `Unsupported encoding: <encoding>`.
pub(crate) fn utf8_only<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, encoding: &V::Value<'s>, kind: ErrorKind) -> Outcome<()> {
    if scope.kind(encoding) == Kind::Undefined {
        return Ok(());
    }
    let label = node.text(scope, encoding)?;
    if label.eq_ignore_ascii_case("utf8") || label.eq_ignore_ascii_case("utf-8") {
        return Ok(());
    }
    Err(scope.throw_error(kind, &format!("Unsupported encoding: {label}")))
}

/// Re-types a `Uint8Array` as a `Buffer` without copying its bytes.
///
/// # Arguments
/// * `node` - the kept builtins.
/// * `scope` - the context handle.
/// * `bytes` - the array.
///
/// # Returns
/// The same array, now a `Buffer`.
///
/// # Errors
/// What `Object.setPrototypeOf` throws.
pub(crate) fn as_buffer<'s, V: Vm>(node: &Node<V>, scope: &V::Scope<'s>, bytes: V::Value<'s>) -> Outcome<V::Value<'s>> {
    node.call(scope, Slot::SetPrototypeOf, &[bytes, node.get(scope, Slot::BufferProto)])
}

/// Builds `Buffer`, `TextEncoder` and `TextDecoder` and puts them on the global object.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `node` - the kept builtins; `Buffer` and its prototype are kept.
/// * `native` - the object the glue receives; `asBuffer` and `utf8Only` are set on it.
///
/// # Returns
/// The `node:buffer` module object, `{ Buffer }`.
///
/// # Errors
/// [`lunarstorm_vm::Thrown`] when a value cannot be created.
pub(crate) fn install<'s, V: Vm>(scope: &V::Scope<'s>, node: &Rc<Node<V>>, native: &V::Value<'s>) -> Outcome<V::Value<'s>> {
    let global = scope.global();

    let n = node.clone();
    let to_string = declared(scope, TO_STRING.name, TO_STRING.sig, move |call: &Call<'_, V>| {
        let scope = call.scope();
        utf8_only(&n, scope, &arg::<V>(scope, call.args(), 0), ErrorKind::Type)?;
        decode_utf8::<V>(scope, call.this())
    })?;
    let n = node.clone();
    let from = declared(scope, FROM.name, FROM.sig, move |call: &Call<'_, V>| {
        let (scope, args) = (call.scope(), call.args());
        let value = arg::<V>(scope, args, 0);
        if scope.kind(&value) == Kind::String {
            utf8_only(&n, scope, &arg::<V>(scope, args, 1), ErrorKind::Type)?;
            let bytes = scope.bytes(utf8::<V>(scope, &value)?)?;
            return as_buffer(&n, scope, bytes);
        }
        n.construct(scope, &n.get(scope, Slot::Buffer), &[value])
    })?;
    let n = node.clone();
    let alloc = declared(scope, ALLOC.name, ALLOC.sig, move |call: &Call<'_, V>| {
        let scope = call.scope();
        n.construct(scope, &n.get(scope, Slot::Buffer), &call.args()[..1])
    })?;
    let n = node.clone();
    let concat = declared(scope, CONCAT.name, CONCAT.sig, move |call: &Call<'_, V>| {
        let (scope, args) = (call.scope(), call.args());
        let list = n.items(scope, &args[0])?;
        let total = arg::<V>(scope, args, 1);
        // One part and no length: that part itself, re-typed (not copied), as in the glue.
        if list.len() == 1 && scope.kind(&total) == Kind::Undefined {
            return as_buffer(&n, scope, list[0].clone());
        }
        let total = match scope.as_number(&total) {
            Some(total) => total,
            None => {
                let mut sum = 0.0;
                for part in &list {
                    sum += n.length(scope, part)? as f64;
                }
                sum
            }
        };
        let out = n.construct(scope, &n.get(scope, Slot::Buffer), &[scope.number(total)])?;
        let set = scope.get(&out, "set")?;
        let mut at = 0.0;
        for part in &list {
            if at >= total {
                break;
            }
            let length = n.length(scope, part)? as f64;
            let part = if at + length > total {
                scope.call_with(&scope.get(part, "subarray")?, part, &[scope.number(0.0), scope.number(total - at)])?
            } else {
                part.clone()
            };
            scope.call_with(&set, &out, &[part.clone(), scope.number(at)])?;
            at += n.length(scope, &part)? as f64;
        }
        Ok(out)
    })?;
    let byte_length = declared(scope, BYTE_LENGTH.name, BYTE_LENGTH.sig, |call: &Call<'_, V>| {
        let scope = call.scope();
        let value = &call.args()[0];
        if scope.kind(value) == Kind::String {
            return Ok(scope.number(utf8::<V>(scope, value)?.len() as f64));
        }
        scope.get(value, "byteLength")
    })?;
    let n = node.clone();
    let is_buffer = declared(scope, IS_BUFFER.name, IS_BUFFER.sig, move |call: &Call<'_, V>| {
        let scope = call.scope();
        Ok(scope.boolean(n.instance_of(scope, Slot::BufferProto, &arg::<V>(scope, call.args(), 0))?))
    })?;
    let spec = ClassSpec::new(BUFFER.name)
        .extends(node.get(scope, Slot::Uint8Array))
        .method(TO_STRING.name, to_string)
        .static_member(FROM.name, from)
        .static_member(ALLOC.name, alloc)
        .static_member(CONCAT.name, concat)
        .static_member(BYTE_LENGTH.name, byte_length)
        .static_member(IS_BUFFER.name, is_buffer);
    let buffer = scope.class::<(), _>(spec, |_: &Call<'_, V>| Ok(()))?;
    node.keep(scope, Slot::Buffer, &buffer);
    node.keep(scope, Slot::BufferProto, &scope.get(&buffer, "prototype")?);
    scope.set(&global, BUFFER.name, buffer.clone())?;

    let utf8_name = scope.function("encoding", |call: &Call<'_, V>| call.scope().string("utf-8"))?;
    let encode = declared(scope, ENCODE.name, ENCODE.sig, {
        let n = node.clone();
        move |call: &Call<'_, V>| {
            let scope = call.scope();
            let text = arg::<V>(scope, call.args(), 0);
            let bytes = match scope.kind(&text) {
                Kind::Undefined => Vec::new(),
                Kind::String => utf8::<V>(scope, &text)?,
                _ => utf8::<V>(scope, &n.call(scope, Slot::String, &[text])?)?,
            };
            scope.bytes(bytes)
        }
    })?;
    let spec = ClassSpec::new(TEXT_ENCODER.name).method(ENCODE.name, encode).accessor(ENCODING_FIELD.name, Some(utf8_name.clone()), None);
    let encoder = scope.class::<(), _>(spec, |_: &Call<'_, V>| Ok(()))?;
    scope.set(&global, TEXT_ENCODER.name, encoder)?;

    let n = node.clone();
    let decode = declared(scope, DECODE.name, DECODE.sig, move |call: &Call<'_, V>| {
        let scope = call.scope();
        let input = arg::<V>(scope, call.args(), 0);
        match scope.kind(&input) {
            Kind::Undefined => scope.string(""),
            Kind::Bytes => decode_utf8::<V>(scope, &input),
            _ => {
                let uint8 = n.get(scope, Slot::Uint8Array);
                let view = n.call(scope, Slot::IsView, std::slice::from_ref(&input))?;
                let bytes = if scope.as_bool(&view) == Some(true) {
                    let parts = [scope.get(&input, "buffer")?, scope.get(&input, "byteOffset")?, scope.get(&input, "byteLength")?];
                    n.construct(scope, &uint8, &parts)?
                } else {
                    n.construct(scope, &uint8, &[input])?
                };
                decode_utf8::<V>(scope, &bytes)
            }
        }
    })?;
    let spec = ClassSpec::new(TEXT_DECODER.name).method(DECODE.name, decode).accessor(ENCODING_FIELD.name, Some(utf8_name), None);
    let n = node.clone();
    let decoder = scope.class::<(), _>(spec, move |call: &Call<'_, V>| {
        let scope = call.scope();
        utf8_only(&n, scope, &arg::<V>(scope, call.args(), 0), ErrorKind::Range)
    })?;
    scope.set(&global, TEXT_DECODER.name, decoder)?;

    let n = node.clone();
    let as_buffer_fn = scope.function("asBuffer", move |call: &Call<'_, V>| {
        let scope = call.scope();
        as_buffer(&n, scope, arg::<V>(scope, call.args(), 0))
    })?;
    scope.set(native, "asBuffer", as_buffer_fn)?;
    let n = node.clone();
    let utf8_only_fn = scope.function("utf8Only", move |call: &Call<'_, V>| {
        let scope = call.scope();
        utf8_only(&n, scope, &arg::<V>(scope, call.args(), 0), ErrorKind::Type)?;
        Ok(scope.undefined())
    })?;
    scope.set(native, "utf8Only", utf8_only_fn)?;

    let module = scope.object()?;
    scope.set(&module, BUFFER.name, buffer)?;
    Ok(module)
}
