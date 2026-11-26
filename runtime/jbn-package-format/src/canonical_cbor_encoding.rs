//! A bounded canonical CBOR subset: the encoding of the manifest and the
//! signature. Unsupported types and noncanonical encodings fail closed, so one
//! manifest has exactly one byte form and its digest identifies it.
//!
//! Ported from jetbunny (`src/jbn/wire.rs`) unchanged in behaviour. A map is
//! encoded with the integer keys `0..n` in order, so a struct's fields are its
//! positions.

use crate::Error;

/// One CBOR value of the subset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Value {
    /// Unsigned integer.
    U(u64),
    /// Text string.
    Text(String),
    /// Byte string.
    Bytes(Vec<u8>),
    /// Array.
    Array(Vec<Value>),
    /// Map with keys `0..n`; holds the values in key order.
    Map(Vec<Value>),
}

/// # Returns
/// A format error naming what was expected.
fn expected(what: &str) -> Error {
    Error::Format(format!("expected {what}"))
}

impl Value {
    /// # Errors
    /// Not an unsigned integer.
    pub fn uint(&self) -> Result<u64, Error> {
        if let Self::U(n) = self { Ok(*n) } else { Err(expected("an unsigned integer")) }
    }

    /// # Errors
    /// Not a text string.
    pub fn text(&self) -> Result<&str, Error> {
        if let Self::Text(s) = self { Ok(s) } else { Err(expected("text")) }
    }

    /// # Errors
    /// Not a byte string.
    pub fn bytes(&self) -> Result<&[u8], Error> {
        if let Self::Bytes(b) = self { Ok(b) } else { Err(expected("bytes")) }
    }

    /// # Errors
    /// Not an array.
    pub fn array(&self) -> Result<&[Value], Error> {
        if let Self::Array(a) = self { Ok(a) } else { Err(expected("an array")) }
    }

    /// # Errors
    /// Not a map of exactly `count` entries.
    pub fn map(&self, count: usize) -> Result<&[Value], Error> {
        match self {
            Self::Map(a) if a.len() == count => Ok(a),
            _ => Err(expected(&format!("a map of {count} entries"))),
        }
    }
}

/// # Returns
/// The canonical encoding of `value`.
pub(crate) fn encode(value: &Value) -> Vec<u8> {
    /// Encodes one value; writing to a `Vec` cannot fail.
    fn put(v: &Value, e: &mut minicbor::Encoder<Vec<u8>>) {
        match v {
            Value::U(n) => {
                e.u64(*n).expect("vec write");
            }
            Value::Text(s) => {
                e.str(s).expect("vec write");
            }
            Value::Bytes(b) => {
                e.bytes(b).expect("vec write");
            }
            Value::Array(a) => {
                e.array(a.len() as u64).expect("vec write");
                for v in a {
                    put(v, e);
                }
            }
            Value::Map(a) => {
                e.map(a.len() as u64).expect("vec write");
                for (k, v) in a.iter().enumerate() {
                    e.u64(k as u64).expect("vec write");
                    put(v, e);
                }
            }
        }
    }
    let mut e = minicbor::Encoder::new(Vec::new());
    put(value, &mut e);
    e.into_writer()
}

/// Decodes canonical bytes.
///
/// # Errors
/// [`Error::Limit`] past 16 levels or 200 000 items; [`Error::Format`] for any
/// type outside the subset, trailing bytes, or an encoding that is not canonical
/// (re-encoding must give the same bytes).
pub(crate) fn decode(bytes: &[u8]) -> Result<Value, Error> {
    use minicbor::data::Type;
    /// Reads one value within the depth and item budget.
    fn read(d: &mut minicbor::Decoder<'_>, depth: usize, budget: &mut usize) -> Result<Value, Error> {
        if depth > 16 || *budget == 0 {
            return Err(Error::Limit);
        }
        *budget -= 1;
        let err = |e: minicbor::decode::Error| Error::Format(e.to_string());
        Ok(match d.datatype().map_err(err)? {
            Type::U8 | Type::U16 | Type::U32 | Type::U64 => Value::U(d.u64().map_err(err)?),
            Type::String => Value::Text(d.str().map_err(err)?.to_owned()),
            Type::Bytes => Value::Bytes(d.bytes().map_err(err)?.to_vec()),
            Type::Array => {
                let n = d.array().map_err(err)?.ok_or_else(|| expected("a definite array"))?;
                if n > *budget as u64 {
                    return Err(Error::Limit);
                }
                let mut a = Vec::new();
                for _ in 0..n {
                    a.push(read(d, depth + 1, budget)?);
                }
                Value::Array(a)
            }
            Type::Map => {
                let n = d.map().map_err(err)?.ok_or_else(|| expected("a definite map"))?;
                if n > *budget as u64 {
                    return Err(Error::Limit);
                }
                let mut a = Vec::new();
                for k in 0..n {
                    if d.u64().map_err(err)? != k {
                        return Err(expected("map keys 0..n in order"));
                    }
                    a.push(read(d, depth + 1, budget)?);
                }
                Value::Map(a)
            }
            other => return Err(Error::Format(format!("unsupported CBOR type {other}"))),
        })
    }
    let mut d = minicbor::Decoder::new(bytes);
    let value = read(&mut d, 0, &mut 200_000)?;
    if d.position() != bytes.len() || encode(&value) != bytes {
        return Err(Error::Format(String::from("noncanonical CBOR")));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_and_noncanonical_forms_are_refused() {
        let value = Value::Map(vec![
            Value::U(1),
            Value::Text("a".into()),
            Value::Array(vec![Value::Bytes(vec![1, 2]), Value::U(300)]),
        ]);
        let bytes = encode(&value);
        assert_eq!(decode(&bytes).expect("decodes"), value);
        // 1 encoded with a needless extra byte (0x18 0x01) is not canonical.
        assert!(decode(&[0x18, 0x01]).is_err());
        // Trailing bytes.
        assert!(decode(&[0x01, 0x01]).is_err());
        // Map keys out of order.
        assert!(decode(&[0xa1, 0x01, 0x01]).is_err());
        // Floats are outside the subset.
        assert!(decode(&[0xf9, 0x3c, 0x00]).is_err());
    }
}
