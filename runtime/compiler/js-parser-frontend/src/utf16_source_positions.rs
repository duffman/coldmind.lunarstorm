//! Owned UTF-16 input and reversible OXC parse-view positions.
//!
//! Unpaired surrogates become Unicode escape spellings, with backslash parity
//! preserved. Cooked string values can therefore retain their original units.
//! This parse view is NOT a replacement for original template/regexp raw data:
//! consumers must recover raw tokens with `original_units`. In particular,
//! regexp Unicode identity-escape validity requires `validate_regexp_literal`
//! against original-token units before lowering.
use std::ops::Range;

/// A range in original UTF-16 units, with an exclusive end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Utf16Span {
    pub start: u32,
    pub end: u32,
}

/// Source owned independently of OXC's arena and all caller buffers.
#[derive(Clone, Debug)]
pub struct SourceInput {
    original: Vec<u16>,
    text: String,
    // UTF-8/UTF-16 scalar or normalized-escape boundaries, both monotonic.
    boundaries: Vec<(u32, u32)>,
    unpaired: Vec<Utf16Span>,
    // A CRLF enters the next line after CR, while its content starts after LF.
    lines: SourceLineMap,
    // Parser-only text with no original units, see `with_insertion`.
    inserted: Vec<Range<u32>>,
}
impl SourceInput {
    /// Build a reversible parser view. This does not certify raw-token semantics.
    pub fn from_utf16(units: &[u16]) -> Result<Self, &'static str> {
        if units.len() > u32::MAX as usize / 6 {
            return Err("source exceeds position range");
        }
        let mut text = String::new();
        let mut boundaries = vec![(0, 0)];
        let mut unpaired = Vec::new();
        let mut i = 0;
        let mut slashes = 0;
        while i < units.len() {
            let unit = units[i];
            if (0xd800..=0xdbff).contains(&unit)
                && units
                    .get(i + 1)
                    .is_some_and(|u| (0xdc00..=0xdfff).contains(u))
            {
                let scalar =
                    0x10000 + ((u32::from(unit) - 0xd800) << 10) + u32::from(units[i + 1]) - 0xdc00;
                text.push(char::from_u32(scalar).expect("validated surrogate pair"));
                i += 2;
                slashes = 0;
            } else if (0xd800..=0xdfff).contains(&unit) {
                use std::fmt::Write;
                if slashes % 2 == 0 {
                    text.push('\\');
                }
                write!(text, "u{unit:04x}").expect("writing to String");
                unpaired.push(Utf16Span {
                    start: i as u32,
                    end: i as u32 + 1,
                });
                i += 1;
                slashes = 0;
            } else {
                text.push(char::from_u32(u32::from(unit)).expect("non-surrogate unit"));
                i += 1;
                slashes = if unit == u16::from(b'\\') {
                    slashes + 1
                } else {
                    0
                };
            }
            boundaries.push((text.len() as u32, i as u32));
        }
        let lines = SourceLineMap::from_utf16(units);
        Ok(Self {
            original: units.to_vec(),
            text,
            boundaries,
            unpaired,
            lines,
            inserted: Vec::new(),
        })
    }
    /// The same source with parser-only `text` inserted at parser byte `at`. The inserted
    /// bytes map to the empty original range at `at`, and every other span keeps its
    /// original units.
    pub fn with_insertion(&self, at: usize, text: &str) -> Self {
        let (at, k) = (at as u32, text.len() as u32);
        let mut view = self.clone();
        view.text.insert_str(at as usize, text);
        let split = view.boundaries.partition_point(|&(b, _)| b <= at);
        let unit = view.boundaries[split - 1].1;
        for boundary in &mut view.boundaries[split..] {
            boundary.0 += k;
        }
        view.boundaries.insert(split, (at + k, unit));
        for range in &mut view.inserted {
            if range.start >= at {
                *range = range.start + k..range.end + k;
            }
        }
        view.inserted.push(at..at + k);
        view
    }
    /// Whether parser bytes `bytes` are parser-only text from `with_insertion`.
    pub fn is_inserted(&self, bytes: Range<u32>) -> bool {
        self.inserted
            .iter()
            .any(|r| r.start <= bytes.start && bytes.end <= r.end && bytes.start < bytes.end)
    }
    /// Whether an insertion starts at parser byte `at`.
    pub fn insertion_at(&self, at: u32) -> bool {
        self.inserted.iter().any(|r| r.start == at)
    }

    /// UTF-8 text for OXC. Raw AST text may differ from original source.
    pub fn parser_text(&self) -> &str {
        &self.text
    }
    /// Exact original units, including NUL and unpaired surrogates.
    pub fn original(&self) -> &[u16] {
        &self.original
    }
    /// Digest the exact UTF-8 parser bytes; legacy scalar input has no normalization.
    pub fn utf8_digest(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        Sha256::digest(self.text.as_bytes()).into()
    }
    /// Original ranges whose spelling was changed for OXC.
    pub fn unpaired_surrogates(&self) -> &[Utf16Span] {
        &self.unpaired
    }
    /// Map parser boundaries to original units. Interior normalized-escape
    /// offsets conservatively cover the original unit, never fabricate units.
    pub fn original_span(&self, bytes: Range<usize>) -> Option<Utf16Span> {
        if bytes.start > bytes.end || bytes.end > self.text.len() {
            return None;
        }
        let start = self
            .boundaries
            .partition_point(|&(b, _)| (b as usize) <= bytes.start)
            .checked_sub(1)?;
        let end = self
            .boundaries
            .partition_point(|&(b, _)| (b as usize) < bytes.end);
        Some(Utf16Span {
            start: self.boundaries[start].1,
            end: self.boundaries.get(end)?.1,
        })
    }
    /// Recover an original raw token from its normalized OXC span.
    /// Template CR/CRLF normalization remains the lowering consumer's duty.
    pub fn original_units(&self, bytes: Range<usize>) -> Option<&[u16]> {
        let span = self.original_span(bytes)?;
        self.original.get(span.start as usize..span.end as usize)
    }
    /// Reverse map at a Unicode scalar boundary. A surrogate pair's interior
    /// has no UTF-8 byte boundary and returns None.
    pub fn parser_offset(&self, unit: u32) -> Option<u32> {
        self.boundaries
            .binary_search_by_key(&unit, |&(_, u)| u)
            .ok()
            .map(|i| self.boundaries[i].0)
    }
    /// Reject treating the parser view as a semantically identical raw source.
    /// Use original raw tokens and original-token regexp validation instead.
    pub fn require_literal_fidelity(&self) -> Result<(), &'static str> {
        if self.unpaired.is_empty() {
            Ok(())
        } else {
            Err(
                "normalized raw tokens require original UTF-16 reconstruction and regexp validation",
            )
        }
    }
    /// Map a parser UTF-8 byte boundary to its original line and UTF-16 column.
    pub fn parser_line_column(&self, offset: u32) -> Option<(u32, u32)> {
        let original = self.original_span(offset as usize..offset as usize)?;
        self.line_column(original.start)
    }
    /// One-based original line and UTF-16 column, recognizing ECMAScript line
    /// terminators and counting CRLF as one line break.
    pub fn line_column(&self, offset: u32) -> Option<(u32, u32)> {
        if offset as usize > self.original.len() {
            return None;
        }
        self.lines.line_column(offset)
    }
}

