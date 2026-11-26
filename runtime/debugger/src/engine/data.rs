use super::{FailureCode, FailureReport};
use std::{fmt, mem::size_of};

/// Lossless owned JavaScript string containing UTF-16 code units.
///
/// Unpaired surrogates and embedded NULs are preserved without normalization.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct JsString(Vec<u16>);
impl JsString {
    /// Takes ownership of lossless UTF-16 units without validating or normalizing them.
    pub fn from_units(units: Vec<u16>) -> Self {
        Self(units)
    }
    /// Copies valid UTF-8 text into JavaScript UTF-16 units.
    pub fn from_utf8(text: &str) -> Self {
        Self(text.encode_utf16().collect())
    }
    /// Borrows the original UTF-16 code units without conversion.
    pub fn as_units(&self) -> &[u16] {
        &self.0
    }
    /// Consumes the string and returns its owned UTF-16 storage.
    pub fn into_units(self) -> Vec<u16> {
        self.0
    }
    /// Converts to UTF-8, returning an error for unpaired UTF-16 surrogates.
    pub fn to_utf8(&self) -> Result<String, std::string::FromUtf16Error> {
        String::from_utf16(&self.0)
    }
    /// Returns the backing allocation capacity in bytes, saturating on overflow.
    pub fn capacity_bytes(&self) -> usize {
        self.0.capacity().saturating_mul(2)
    }
    pub(crate) fn bound(self, max: usize) -> (Self, bool) {
        let truncated = self.0.len() > max;
        // A take/collect over Vec::IntoIter can reuse the original oversized
        // allocation. Diagnostics must discard that backing capacity as well.
        (Self(self.0[..self.0.len().min(max)].to_vec()), truncated)
    }
}
impl fmt::Debug for JsString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_utf8() {
            Ok(text) => text.fmt(f),
            Err(_) => f.debug_tuple("UTF16").field(&self.0).finish(),
        }
    }
}

/// Bounds for copied data import, export, and owned payload validation.
///
/// Defaults are depth 64, 65,536 nodes, and 8 MiB of owned storage. These
/// limits bound boundary data, not allocations in the native JavaScript heap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportLimits {
    /// Maximum nesting depth, counting the root as depth one.
    pub max_depth: usize,
    /// Maximum number of value nodes visited, including the root.
    pub max_nodes: usize,
    /// Maximum owned footprint, including string, buffer, and vector capacities.
    pub max_bytes: usize,
}
impl Default for ExportLimits {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_nodes: 65_536,
            max_bytes: 8_388_608,
        }
    }
}

/// Owned, transferable data at the JavaScript/host boundary.
///
/// This is a copied data model, not a representation of every JavaScript value.
/// Record order is retained; functions, symbols, BigInts, cycles, sparse arrays,
/// and unsupported object semantics fail export rather than being stringified.
/// Dropping nested data dismantles containers iteratively.
#[derive(Clone, Debug, PartialEq)]
pub enum JsData {
    /// JavaScript `undefined`.
    Undefined,
    /// JavaScript `null`.
    Null,
    /// A Boolean value.
    Bool(bool),
    /// An IEEE 754 number preserving negative zero, infinities, and NaN semantics.
    Number(f64),
    /// Lossless UTF-16 text.
    String(JsString),
    /// Owned bytes imported as `Uint8Array`; base export accepts ordinary buffers/views.
    Bytes(Vec<u8>),
    /// Dense ordered elements; export must reject holes.
    Array(Vec<JsData>),
    /// Ordered string-keyed entries imported as null-prototype own data properties.
    Record(Vec<(JsString, JsData)>),
}
impl JsData {
    /// Includes backing capacities, not just logical lengths. The walk is iterative.
    /// Returns the checked owned footprint, rejecting depth, node, or byte overflow
    /// with [`FailureCode::Capacity`]. The root has depth one and one node.
    /// No JavaScript is executed and no partial successful size is returned.
    pub fn footprint(&self, limits: ExportLimits) -> Result<usize, FailureReport> {
        let fail = || FailureReport::fault(FailureCode::Capacity, "owned data limits exceeded");
        let mut bytes = size_of::<Self>();
        let mut nodes = 0usize;
        let mut stack = vec![(self, 1usize)];
        while let Some((node, depth)) = stack.pop() {
            nodes = nodes.checked_add(1).ok_or_else(fail)?;
            if depth > limits.max_depth || nodes > limits.max_nodes {
                return Err(fail());
            }
            let extra = match node {
                Self::String(s) => s.0.capacity().checked_mul(2).ok_or_else(fail)?,
                Self::Bytes(v) => v.capacity(),
                Self::Array(v) => {
                    if v.len() > limits.max_nodes.saturating_sub(nodes) {
                        return Err(fail());
                    }
                    stack.extend(v.iter().rev().map(|x| (x, depth + 1)));
                    v.capacity()
                        .checked_mul(size_of::<Self>())
                        .ok_or_else(fail)?
                }
                Self::Record(v) => {
                    if v.len() > limits.max_nodes.saturating_sub(nodes) {
                        return Err(fail());
                    }
                    let mut total = v
                        .capacity()
                        .checked_mul(size_of::<(JsString, Self)>())
                        .ok_or_else(fail)?;
                    for (key, value) in v.iter().rev() {
                        total = total
                            .checked_add(key.0.capacity().checked_mul(2).ok_or_else(fail)?)
                            .ok_or_else(fail)?;
                        stack.push((value, depth + 1));
                    }
                    total
                }
                _ => 0,
            };
            bytes = bytes.checked_add(extra).ok_or_else(fail)?;
            if bytes > limits.max_bytes {
                return Err(fail());
            }
        }
        Ok(bytes)
    }
}
impl Drop for JsData {
    fn drop(&mut self) {
        fn drain(data: &mut JsData, pending: &mut Vec<JsData>) {
            match data {
                JsData::Array(values) => pending.append(values),
                JsData::Record(values) => pending.extend(values.drain(..).map(|(_, value)| value)),
                _ => {}
            }
        }
        let mut pending = Vec::new();
        drain(self, &mut pending);
        while let Some(mut value) = pending.pop() {
            drain(&mut value, &mut pending);
        }
    }
}

