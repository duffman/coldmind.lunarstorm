//! Protected Rust metadata envelope. Only its native payload enters the C loader.
use super::*;
use crate::snapshot::Position;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
const MAGIC: &[u8; 8] = b"QJSRIMG\0";
const VERSION: u32 = 3;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceUnit {
    Utf8,
    Utf16,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionSpans {
    /// Constant-index path from the root function. Empty identifies the root.
    pub path: Vec<u32>,
    pub function_start: u32,
    pub function_end: u32,
    pub positions: Vec<Position>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanMetadata {
    pub source_unit: SourceUnit,
    pub source_len: u32,
    /// Source identity asserted by the producer: UTF8 bytes or little-endian UTF16 units.
    /// Source-free verification checks its protection, not unavailable original text.
    pub source_digest: [u8; 32],
    pub functions: Vec<FunctionSpans>,
}
impl SpanMetadata {
    pub fn function(&self, path: &[u32]) -> Option<&FunctionSpans> {
        self.functions.iter().find(|f| f.path == path)
    }
}
pub fn source_digest_utf16(units: &[u16]) -> [u8; 32] {
    let mut h = Sha256::new();
    let mut buffer = [0u8; 1024];
    for chunk in units.chunks(512) {
        for (at, u) in chunk.iter().enumerate() {
            buffer[at * 2..at * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        h.update(&buffer[..chunk.len() * 2]);
    }
    h.finalize().into()
}
pub(super) fn graph_limits(
    spans: Option<&SpanMetadata>,
    l: &ImageLimits,
) -> Result<ImageLimits, String> {
    l.validate()?;
    let mut total = 0usize;
    if let Some(meta) = spans {
        if meta.functions.len() > l.max_functions {
            return Err("capacity: span function budget".into());
        }
        total = meta.functions.len() * std::mem::size_of::<FunctionSpans>();
        for f in &meta.functions {
            if f.path.len() > l.max_depth || f.positions.len() > l.max_positions {
                return Err("capacity: span table budget".into());
            }
            total = total
                .checked_add(f.path.len() * 4 + f.positions.len() * std::mem::size_of::<Position>())
                .ok_or("capacity: span allocation overflow")?;
        }
        if total > 16 * 1024 * 1024 {
            return Err("capacity: span allocation budget".into());
        }
    }
    Ok(ImageLimits {
        max_graph_bytes: l
            .max_graph_bytes
            .checked_sub(total)
            .ok_or("capacity: combined span/native graph allocation budget")?,
        ..*l
    })
}
pub(super) fn verify_spans_with_limits(
    root: &FunctionImage,
    meta: &SpanMetadata,
    l: &ImageLimits,
) -> Result<usize, String> {
    l.validate()?;
    let mut work = limits::Work::new(l);
    if meta.functions.len() > l.max_functions {
        return Err("capacity: span function budget".into());
    }
    let mut paths = BTreeSet::new();
    let mut total = meta.functions.len() * std::mem::size_of::<FunctionSpans>();
    for spans in &meta.functions {
        work.charge(1 + spans.path.len() + spans.positions.len())?;
        if spans.path.len() > l.max_depth
            || !paths.insert(&spans.path)
            || spans.function_start > spans.function_end
            || spans.function_end > meta.source_len
        {
            return Err("span path/range metadata".into());
        }
        total = total
            .checked_add(
                spans.path.len() * 4 + spans.positions.len() * std::mem::size_of::<Position>(),
            )
            .ok_or("capacity: span allocation overflow")?;
        if total > (16 * 1024 * 1024).min(l.max_graph_bytes)
            || spans.positions.len() > l.max_positions
        {
            return Err("capacity: span allocation budget".into());
        }
        let mut function = root;
        for index in &spans.path {
            function = match function.constants.get(*index as usize) {
                Some(Constant::Function(f)) => f,
                _ => return Err("span path does not identify a function".into()),
            }
        }
        let coordinate_bound = u64::from(meta.source_len) + 1;
        if function
            .debug_locations
            .iter()
            .any(|p| u64::from(p.line) > coordinate_bound || u64::from(p.column) > coordinate_bound)
        {
            return Err("native location exceeds claimed whole-source coordinate bound".into());
        }
        work.charge(function.debug_locations.len())?;
        limits::instructions(&function.code, l, &mut work)?;
        let mut boundary = 0usize;
        let mut last = None;
        for p in &spans.positions {
            if p.source_start > p.source_end
                || p.source_start < spans.function_start
                || p.source_end > spans.function_end
                || p.code_offset as usize >= function.code.len()
                || last.is_some_and(|n| n >= p.code_offset)
            {
                return Err("source span bounds/instruction boundary".into());
            }
            while boundary < p.code_offset as usize {
                work.charge(1)?;
                let id = function.code[boundary];
                let op = crate::production::FINAL_OPCODES
                    .get(id as usize - 1)
                    .ok_or("invalid span opcode")?;
                boundary += op.width as usize;
            }
            if boundary != p.code_offset as usize {
                return Err("source span instruction boundary".into());
            }
            last = Some(p.code_offset);
        }
    }
    Ok(l.max_work - work.remaining())
}
fn num(v: u32, b: &mut Vec<u8>) {
    b.extend(v.to_le_bytes())
}
fn blob(v: &[u8], b: &mut Vec<u8>) {
    num(v.len() as u32, b);
    b.extend(v)
}
const EVAL_MAGIC: &[u8; 8] = b"QJSREVL\0";
fn write_spans(spans: Option<&SpanMetadata>, b: &mut Vec<u8>) {
    match spans {
        None => b.push(0),
        Some(m) => {
            b.push(1);
            b.push(match m.source_unit {
                SourceUnit::Utf8 => 0,
                SourceUnit::Utf16 => 1,
            });
            num(m.source_len, b);
            b.extend(m.source_digest);
            num(m.functions.len() as u32, b);
            for f in &m.functions {
                num(f.path.len() as u32, b);
                for i in &f.path {
                    num(*i, b)
                }
                num(f.function_start, b);
                num(f.function_end, b);
                num(f.positions.len() as u32, b);
                for p in &f.positions {
                    num(p.code_offset, b);
                    num(p.source_start, b);
                    num(p.source_end, b)
                }
            }
        }
    }
}
fn serialized_size(
    native: &[u8],
    spans: Option<&SpanMetadata>,
    profile: &str,
    env: bool,
) -> Result<usize, String> {
    let mut size = 8usize
        + 4
        + 4
        + crate::production::TARGET_FINGERPRINT.len()
        + 4
        + profile.len()
        + 4
        + native.len()
        + 1
        + 32
        + if env { 32 } else { 0 };
    if let Some(m) = spans {
        size = size
            .checked_add(1 + 4 + 32 + 4)
            .ok_or("capacity: artifact size overflow")?;
        for f in &m.functions {
            size = size
                .checked_add(16 + f.path.len() * 4 + f.positions.len() * 12)
                .ok_or("capacity: artifact size overflow")?;
        }
    }
    Ok(size)
}
fn serialize_parts(
    f: &FunctionImage,
    native: &[u8],
    spans: Option<&SpanMetadata>,
    profile: &str,
    env: Option<&[u8; 32]>,
    l: &ImageLimits,
    module: Option<&ModuleImage>,
) -> Result<Vec<u8>, String> {
    l.validate()?;
    let graph_limits = graph_limits(spans, l)?;
    let mut work = limits::preflight(f, module, &graph_limits)?;
    if profile == NATIVE_PROFILE && spans.is_some() {
        return Err("native-only payload cannot claim Rust span provenance".into());
    }
    if let Some(m) = spans {
        let remaining = ImageLimits {
            max_work: work.remaining(),
            ..*l
        };
        let used = verify_spans_with_limits(f, m, &remaining)?;
        work.charge(used)?;
    }
    let size = serialized_size(native, spans, profile, env.is_some())?;
    if size > l.max_artifact_bytes {
        return Err("capacity: artifact byte budget".into());
    }
    work.charge(size.saturating_mul(2))?;
    let mut b = Vec::new();
    b.try_reserve_exact(size)
        .map_err(|_| "capacity: artifact allocation failed")?;
    b.extend(if env.is_some() { EVAL_MAGIC } else { MAGIC });
    num(VERSION, &mut b);
    if let Some(d) = env {
        b.extend(d)
    }
    blob(crate::production::TARGET_FINGERPRINT.as_bytes(), &mut b);
    blob(profile.as_bytes(), &mut b);
    blob(native, &mut b);
    write_spans(spans, &mut b);
    let digest = Sha256::digest(&b);
    b.extend(digest);
    Ok(b)
}
pub(super) fn serialize(image: &VerifiedImage) -> Result<Vec<u8>, String> {
    serialize_with_limits(image, &ImageLimits::default())
}
pub(super) fn serialize_with_limits(
    image: &VerifiedImage,
    l: &ImageLimits,
) -> Result<Vec<u8>, String> {
    serialize_parts(
        image.function(),
        image.bytes(),
        image.spans(),
        image.profile(),
        None,
        l,
        image.module(),
    )
}
pub(super) fn serialize_eval(
    image: &super::eval::VerifiedEvalImage,
    l: &ImageLimits,
) -> Result<Vec<u8>, String> {
    serialize_parts(
        image.function(),
        image.bytes(),
        image.spans(),
        image.profile(),
        Some(image.layout_digest()),
        l,
        None,
    )
}
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    budget: usize,
    work: limits::Work,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        self.work.charge(n)?;
        let end = self
            .at
            .checked_add(n)
            .ok_or("capacity: artifact size overflow")?;
        let v = self
            .bytes
            .get(self.at..end)
            .ok_or("truncated Rust artifact")?;
        self.at = end;
        Ok(v)
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn num(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn count(&mut self, max: usize) -> Result<u32, String> {
        let v = self.num()?;
        if v as usize > max {
            Err("capacity: artifact count budget".into())
        } else {
            Ok(v)
        }
    }
    fn blob(&mut self, max: usize) -> Result<&'a [u8], String> {
        let n = self.count(max)?;
        self.take(n as usize)
    }
    fn charge(&mut self, n: usize) -> Result<(), String> {
        self.budget = self
            .budget
            .checked_sub(n)
            .ok_or("capacity: artifact allocation budget")?;
        Ok(())
    }
}
struct Parts<'a> {
    native: &'a [u8],
    spans: Option<SpanMetadata>,
    profile: &'static str,
    remaining: ImageLimits,
}
fn parse<'a>(
    bytes: &'a [u8],
    env: Option<&[u8; 32]>,
    l: &ImageLimits,
) -> Result<Parts<'a>, String> {
    l.validate()?;
    if bytes.len() > l.max_artifact_bytes {
        return Err("capacity: Rust artifact byte budget".into());
    }
    if bytes.len() < 8 + 4 + 32 {
        return Err("truncated Rust artifact header".into());
    }
    let mut work = limits::Work::new(l);
    work.charge(bytes.len())?;
    let split = bytes.len() - 32;
    if Sha256::digest(&bytes[..split])[..] != bytes[split..] {
        return Err("Rust artifact integrity mismatch".into());
    }
    let mut r = Reader {
        bytes: &bytes[..split],
        at: 0,
        budget: (16 * 1024 * 1024).min(l.max_graph_bytes),
        work,
    };
    if r.take(8)? != if env.is_some() { EVAL_MAGIC } else { MAGIC } || r.num()? != VERSION {
        return Err("Rust artifact magic/version mismatch".into());
    }
    if let Some(expected) = env {
        if r.take(32)? != expected {
            return Err("eval artifact environment digest mismatch".into());
        }
    }
    if r.blob(1024)? != crate::production::TARGET_FINGERPRINT.as_bytes() {
        return Err("Rust artifact target mismatch".into());
    }
    let profile = match r.blob(128)? {
        p if p == COMPILER_PROFILE.as_bytes() => COMPILER_PROFILE,
        p if p == NATIVE_PROFILE.as_bytes() => NATIVE_PROFILE,
        _ => return Err("Rust artifact compiler profile mismatch".into()),
    };
    let native = r.blob(l.max_native_bytes)?;
    let spans = match r.byte()? {
        0 => None,
        1 => {
            if profile != COMPILER_PROFILE {
                return Err("unknown native profile cannot assert source spans".into());
            }
            let source_unit = match r.byte()? {
                0 => SourceUnit::Utf8,
                1 => SourceUnit::Utf16,
                _ => return Err("span source unit".into()),
            };
            let source_len = r.num()?;
            let source_digest = r.take(32)?.try_into().unwrap();
            let count = r.count(l.max_functions)?;
            r.charge(count as usize * std::mem::size_of::<FunctionSpans>())?;
            let mut functions = Vec::new();
            for _ in 0..count {
                let count = r.count(l.max_depth)?;
                r.charge(count as usize * 4)?;
                let mut path = Vec::new();
                for _ in 0..count {
                    path.push(r.num()?)
                }
                let function_start = r.num()?;
                let function_end = r.num()?;
                let count = r.count(l.max_positions)?;
                r.charge(count as usize * std::mem::size_of::<Position>())?;
                let mut positions = Vec::new();
                for _ in 0..count {
                    positions.push(Position {
                        code_offset: r.num()?,
                        source_start: r.num()?,
                        source_end: r.num()?,
                    })
                }
                functions.push(FunctionSpans {
                    path,
                    function_start,
                    function_end,
                    positions,
                });
            }
            Some(SpanMetadata {
                source_unit,
                source_len,
                source_digest,
                functions,
            })
        }
        _ => return Err("span presence flag".into()),
    };
    if r.at != r.bytes.len() {
        return Err("trailing Rust artifact data".into());
    }
    let consumed = (16 * 1024 * 1024).min(l.max_graph_bytes) - r.budget;
    let remaining = ImageLimits {
        max_work: r.work.remaining(),
        max_graph_bytes: l.max_graph_bytes - consumed,
        ..*l
    };
    Ok(Parts {
        native,
        spans,
        profile,
        remaining,
    })
}
pub fn deserialize_artifact(bytes: &[u8]) -> Result<VerifiedImage, ImageError> {
    deserialize_artifact_with_limits(bytes, &ImageLimits::default())
}
pub fn deserialize_artifact_with_limits(
    bytes: &[u8],
    l: &ImageLimits,
) -> Result<VerifiedImage, ImageError> {
    let parts = parse(bytes, None, l)?;
    let (mut image, used) = super::decode_limited(parts.native, &parts.remaining)?;
    let remaining = ImageLimits {
        max_work: parts.remaining.max_work - used,
        ..*l
    };
    if let Some(m) = &parts.spans {
        verify_spans_with_limits(image.function(), m, &remaining)?;
    }
    image.spans = parts.spans;
    image.profile = parts.profile;
    Ok(image)
}
pub(super) fn deserialize_eval(
    bytes: &[u8],
    layout: &super::eval::VerifiedEnvLayout,
    l: &ImageLimits,
) -> Result<super::eval::VerifiedEvalImage, ImageError> {
    let parts = parse(bytes, Some(layout.digest()), l)?;
    super::eval::from_artifact(
        parts.native,
        layout,
        parts.spans,
        parts.profile,
        &parts.remaining,
        l,
    )
}
