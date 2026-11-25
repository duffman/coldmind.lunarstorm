//! Pure, bounded parsing for non-executing debugger paths.
use crate::{debug::DebugError, engine::JsString};

/// A binding or `this`, followed by own-property traversal steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupPath {
    /// Starting binding or frame receiver.
    pub root: LookupRoot,
    /// Ordered member accesses, with no computed expressions.
    pub steps: Vec<LookupStep>,
}

/// The starting point of a safe lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookupRoot {
    /// A lexical identifier.
    Identifier(JsString),
    /// The current frame receiver.
    This,
}

/// A literal own-property access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookupStep {
    /// A dot name or decoded literal string key, preserving UTF-16 units.
    Named(JsString),
    /// A nonnegative decimal integer fitting in `u32`.
    Index(u32),
}

impl LookupPath {
    /// Parses a restricted path without invoking an engine or executing code.
    ///
    /// Identifiers use the ASCII subset (`A-Z`, `a-z`, `_`, `$`, then digits
    /// also). Unicode and escaped identifiers are unsupported; quoted keys retain
    /// arbitrary UTF-16, including lone surrogates. Strings support explicit JS
    /// character, hex, Unicode and line-continuation escapes, but reject legacy
    /// octal and identity escapes. Indices use canonical decimal notation.
    /// Inputs above 16,384 UTF-16 units or 64 member steps return `Capacity`;
    /// malformed or unsupported syntax returns `InvalidRequest`.
    pub fn parse(input: &JsString) -> Result<Self, DebugError> {
        let units = input.as_units();
        if units.len() > 16_384 {
            return Err(DebugError::Capacity);
        }
        let mut parser = Parser { units, offset: 0 };
        parser.whitespace();
        let name = parser.identifier()?;
        let root = if name.as_units() == [116, 104, 105, 115] {
            LookupRoot::This
        } else {
            // Root keywords cannot denote lexical bindings in supported JS modes.
            let text = name.to_utf8().map_err(|_| DebugError::InvalidRequest)?;
            if matches!(
                text.as_str(),
                "await"
                    | "break"
                    | "case"
                    | "catch"
                    | "class"
                    | "const"
                    | "continue"
                    | "debugger"
                    | "default"
                    | "delete"
                    | "do"
                    | "else"
                    | "enum"
                    | "export"
                    | "extends"
                    | "false"
                    | "finally"
                    | "for"
                    | "function"
                    | "if"
                    | "implements"
                    | "import"
                    | "in"
                    | "instanceof"
                    | "interface"
                    | "let"
                    | "new"
                    | "null"
                    | "package"
                    | "private"
                    | "protected"
                    | "public"
                    | "return"
                    | "static"
                    | "super"
                    | "switch"
                    | "throw"
                    | "true"
                    | "try"
                    | "typeof"
                    | "var"
                    | "void"
                    | "while"
                    | "with"
                    | "yield"
            ) {
                return Err(DebugError::InvalidRequest);
            }
            LookupRoot::Identifier(name)
        };
        let mut steps = Vec::new();
        loop {
            parser.whitespace();
            if parser.peek().is_none() {
                return Ok(Self { root, steps });
            }
            if steps.len() == 64 {
                return Err(DebugError::Capacity);
            }
            let step = match parser.take()? {
                46 => {
                    parser.whitespace();
                    LookupStep::Named(parser.identifier()?)
                }
                91 => {
                    parser.whitespace();
                    let step = match parser.peek() {
                        Some(34 | 39) => LookupStep::Named(parser.string()?),
                        Some(48..=57) => LookupStep::Index(parser.index()?),
                        _ => return Err(DebugError::InvalidRequest),
                    };
                    parser.whitespace();
                    if parser.take()? != 93 {
                        return Err(DebugError::InvalidRequest);
                    }
                    step
                }
                _ => return Err(DebugError::InvalidRequest),
            };
            steps.push(step);
        }
    }
}

