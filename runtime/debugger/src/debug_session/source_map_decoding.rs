//! Source map decoding: basic and indexed maps parsed losslessly from JSON, VLQ
//! segments, and lookup from generated to original positions.

use super::DebugError;
use crate::engine::{JsString, OriginalSource, SourceDigest, SourceMapAttachment};
use serde::{
    Deserialize, Deserializer,
    de::{self, Visitor},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    mem::size_of,
};
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Position {
    pub line: u32,
    pub column: u32,
}
#[derive(Debug)]
pub(super) struct Original {
    pub origin: Option<String>,
    pub text: Option<JsString>,
    pub digest: Option<SourceDigest>,
}
#[derive(Debug)]
pub(super) struct Segment {
    pub generated: Position,
    pub original: Option<(usize, Position)>,
}
#[derive(Debug)]
pub(super) struct MapData {
    pub sources: Vec<Original>,
    pub segments: Vec<Segment>,
}
#[derive(Debug)]
pub(super) struct SourceMaps {
    pub root: MapData,
    pub preceding: Vec<(String, MapData)>,
}

fn bad<T>() -> Result<T, DebugError> {
    Err(DebugError::InvalidMap)
}
fn add(a: usize, b: usize) -> Result<usize, DebugError> {
    a.checked_add(b).ok_or(DebugError::Capacity)
}
fn allocation_bytes(count: usize, item_bytes: usize) -> Result<usize, DebugError> {
    count.checked_mul(item_bytes).ok_or(DebugError::Capacity)
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct LosslessString(pub(super) JsString);
impl<'de> Deserialize<'de> for LosslessString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BytesVisitor;
        impl<'de> Visitor<'de> for BytesVisitor {
            type Value = LosslessString;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON string with lossless UTF-16 content")
            }
            fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
                let mut units = Vec::new();
                let mut start = 0;
                let mut cursor = 0;
                while cursor + 2 < bytes.len() {
                    if bytes[cursor] == 0xed
                        && (0xa0..=0xbf).contains(&bytes[cursor + 1])
                        && (0x80..=0xbf).contains(&bytes[cursor + 2])
                    {
                        let prefix =
                            std::str::from_utf8(&bytes[start..cursor]).map_err(E::custom)?;
                        units.extend(prefix.encode_utf16());
                        let surrogate = (u16::from(bytes[cursor] & 15) << 12)
                            | (u16::from(bytes[cursor + 1] & 63) << 6)
                            | u16::from(bytes[cursor + 2] & 63);
                        units.push(surrogate);
                        cursor += 3;
                        start = cursor;
                    } else {
                        cursor += 1;
                    }
                }
                let suffix = std::str::from_utf8(&bytes[start..]).map_err(E::custom)?;
                units.extend(suffix.encode_utf16());
                Ok(LosslessString(JsString::from_units(units)))
            }
        }
        // serde_json deliberately preserves lone surrogate escapes as WTF-8
        // through deserialize_bytes; the visitor intentionally has no visit_seq.
        deserializer.deserialize_bytes(BytesVisitor)
    }
}
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RawMap {
    version: u32,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    source_root: Option<String>,
    #[serde(default)]
    sources: Option<Vec<Option<String>>>,
    #[serde(default)]
    sources_content: Option<Vec<Option<LosslessString>>>,
    #[serde(default)]
    names: Option<Vec<LosslessString>>,
    #[serde(default)]
    mappings: Option<String>,
    #[serde(default)]
    sections: Option<Vec<RawSection>>,
    #[serde(default)]
    ignore_list: Option<Vec<u32>>,
    #[serde(default, rename = "x_google_ignoreList")]
    google_ignore_list: Option<Vec<u32>>,
}
#[derive(Debug, Deserialize, PartialEq, Eq)]
struct RawSection {
    offset: RawOffset,
    map: Box<RawMap>,
    #[serde(default)]
    url: Option<String>,
}
#[derive(Debug, Deserialize, PartialEq, Eq)]
struct RawOffset {
    line: u32,
    column: u32,
}
fn parse_json(raw: &[u8]) -> Result<RawMap, DebugError> {
    // JSON's external encoding remains UTF-8; only escaped string contents may
    // decode to unpaired UTF-16. Invalid literal UTF-8 is never admitted.
    std::str::from_utf8(raw).map_err(|_| DebugError::InvalidMap)?;
    serde_json::from_slice(raw).map_err(|_| DebugError::InvalidMap)
}
pub(super) fn equivalent_json(a: &[u8], b: &[u8]) -> bool {
    match (parse_json(a), parse_json(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
fn resolve(base: &Url, path: &str) -> Result<String, DebugError> {
    // URL parsing is metadata-only. No fetch or filesystem lookup is performed.
    if path.chars().any(|c| c.is_control()) {
        return bad();
    }
    base.join(path)
        .map(|url| url.to_string())
        .map_err(|_| DebugError::InvalidMap)
}
fn source_uri(base: &Url, root: Option<&str>, path: &str) -> Result<String, DebugError> {
    let joined = match root {
        Some(root) if !root.is_empty() => format!(
            "{}{}{}",
            root,
            if root.ends_with('/') { "" } else { "/" },
            path
        ),
        _ => path.to_owned(),
    };
    resolve(base, &joined)
}

/// UTF-16 line lengths, treating CRLF as one separator and JavaScript's other
/// line terminators as separators. The last line exists even for empty source.
fn line_lengths(text: &JsString) -> Vec<u32> {
    let mut lines = Vec::new();
    let mut length = 0u32;
    let mut iter = text.as_units().iter().peekable();
    while let Some(unit) = iter.next() {
        match unit {
            0x0d => {
                if iter.peek() == Some(&&0x0a) {
                    iter.next();
                }
                lines.push(length);
                length = 0;
            }
            0x0a | 0x2028 | 0x2029 => {
                lines.push(length);
                length = 0;
            }
            _ => length = length.saturating_add(1),
        }
    }
    lines.push(length);
    lines
}
fn validate_bounds(lengths: &[u32], position: Position) -> Result<(), DebugError> {
    if lengths
        .get(position.line as usize)
        .is_some_and(|length| position.column <= *length)
    {
        Ok(())
    } else {
        bad()
    }
}
pub(super) fn validate_position(text: &JsString, position: Position) -> Result<(), DebugError> {
    let mut current = Position { line: 0, column: 0 };
    let mut units = text.as_units().iter().peekable();
    loop {
        if current == position {
            return Ok(());
        }
        let Some(unit) = units.next() else {
            return bad();
        };
        match unit {
            0x0d => {
                if units.peek() == Some(&&0x0a) {
                    units.next();
                }
                if current.line == position.line {
                    return bad();
                }
                current.line = current.line.checked_add(1).ok_or(DebugError::InvalidMap)?;
                current.column = 0;
            }
            0x0a | 0x2028 | 0x2029 => {
                if current.line == position.line {
                    return bad();
                }
                current.line = current.line.checked_add(1).ok_or(DebugError::InvalidMap)?;
                current.column = 0;
            }
            _ => {
                current.column = current
                    .column
                    .checked_add(1)
                    .ok_or(DebugError::InvalidMap)?;
            }
        }
    }
}
fn offset(position: Position, by: Position) -> Result<Position, DebugError> {
    Ok(Position {
        line: position
            .line
            .checked_add(by.line)
            .ok_or(DebugError::InvalidMap)?,
        column: if position.line == 0 {
            position
                .column
                .checked_add(by.column)
                .ok_or(DebugError::InvalidMap)?
        } else {
            position.column
        },
    })
}
fn digit(byte: u8) -> Result<u8, DebugError> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => bad(),
    }
}
fn vlq(bytes: &[u8], cursor: &mut usize) -> Result<i64, DebugError> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        let unit = digit(*bytes.get(*cursor).ok_or(DebugError::InvalidMap)?)?;
        *cursor += 1;
        let bits = u64::from(unit & 31);
        if shift >= 64 || bits > (u64::MAX >> shift) {
            return bad();
        }
        value |= bits << shift;
        if unit & 32 == 0 {
            break;
        }
        shift = shift.checked_add(5).ok_or(DebugError::InvalidMap)?;
    }
    if value > u64::from(u32::MAX) {
        return bad();
    }
    let magnitude = i64::try_from(value >> 1).map_err(|_| DebugError::InvalidMap)?;
    // ECMA-426 reserves signed zero for the minimum signed 32-bit value.
    Ok(if value == 1 {
        i64::from(i32::MIN)
    } else if value & 1 == 1 {
        -magnitude
    } else {
        magnitude
    })
}
fn delta(previous: &mut i64, change: i64) -> Result<u32, DebugError> {
    *previous = previous.checked_add(change).ok_or(DebugError::InvalidMap)?;
    u32::try_from(*previous).map_err(|_| DebugError::InvalidMap)
}
fn decode_mappings(
    encoded: &str,
    sources: usize,
    names: usize,
) -> Result<Vec<Segment>, DebugError> {
    let mut decoded = Vec::new();
    let (mut source, mut original_line, mut original_column, mut name) = (0i64, 0i64, 0i64, 0i64);
    for (line_index, line) in encoded.split(';').enumerate() {
        let generated_line = u32::try_from(line_index).map_err(|_| DebugError::InvalidMap)?;
        let mut column = 0i64;
        if line.is_empty() {
            continue;
        }
        for encoded_segment in line.split(',') {
            if encoded_segment.is_empty() {
                return bad();
            }
            let mut cursor = 0;
            let mut fields = Vec::with_capacity(5);
            while cursor < encoded_segment.len() {
                if fields.len() == 5 {
                    return bad();
                }
                fields.push(vlq(encoded_segment.as_bytes(), &mut cursor)?);
            }
            if !matches!(fields.len(), 1 | 4 | 5) {
                return bad();
            }
            let generated = Position {
                line: generated_line,
                column: delta(&mut column, fields[0])?,
            };
            let original = if fields.len() > 1 {
                let index = delta(&mut source, fields[1])? as usize;
                if index >= sources {
                    return bad();
                }
                let line = delta(&mut original_line, fields[2])?;
                let column = delta(&mut original_column, fields[3])?;
                if fields.len() == 5 && delta(&mut name, fields[4])? as usize >= names {
                    return bad();
                }
                Some((index, Position { line, column }))
            } else {
                None
            };
            decoded.push(Segment {
                generated,
                original,
            });
        }
    }
    // Signed generated deltas can arrive out of order. Stable sorting retains
    // every original at duplicate generated coordinates in encoded order.
    decoded.sort_by_key(|segment| segment.generated);
    Ok(decoded)
}