// Conversions between Rust values and copied data, used by typed host functions
// (`HostModule::typed`). Decision: only exact, lossless pairs. `Vec<u8>` is `Bytes`, never
// an array, and a number is `f64` only; integer and `Option` conversions were left out
// because each needs a rounding or null/undefined policy the caller should choose, and
// `TryFrom<JsData> for Option<T>` would overlap core's blanket impl for `T = JsData`.

impl From<()> for JsData {
    /// `undefined`, the result of a function that returns nothing.
    fn from((): ()) -> Self {
        JsData::Undefined
    }
}
impl From<bool> for JsData {
    /// A Boolean.
    fn from(value: bool) -> Self {
        JsData::Bool(value)
    }
}
impl From<f64> for JsData {
    /// A number.
    fn from(value: f64) -> Self {
        JsData::Number(value)
    }
}
impl From<String> for JsData {
    /// A string, converted to UTF-16.
    fn from(value: String) -> Self {
        JsData::String(JsString::from_utf8(&value))
    }
}
impl From<&str> for JsData {
    /// A string, converted to UTF-16.
    fn from(value: &str) -> Self {
        JsData::String(JsString::from_utf8(value))
    }
}
impl From<JsString> for JsData {
    /// A string, as is.
    fn from(value: JsString) -> Self {
        JsData::String(value)
    }
}
impl From<Vec<u8>> for JsData {
    /// Bytes, imported as a `Uint8Array`.
    fn from(value: Vec<u8>) -> Self {
        JsData::Bytes(value)
    }
}
impl From<Vec<JsData>> for JsData {
    /// An array.
    fn from(value: Vec<JsData>) -> Self {
        JsData::Array(value)
    }
}

impl JsData {
    /// The JavaScript type name used in conversion errors.
    ///
    /// # Returns
    /// `undefined`, `null`, `boolean`, `number`, `string`, `Uint8Array`, `array` or `object`.
    fn kind(&self) -> &'static str {
        match self {
            JsData::Undefined => "undefined",
            JsData::Null => "null",
            JsData::Bool(_) => "boolean",
            JsData::Number(_) => "number",
            JsData::String(_) => "string",
            JsData::Bytes(_) => "Uint8Array",
            JsData::Array(_) => "array",
            JsData::Record(_) => "object",
        }
    }
}
impl TryFrom<JsData> for bool {
    /// The error message.
    type Error = String;
    /// A Boolean, without JavaScript truthiness.
    ///
    /// # Errors
    /// Any other kind of value.
    fn try_from(value: JsData) -> Result<Self, String> {
        match value {
            JsData::Bool(value) => Ok(value),
            other => Err(format!("expected a boolean, got {}", other.kind())),
        }
    }
}
impl TryFrom<JsData> for f64 {
    /// The error message.
    type Error = String;
    /// A number, without coercion.
    ///
    /// # Errors
    /// Any other kind of value.
    fn try_from(value: JsData) -> Result<Self, String> {
        match value {
            JsData::Number(value) => Ok(value),
            other => Err(format!("expected a number, got {}", other.kind())),
        }
    }
}
impl TryFrom<JsData> for String {
    /// The error message.
    type Error = String;
    /// A string as UTF-8.
    ///
    /// # Errors
    /// Any other kind of value, or a string with an unpaired surrogate (not valid UTF-8;
    /// take [`JsString`] through `JsData` to keep it).
    fn try_from(mut value: JsData) -> Result<Self, String> {
        // `JsData` implements `Drop`, so the text is taken out instead of moved.
        match &mut value {
            JsData::String(text) => std::mem::take(text)
                .to_utf8()
                .map_err(|_| "expected a string, got one with an unpaired surrogate".into()),
            other => Err(format!("expected a string, got {}", other.kind())),
        }
    }
}
impl TryFrom<JsData> for Vec<u8> {
    /// The error message.
    type Error = String;
    /// The bytes of a buffer or view.
    ///
    /// # Errors
    /// Any other kind of value.
    fn try_from(mut value: JsData) -> Result<Self, String> {
        match &mut value {
            JsData::Bytes(bytes) => Ok(std::mem::take(bytes)),
            other => Err(format!("expected a Uint8Array, got {}", other.kind())),
        }
    }
}