/// The original regexp component containing a diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegexpPart {
    Pattern,
    Flags,
}
/// A regexp validation failure. Standalone spans are component-local UTF-16;
/// `SourceInput::validate_regexp_literal` returns original-file UTF-16 spans.
#[derive(Clone, Debug)]
pub struct RegexpDiagnostic {
    pub part: RegexpPart,
    pub span: Utf16Span,
    pub message: String,
}

fn quoted_units(units: &[u16]) -> String {
    use std::fmt::Write;
    let mut text = String::from("\"");
    for u in units {
        write!(text, "\\u{u:04x}").expect("writing String");
    }
    text.push('"');
    text
}
/// Validate original regexp units through OXC's constructor-pattern reader.
/// Each unit is transported as a quoted JS Unicode escape, so original pattern
/// backslashes are decoded ONCE before regexp syntax parsing. No C parser or
/// generated JavaScript program is involved. Regex ASTs from the parse view
/// must not substitute for this original-token validation.
pub fn validate_regexp_units(pattern: &[u16], flags: &[u16]) -> Result<(), RegexpDiagnostic> {
    if pattern.len() > (u32::MAX as usize - 5) / 12 || flags.len() > (u32::MAX as usize - 5) / 12 {
        return Err(RegexpDiagnostic {
            part: RegexpPart::Pattern,
            span: Utf16Span { start: 0, end: 0 },
            message: "regexp exceeds position budget".into(),
        });
    }
    let pattern_text = quoted_units(pattern);
    let flags_text = quoted_units(flags);
    let flags_offset = pattern_text.len() as u32 + 1;
    let arena = oxc_allocator::Allocator::default();
    let options = oxc_regular_expression::Options {
        pattern_span_offset: 0,
        flags_span_offset: flags_offset,
    };
    oxc_regular_expression::ConstructorParser::new(
        &arena,
        &pattern_text,
        Some(&flags_text),
        options,
    )
    .parse()
    .map(|_| ())
    .map_err(|e| {
        let (offset, len) = e
            .labels
            .first()
            .map(|l| (l.offset(), l.len()))
            .unwrap_or((0, pattern_text.len() as u32));
        let (part, base, count) = if offset >= flags_offset {
            (RegexpPart::Flags, flags_offset, flags.len())
        } else {
            (RegexpPart::Pattern, 0, pattern.len())
        };
        let start = (offset.saturating_sub(base).saturating_sub(1) / 6).min(count as u32);
        let end = offset
            .saturating_sub(base)
            .saturating_add(len)
            .saturating_sub(1)
            .div_ceil(6)
            .min(count as u32);
        RegexpDiagnostic {
            part,
            span: Utf16Span {
                start,
                end: end.max(start),
            },
            message: e.to_string(),
        }
    })
}
impl SourceInput {
    /// Validate actual pattern/flags from normalized OXC byte ranges, excluding
    /// slash delimiters. Returns diagnostics in original-file UTF-16 units.
    /// Lowering must retain original pattern/flag units for runtime RegExp
    /// construction; normalization is not a regexp semantic rewrite.
    pub fn validate_regexp_literal(
        &self,
        pattern: Range<usize>,
        flags: Range<usize>,
    ) -> Result<(), RegexpDiagnostic> {
        let invalid = || RegexpDiagnostic {
            part: RegexpPart::Pattern,
            span: Utf16Span { start: 0, end: 0 },
            message: "regexp span outside source".into(),
        };
        let pattern_span = self.original_span(pattern.clone()).ok_or_else(invalid)?;
        let flags_span = self.original_span(flags.clone()).ok_or_else(invalid)?;
        let pattern_units = self.original_units(pattern).ok_or_else(invalid)?;
        let flags_units = self.original_units(flags).ok_or_else(invalid)?;
        if let Some(i) = pattern_units
            .iter()
            .position(|u| matches!(u, 0x0a | 0x0d | 0x2028 | 0x2029))
        {
            return Err(RegexpDiagnostic {
                part: RegexpPart::Pattern,
                span: Utf16Span {
                    start: pattern_span.start + i as u32,
                    end: pattern_span.start + i as u32 + 1,
                },
                message: "line terminator in regexp literal".into(),
            });
        }
        validate_regexp_units(pattern_units, flags_units).map_err(|mut e| {
            let base = if e.part == RegexpPart::Pattern {
                pattern_span.start
            } else {
                flags_span.start
            };
            e.span.start += base;
            e.span.end += base;
            e
        })
    }
}

/// Original UTF-16 line coordinates shared by all functions in one source file.
#[derive(Clone, Debug)]
pub struct SourceLineMap {
    source_len: usize,
    lines: Vec<(u32, u32)>,
}
impl SourceLineMap {
    pub fn from_utf16(units: &[u16]) -> Self {
        let mut lines = vec![(0, 0)];
        let mut at = 0;
        while at < units.len() {
            if units[at] == 13 {
                let content = at + 1 + usize::from(units.get(at + 1) == Some(&10));
                lines.push((at as u32 + 1, content as u32));
                at = content;
            } else {
                if matches!(units[at], 10 | 0x2028 | 0x2029) {
                    lines.push((at as u32 + 1, at as u32 + 1));
                }
                at += 1;
            }
        }
        Self {
            source_len: units.len(),
            lines,
        }
    }
    pub fn line_column(&self, offset: u32) -> Option<(u32, u32)> {
        if offset as usize > self.source_len {
            return None;
        }
        let line = self.lines.partition_point(|&(start, _)| start <= offset) - 1;
        Some((
            line as u32 + 1,
            offset.saturating_sub(self.lines[line].1) + 1,
        ))
    }
}