struct Parser<'a> {
    units: &'a [u16],
    offset: usize,
}
impl Parser<'_> {
    fn peek(&self) -> Option<u16> {
        self.units.get(self.offset).copied()
    }
    fn take(&mut self) -> Result<u16, DebugError> {
        let unit = self.peek().ok_or(DebugError::InvalidRequest)?;
        self.offset += 1;
        Ok(unit)
    }
    fn whitespace(&mut self) {
        while matches!(
            self.peek(),
            Some(
                9..=13
                | 32
                | 0xa0
                | 0x1680
                | 0x2000..=0x200a
                | 0x2028
                | 0x2029
                | 0x202f
                | 0x205f
                | 0x3000
                | 0xfeff,
            )
        ) {
            self.offset += 1;
        }
    }
    fn identifier(&mut self) -> Result<JsString, DebugError> {
        let start = self.offset;
        if !matches!(self.take()?, 65..=90 | 97..=122 | 95 | 36) {
            return Err(DebugError::InvalidRequest);
        }
        while matches!(self.peek(), Some(65..=90 | 97..=122 | 95 | 36 | 48..=57)) {
            self.offset += 1;
        }
        Ok(JsString::from_units(
            self.units[start..self.offset].to_vec(),
        ))
    }
    fn index(&mut self) -> Result<u32, DebugError> {
        let first = self.take()?;
        let mut value = u32::from(first - 48);
        while let Some(unit @ 48..=57) = self.peek() {
            if first == 48 {
                return Err(DebugError::InvalidRequest);
            }
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(u32::from(unit - 48)))
                .ok_or(DebugError::InvalidRequest)?;
            self.offset += 1;
        }
        Ok(value)
    }
    fn hex_digit(&mut self) -> Result<u32, DebugError> {
        match self.take()? {
            unit @ 48..=57 => Ok(u32::from(unit - 48)),
            unit @ 65..=70 => Ok(u32::from(unit - 65 + 10)),
            unit @ 97..=102 => Ok(u32::from(unit - 97 + 10)),
            _ => Err(DebugError::InvalidRequest),
        }
    }
    fn hex(&mut self, count: usize) -> Result<u16, DebugError> {
        let mut value = 0;
        for _ in 0..count {
            value = value * 16 + self.hex_digit()? as u16;
        }
        Ok(value)
    }
    fn string(&mut self) -> Result<JsString, DebugError> {
        let quote = self.take()?;
        let mut units = Vec::new();
        loop {
            let unit = self.take()?;
            if unit == quote {
                return Ok(JsString::from_units(units));
            }
            match unit {
                10 | 13 | 0x2028 | 0x2029 => return Err(DebugError::InvalidRequest),
                92 => match self.take()? {
                    unit @ (34 | 39 | 92) => units.push(unit),
                    98 => units.push(8),
                    102 => units.push(12),
                    110 => units.push(10),
                    114 => units.push(13),
                    116 => units.push(9),
                    118 => units.push(11),
                    48 if !matches!(self.peek(), Some(48..=57)) => units.push(0),
                    120 => units.push(self.hex(2)?),
                    117 if self.peek() == Some(123) => {
                        self.offset += 1;
                        let mut value = self.hex_digit()?;
                        while self.peek() != Some(125) {
                            let digit = self.hex_digit()?;
                            value = value
                                .checked_mul(16)
                                .and_then(|v| v.checked_add(digit))
                                .filter(|v| *v <= 0x10ffff)
                                .ok_or(DebugError::InvalidRequest)?;
                        }
                        self.offset += 1;
                        if value <= 0xffff {
                            units.push(value as u16);
                        } else {
                            let value = value - 0x10000;
                            units.push(0xd800 + (value >> 10) as u16);
                            units.push(0xdc00 + (value & 0x3ff) as u16);
                        }
                    }
                    117 => units.push(self.hex(4)?),
                    13 => {
                        if self.peek() == Some(10) {
                            self.offset += 1;
                        }
                    }
                    10 | 0x2028 | 0x2029 => {}
                    _ => return Err(DebugError::InvalidRequest),
                },
                _ => units.push(unit),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<LookupPath, DebugError> {
        LookupPath::parse(&JsString::from_utf8(text))
    }
    fn named(text: &str) -> LookupStep {
        LookupStep::Named(JsString::from_utf8(text))
    }

    #[test]
    fn parses_binding_this_and_literal_members() {
        assert_eq!(
            parse("  value . child [ 'key' ] [42] ").unwrap(),
            LookupPath {
                root: LookupRoot::Identifier(JsString::from_utf8("value")),
                steps: vec![named("child"), named("key"), LookupStep::Index(42)],
            }
        );
        assert_eq!(
            parse("this.default[0][4294967295]").unwrap(),
            LookupPath {
                root: LookupRoot::This,
                steps: vec![
                    named("default"),
                    LookupStep::Index(0),
                    LookupStep::Index(u32::MAX)
                ],
            }
        );
        assert!(parse("_$value09").is_ok());
        assert!(parse("undefined").is_ok());
        assert_eq!(parse("this").unwrap().root, LookupRoot::This);
        assert!(parse("\u{feff}value\u{2028}.\u{a0}name").is_ok());
        assert_eq!(parse("obj['0']").unwrap().steps, vec![named("0")]);
        assert_eq!(parse("obj['é']").unwrap().steps, vec![named("é")]);
    }

    #[test]
    fn decodes_javascript_string_escapes_losslessly() {
        let path = parse(r#"obj['\'\"\\\b\f\n\r\t\v\0\x41\u0042\u{1F600}\uD800']"#).unwrap();
        assert_eq!(
            path.steps,
            vec![LookupStep::Named(JsString::from_units(vec![
                39, 34, 92, 8, 12, 10, 13, 9, 11, 0, 65, 66, 0xd83d, 0xde00, 0xd800,
            ]))]
        );
        assert_eq!(
            parse("obj['a\\\nb\\\r\nc']").unwrap().steps,
            vec![named("abc")]
        );
        let mut units: Vec<u16> = "obj['".encode_utf16().collect();
        units.extend([0xd800, 0, 0xdc00]);
        units.extend("']".encode_utf16());
        assert_eq!(
            LookupPath::parse(&JsString::from_units(units))
                .unwrap()
                .steps,
            vec![LookupStep::Named(JsString::from_units(vec![
                0xd800, 0, 0xdc00
            ]))]
        );
    }

    #[test]
    fn rejects_executable_and_unsupported_syntax() {
        for text in [
            "",
            " ",
            "obj()",
            "obj.fn()",
            "obj = 1",
            "obj++",
            "obj + key",
            "obj[key]",
            "obj[1 + 1]",
            "obj[`key`]",
            "obj?.key",
            "obj/*x*/.key",
            "(obj)",
            "new Object",
            "super.key",
            "null",
            "true",
            "return",
            "obj;",
            "obj.",
            "obj[",
            "obj[-1]",
            "obj[+1]",
            "obj[1.0]",
            "obj[1e2]",
            "obj[0x10]",
            "obj[01]",
            "obj[4294967296]",
            "obj[1n]",
            "obj['x' + 'y']",
            "obj['x']tail",
            "obj['unterminated]",
            "obj['line\nbreak']",
            r"obj['\uZZZZ']",
            r"obj['\x0']",
            r"obj['\u{110000}']",
            r"obj['\u{}']",
            r"obj['\u{FFFFFFFFFFFFFFFFFFFFFFFF}']",
            r"obj['\u{41']",
            r"obj['\8']",
            r"obj['\1']",
            r"obj['\01']",
            r"obj['\q']",
            r"\u0061",
            "é.key",
            "obj.é",
        ] {
            assert_eq!(parse(text), Err(DebugError::InvalidRequest), "{text:?}");
        }
    }

    #[test]
    fn bounds_units_and_member_steps() {
        assert!(parse(&"x".repeat(16_384)).is_ok());
        assert_eq!(parse(&"x".repeat(16_385)), Err(DebugError::Capacity));
        assert!(parse(&format!("x{}", ".x".repeat(64))).is_ok());
        assert_eq!(
            parse(&format!("x{}", ".x".repeat(65))),
            Err(DebugError::Capacity)
        );
    }
}
