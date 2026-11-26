//! Classifications the contract hands back: [`Kind`], the type of a value, and
//! [`ErrorKind`], the constructor of an error a native throws.

/// The standard error constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// `Error`
    Error,
    /// `TypeError`
    Type,
    /// `RangeError`
    Range,
    /// `SyntaxError`
    Syntax,
    /// `ReferenceError`
    Reference,
}

/// Coarse type of a value, enough to convert it to a host representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `undefined`
    Undefined,
    /// `null`
    Null,
    /// A boolean.
    Bool,
    /// A number.
    Number,
    /// A string.
    String,
    /// A `Uint8Array`.
    Bytes,
    /// A callable object.
    Function,
    /// Any other object.
    Object,
    /// Symbols, big integers and engine-internal values.
    Other,
}