fn parse_map(map: &RawMap, base: &Url, depth: usize) -> Result<MapData, DebugError> {
    if depth > 64 || map.version != 3 {
        return bad();
    }
    if let Some(sections) = &map.sections {
        if map.mappings.is_some() || map.sources.is_some() || map.source_root.is_some() {
            return bad();
        }
        let mut result = MapData {
            sources: Vec::new(),
            segments: Vec::new(),
        };
        let mut previous_offset = None;
        for section in sections {
            if section.url.is_some() {
                return bad();
            }
            let by = Position {
                line: section.offset.line,
                column: section.offset.column,
            };
            if previous_offset.is_some_and(|old| by <= old)
                || result
                    .segments
                    .last()
                    .is_some_and(|last| last.generated >= by)
            {
                return bad();
            }
            previous_offset = Some(by);
            let mut child = parse_map(&section.map, base, depth + 1)?;
            let source_offset = result.sources.len();
            // An unmapped boundary prevents a preceding section's last segment from
            // leaking into an unmapped prefix/empty section on the same line.
            let needs_boundary = child
                .segments
                .first()
                .is_none_or(|first| first.generated != Position { line: 0, column: 0 });
            if needs_boundary {
                result.segments.push(Segment {
                    generated: by,
                    original: None,
                });
            }
            for mut segment in child.segments.drain(..) {
                segment.generated = offset(segment.generated, by)?;
                if let Some((index, _)) = &mut segment.original {
                    *index = index
                        .checked_add(source_offset)
                        .ok_or(DebugError::InvalidMap)?;
                }
                result.segments.push(segment);
            }
            result.sources.append(&mut child.sources);
        }
        return Ok(result);
    }
    let sources = map.sources.as_deref().unwrap_or_default();
    let content = map.sources_content.as_deref().unwrap_or_default();
    let mut originals = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        let origin = source
            .as_ref()
            .map(|path| source_uri(base, map.source_root.as_deref(), path))
            .transpose()?;
        let text = content
            .get(index)
            .and_then(Option::as_ref)
            .map(|text| text.0.clone());
        let digest = text.as_ref().map(SourceDigest::of_utf16);
        originals.push(Original {
            origin,
            text,
            digest,
        });
    }
    for indices in [&map.ignore_list, &map.google_ignore_list]
        .into_iter()
        .flatten()
    {
        if indices.iter().any(|index| *index as usize >= sources.len()) {
            return bad();
        }
    }
    Ok(MapData {
        sources: originals,
        segments: decode_mappings(
            map.mappings.as_deref().ok_or(DebugError::InvalidMap)?,
            sources.len(),
            map.names.as_ref().map_or(0, Vec::len),
        )?,
    })
}
impl MapData {
    fn apply_supplied(&mut self, supplied: &[OriginalSource]) -> Result<(), DebugError> {
        let mut supplied_text = BTreeMap::new();
        for original in supplied {
            if let Some(text) = &original.text
                && supplied_text
                    .insert(original.origin.as_str(), text)
                    .is_some_and(|prior| prior != text)
            {
                return bad();
            }
        }
        for source in &mut self.sources {
            if let Some(text) = source
                .origin
                .as_ref()
                .and_then(|origin| supplied_text.get(origin.as_str()))
            {
                if source
                    .text
                    .as_ref()
                    .is_some_and(|existing| existing != *text)
                {
                    return bad();
                }
                if source.text.is_none() {
                    source.text = Some((*text).clone());
                }
            }
        }
        for source in &mut self.sources {
            source.digest = source.text.as_ref().map(SourceDigest::of_utf16);
        }
        let bounds: Vec<_> = self
            .sources
            .iter()
            .map(|source| source.text.as_ref().map(line_lengths))
            .collect();
        for segment in &self.segments {
            if let Some((index, position)) = segment.original
                && let Some(lengths) = &bounds[index]
            {
                validate_bounds(lengths, position)?;
            }
        }
        Ok(())
    }
    pub fn lookup_all(&self, position: Position) -> Vec<(&Original, Position)> {
        let end = self
            .segments
            .partition_point(|segment| segment.generated <= position);
        let Some(last) = end
            .checked_sub(1)
            .and_then(|index| self.segments.get(index))
        else {
            return Vec::new();
        };
        if last.generated.line != position.line {
            return Vec::new();
        }
        let start =
            self.segments[..end].partition_point(|segment| segment.generated < last.generated);
        self.segments[start..end]
            .iter()
            .filter_map(|segment| {
                let (index, original) = segment.original?;
                let source = &self.sources[index];
                source.origin.as_ref()?;
                Some((source, original))
            })
            .collect()
    }
    fn footprint(&self) -> Result<usize, DebugError> {
        let mut bytes = allocation_bytes(self.sources.capacity(), size_of::<Original>())?;
        let segments = allocation_bytes(self.segments.capacity(), size_of::<Segment>())?;
        bytes = add(bytes, segments)?;
        for source in &self.sources {
            bytes = add(bytes, source.origin.as_ref().map_or(0, String::capacity))?;
            let text = source.text.as_ref().map_or(0, JsString::capacity_bytes);
            bytes = add(bytes, text)?;
        }
        Ok(bytes)
    }
}
impl SourceMaps {
    pub fn parse(
        origin: &str,
        generated: &JsString,
        attachment: &SourceMapAttachment,
    ) -> Result<Self, DebugError> {
        let base = Url::parse(origin).map_err(|_| DebugError::InvalidMap)?;
        let json = parse_json(&attachment.json)?;
        // The v3 file field has no universally defined path convention. This
        // host contract accepts only a URL resolved against the attached source
        // that actually identifies that generated source; mismatches fail closed.
        if let Some(file) = &json.file
            && resolve(&base, file)? != base.as_str()
        {
            return bad();
        }
        let mut root = parse_map(&json, &base, 0)?;
        root.apply_supplied(&attachment.originals)?;
        let generated_bounds = line_lengths(generated);
        for segment in &root.segments {
            validate_bounds(&generated_bounds, segment.generated)?;
        }
        let mut preceding: Vec<(String, MapData)> = Vec::new();
        for raw in &attachment.preceding_maps {
            let json = parse_json(raw)?;
            let file = json.file.as_deref().ok_or(DebugError::InvalidMap)?;
            let target = resolve(&base, file)?;
            if target == origin || preceding.iter().any(|(old, _)| old == &target) {
                return bad();
            }
            let candidates = root
                .sources
                .iter()
                .chain(preceding.iter().flat_map(|(_, map)| &map.sources));
            let targets: Vec<_> = candidates
                .filter(|source| source.origin.as_ref() == Some(&target))
                .collect();
            if targets.is_empty() {
                return bad();
            }
            let mut map = parse_map(
                &json,
                &Url::parse(&target).map_err(|_| DebugError::InvalidMap)?,
                0,
            )?;
            map.apply_supplied(&attachment.originals)?;
            for source in &targets {
                if let Some(text) = &source.text {
                    let bounds = line_lengths(text);
                    for segment in &map.segments {
                        validate_bounds(&bounds, segment.generated)?;
                    }
                }
            }
            preceding.push((target, map));
        }
        let result = Self { root, preceding };
        // Conflicting contents for one URI are ambiguous and cannot be merged.
        let all: Vec<_> = result
            .root
            .sources
            .iter()
            .chain(result.preceding.iter().flat_map(|(_, map)| &map.sources))
            .collect();
        let mut known = BTreeMap::new();
        for source in &all {
            if let (Some(origin), Some(text)) = (&source.origin, &source.text)
                && known
                    .insert(origin.as_str(), text)
                    .is_some_and(|prior| prior != text)
            {
                return bad();
            }
        }
        // Every supplied original must name actual mapping input, and duplicate
        // supplied contents must agree even when an unused entry has no mapping.
        for original in &attachment.originals {
            if !all
                .iter()
                .any(|source| source.origin.as_ref() == Some(&original.origin))
            {
                return bad();
            }
        }
        result.validate_graph()?;
        Ok(result)
    }
    fn validate_graph(&self) -> Result<(), DebugError> {
        let targets: BTreeMap<_, _> = self
            .preceding
            .iter()
            .enumerate()
            .map(|(index, (target, _))| (target.as_str(), index))
            .collect();
        let edges: Vec<Vec<usize>> = self
            .preceding
            .iter()
            .map(|(_, map)| {
                map.sources
                    .iter()
                    .filter_map(|source| {
                        source
                            .origin
                            .as_ref()
                            .and_then(|origin| targets.get(origin.as_str()).copied())
                    })
                    .collect()
            })
            .collect();
        let mut color = vec![0u8; edges.len()];
        for start in 0..edges.len() {
            let mut pending = vec![(start, false)];
            while let Some((node, finishing)) = pending.pop() {
                if finishing {
                    color[node] = 2;
                    continue;
                }
                if color[node] == 1 {
                    return bad();
                }
                if color[node] == 2 {
                    continue;
                }
                color[node] = 1;
                pending.push((node, true));
                for next in edges[node].iter().rev() {
                    pending.push((*next, false));
                }
            }
        }
        Ok(())
    }
    pub fn trace<'a>(
        &'a self,
        source: &'a Original,
        position: Position,
    ) -> Result<Vec<(&'a Original, Position)>, DebugError> {
        let mut result = Vec::new();
        let mut pending = vec![(source, position)];
        let mut expanded = BTreeSet::new();
        while let Some((source, position)) = pending.pop() {
            let origin = source.origin.as_ref().ok_or(DebugError::InvalidMap)?;
            let Some((index, (_, map))) = self
                .preceding
                .iter()
                .enumerate()
                .find(|(_, (target, _))| target == origin)
            else {
                result.push((source, position));
                continue;
            };
            // Graph acyclicity was validated before admission. Deduplicating an
            // expanded state prevents exponential work when transform paths merge.
            if !expanded.insert((index, position)) {
                continue;
            }
            for (next, mapped) in map.lookup_all(position).into_iter().rev() {
                pending.push((next, mapped));
            }
        }
        Ok(result)
    }
    pub fn footprint(&self) -> Result<usize, DebugError> {
        let mut bytes = self.root.footprint()?;
        let preceding =
            allocation_bytes(self.preceding.capacity(), size_of::<(String, MapData)>())?;
        bytes = add(bytes, preceding)?;
        for (target, map) in &self.preceding {
            bytes = add(add(bytes, target.capacity())?, map.footprint()?)?;
        }
        Ok(bytes)
    }
    pub fn mapped_all(
        &self,
        position: Position,
    ) -> Result<Vec<super::OriginalLocation>, DebugError> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for (source, position) in self.root.lookup_all(position) {
            for (source, original) in self.trace(source, position)? {
                let location = super::OriginalLocation {
                    origin: source.origin.clone().ok_or(DebugError::InvalidMap)?,
                    line: original.line,
                    column: original.column,
                    digest: source.digest,
                };
                if seen.insert(location.clone()) {
                    result.push(location);
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/debug_source_map.rs"]
mod tests;
