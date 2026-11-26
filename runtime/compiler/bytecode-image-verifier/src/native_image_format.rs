//! The native image writer: serializes verified function graphs in QuickJS-NG's
//! bytecode format (LEB128, atoms, UTF-16 strings, function records).

use super::*;
use std::collections::BTreeMap;
const MAX: usize = 1024 * 1024 * 1024;
trait Sink {
    fn push(&mut self, byte: u8);
    fn extend_bytes(&mut self, bytes: impl IntoIterator<Item = u8>) {
        for byte in bytes {
            self.push(byte)
        }
    }
}
impl Sink for Vec<u8> {
    fn push(&mut self, byte: u8) {
        Vec::push(self, byte)
    }
}
struct Output {
    bytes: Vec<u8>,
    limit: usize,
    failed: bool,
}
impl Sink for Output {
    fn push(&mut self, byte: u8) {
        if self.bytes.len() == self.limit {
            self.failed = true;
        } else if !self.failed {
            self.bytes.push(byte)
        }
    }
}
/// Native `BC_VERSION` byte (`30 | 0x40`), matching `quickjs.c`.
///
/// lunarstorm patch: 94 (was 93) for the user-catch table in the debug section
/// and the removal of `debug_catch`; images of version 93 are refused.
pub(crate) const NATIVE_BC_VERSION: u8 = 94;
fn leb(mut n: u32, b: &mut impl Sink) {
    loop {
        let v = (n & 127) as u8;
        n >>= 7;
        b.push(v | if n != 0 { 128 } else { 0 });
        if n == 0 {
            break;
        }
    }
}
fn signed(n: i32, b: &mut impl Sink) {
    leb(((n as u32) << 1) ^ ((n >> 31) as u32), b)
}
fn text(units: &[u16], b: &mut impl Sink) {
    let wide = units.iter().any(|u| *u > 255);
    leb(units.len() as u32 * 2 + u32::from(wide), b);
    for u in units {
        if wide {
            b.extend_bytes(u.to_le_bytes())
        } else {
            b.push(*u as u8)
        }
    }
}
fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}
#[derive(Default)]
struct Atoms {
    index: BTreeMap<Vec<u16>, u32>,
    values: Vec<Vec<u16>>,
    limit: usize,
}
impl Atoms {
    fn add(&mut self, s: Vec<u16>) -> Result<(), String> {
        if !self.index.contains_key(&s) {
            if self.values.len() == self.limit {
                return Err("capacity: global atom table budget".into());
            }
            self.index.insert(s.clone(), self.values.len() as u32);
            self.values.push(s)
        }
        Ok(())
    }
    fn id(&self, s: &[u16]) -> u32 {
        crate::production::ATOM_END + self.index[s]
    }
}
fn gather(f: &FunctionImage, a: &mut Atoms, depth: usize) -> Result<(), String> {
    if depth > 256 {
        return Err("capacity: function depth budget".into());
    }
    a.add(utf16(&f.function_name))?;
    a.add(utf16(&f.filename))?;
    for v in &f.locals {
        a.add(v.name.clone())?
    }
    for c in &f.captures {
        a.add(c.name.clone())?
    }
    for atom in &f.atoms {
        a.add(atom.clone())?
    }
    for c in &f.constants {
        if let Constant::Function(n) = c {
            gather(n, a, depth + 1)?
        }
    }
    Ok(())
}
fn atom(a: &Atoms, s: &[u16], b: &mut impl Sink) {
    leb(a.id(s) * 2, b)
}
fn flags(f: &FunctionFlags) -> u16 {
    u16::from(f.has_prototype)
        | (u16::from(f.simple_parameters) << 1)
        | (u16::from(f.derived_constructor) << 2)
        | (u16::from(f.need_home_object) << 3)
        | (u16::from(f.kind) << 4)
        | (u16::from(f.new_target) << 6)
        | (u16::from(f.super_call) << 7)
        | (u16::from(f.super_allowed) << 8)
        | (u16::from(f.arguments_allowed) << 9)
        | (u16::from(f.backtrace_barrier) << 10)
        | 2048
}
fn atom_format(f: crate::OperandFormat) -> bool {
    use crate::OperandFormat::*;
    matches!(f, Atom | AtomU8 | AtomU16 | AtomLabelU8 | AtomLabelU16)
}
fn write_function(f: &FunctionImage, a: &Atoms, b: &mut Output) -> Result<(), String> {
    if b.failed {
        return Err("capacity: native image byte budget".into());
    }
    b.push(12);
    b.extend_bytes(flags(&f.flags).to_le_bytes());
    b.push(f.strict as u8);
    atom(a, &utf16(&f.function_name), b);
    for v in [
        f.arg_count,
        f.local_count,
        f.defined_arg_count,
        f.stack_size,
        f.var_ref_count,
        f.captures.len() as u32,
        f.constants.len() as u32,
        f.code.len() as u32,
        f.locals.len() as u32,
    ] {
        leb(v, b)
    }
    for v in &f.locals {
        atom(a, &v.name, b);
        leb(v.scope_level, b);
        leb(
            match v.scope_next {
                ScopeLink::End => 0,
                ScopeLink::Parameters => u32::MAX,
                ScopeLink::Local(n) => n + 1,
            },
            b,
        );
        b.push(
            v.var_kind
                | (u8::from(v.is_const) << 4)
                | (u8::from(v.is_lexical) << 5)
                | (u8::from(v.is_captured) << 6),
        );
        if v.is_captured {
            leb(v.var_ref_idx, b)
        }
    }
    for v in &f.captures {
        atom(a, &v.name, b);
        leb(v.var_idx, b);
        leb(
            u32::from(v.closure_type)
                | (u32::from(v.is_const) << 3)
                | (u32::from(v.is_lexical) << 4)
                | (u32::from(v.var_kind) << 5),
            b,
        )
    }
    for c in &f.constants {
        match c {
            Constant::Null => b.push(1),
            Constant::Undefined => b.push(2),
            Constant::Bool(v) => b.push(if *v { 4 } else { 3 }),
            // lunarstorm patch: an integer-valued number in i32 range (not -0)
            // is written as BC_TAG_INT32 (5, zigzag LEB). Fix: every number
            // was written as BC_TAG_FLOAT64, so the engine loaded `42` as a
            // float-tagged value: `1+1` was a float, `as_int()`/`i32::from_js`
            // refused literal integers, and the interpreter's integer fast
            // paths never ran. The C parser emitted integers the same way.
            Constant::Number(n)
                if n.fract() == 0.0
                    && *n >= f64::from(i32::MIN)
                    && *n <= f64::from(i32::MAX)
                    && !(*n == 0.0 && n.is_sign_negative()) =>
            {
                let i = *n as i32;
                b.push(5);
                leb(((i << 1) ^ (i >> 31)) as u32, b)
            }
            Constant::Number(n) => {
                b.push(6);
                b.extend_bytes(n.to_bits().to_le_bytes())
            }
            Constant::String(s) => {
                b.push(7);
                text(s, b)
            }
            Constant::BigInt(bytes) => {
                b.push(10);
                leb(bytes.len() as u32, b);
                b.extend_bytes(bytes.iter().copied());
            }
            Constant::Function(n) => write_function(n, a, b)?,
            Constant::TemplateObject { cooked, raw } => {
                b.push(11);
                leb(cooked.len() as u32, b);
                for c in cooked {
                    match c {
                        Some(s) => {
                            b.push(7);
                            text(s, b)
                        }
                        None => b.push(2),
                    }
                }
                b.push(11);
                leb(raw.len() as u32, b);
                for s in raw {
                    b.push(7);
                    text(s, b)
                }
                b.push(2); // the raw array has no raw property
            }
        }
    }
    let mut code = f.code.clone();
    let d = crate::decode_schema(
        &code,
        crate::Limits::default(),
        crate::production::FINAL_OPCODES,
    )
    .map_err(limits::decode_error)?;
    let updates: Vec<_> = d
        .instructions()
        .iter()
        .filter(|i| atom_format(i.opcode().format))
        .map(|i| {
            let j = u32::from_le_bytes(i.operands()[..4].try_into().unwrap());
            (i.offset() + 1, a.id(&f.atoms[j as usize]))
        })
        .collect();
    for (offset, id) in updates {
        code[offset..offset + 4].copy_from_slice(&id.to_le_bytes())
    }
    b.extend_bytes(code);
    if b.failed {
        return Err("capacity: native image byte budget".into());
    }
    // lunarstorm patch: user-catch table first in the debug section, see `FunctionImage::user_catches`.
    match &f.user_catches {
        None => leb(0, b),
        Some(table) => {
            leb(u32::try_from(table.len() + 1).map_err(|_| "user catch table size")?, b);
            let mut previous = None;
            for &target in table {
                let delta = match previous {
                    None => target,
                    Some(p) if target > p => target - p,
                    Some(_) => return Err("user catch table must be strictly ascending".into()),
                };
                leb(delta, b);
                previous = Some(target);
            }
        }
    }
    atom(a, &utf16(&f.filename), b);
    let first = f.debug_locations.first().copied().unwrap_or(DebugLocation {
        code_offset: 0,
        line: 1,
        column: 1,
    });
    leb(first.line, b);
    leb(first.column, b);
    let mut table = Vec::new();
    let (mut pc, mut line, mut col) = (0, first.line as i32, first.column as i32);
    for p in &f.debug_locations {
        table.push(0);
        leb(p.code_offset - pc, &mut table);
        signed(p.line as i32 - line, &mut table);
        signed(p.column as i32 - col, &mut table);
        pc = p.code_offset;
        line = p.line as i32;
        col = p.column as i32;
    }
    leb(table.len() as u32, b);
    b.extend_bytes(table);
    let source = f.source.as_ref().map(|s| wtf8(s)).unwrap_or_default();
    leb(source.len() as u32, b);
    b.extend_bytes(source);
    if b.failed {
        return Err("capacity: native image byte budget".into());
    }
    Ok(())
}
#[cfg(test)]
pub(super) fn encode(f: &FunctionImage) -> Result<Vec<u8>, String> {
    encode_with_limits(f, &ImageLimits::default())
}
#[cfg(test)]
pub(super) fn encode_module(m: &ModuleImage) -> Result<Vec<u8>, String> {
    encode_module_with_limits(m, &ImageLimits::default())
}
pub(super) fn encode_with_limits(f: &FunctionImage, l: &ImageLimits) -> Result<Vec<u8>, String> {
    encode_image(f, None, l)
}
pub(super) fn encode_module_with_limits(
    m: &ModuleImage,
    l: &ImageLimits,
) -> Result<Vec<u8>, String> {
    encode_image(&m.function, Some(m), l)
}
fn encode_image(
    f: &FunctionImage,
    module: Option<&ModuleImage>,
    l: &ImageLimits,
) -> Result<Vec<u8>, String> {
    if !cfg!(target_endian = "little") {
        return Err("unverified target byte order".into());
    }
    let mut a = Atoms {
        limit: l.max_table_entries,
        ..Default::default()
    };
    if let Some(m) = module {
        a.add(m.name.clone())?;
        for n in &m.required_modules {
            a.add(n.clone())?
        }
        // lunarstorm patch: attribute keys and values are atoms too.
        for (k, v) in m.required_attributes.iter().flatten() {
            a.add(k.clone())?;
            a.add(v.clone())?
        }
        for e in &m.exports {
            match e {
                ModuleExport::Local { name, .. } => a.add(name.clone())?,
                ModuleExport::Indirect { import, name, .. } => {
                    a.add(import.clone())?;
                    a.add(name.clone())?;
                }
            }
        }
        for i in &m.imports {
            a.add(i.name.clone())?
        }
    }
    gather(f, &mut a, 0)?;
    if a.values.len() > 65535 {
        return Err("capacity: atom table budget".into());
    }
    if l.max_native_bytes < 5 {
        return Err("capacity: native image byte budget".into());
    }
    let mut b = Output {
        bytes: vec![NATIVE_BC_VERSION, 0, 0, 0, 0],
        limit: l.max_native_bytes,
        failed: false,
    };
    leb(a.values.len() as u32, &mut b);
    for s in &a.values {
        b.push(1);
        text(s, &mut b);
        if b.failed {
            return Err("capacity: native image byte budget".into());
        }
    }
    if let Some(m) = module {
        b.push(13);
        atom(&a, &m.name, &mut b);
        leb(m.required_modules.len() as u32, &mut b);
        for (i, n) in m.required_modules.iter().enumerate() {
            atom(&a, n, &mut b);
            // lunarstorm patch: the request's import attributes.
            let attributes = m.required_attributes.get(i).map_or(&[][..], Vec::as_slice);
            leb(attributes.len() as u32, &mut b);
            for (k, v) in attributes {
                atom(&a, k, &mut b);
                atom(&a, v, &mut b)
            }
        }
        leb(m.exports.len() as u32, &mut b);
        for e in &m.exports {
            match e {
                ModuleExport::Local { capture, name } => {
                    b.push(0);
                    leb(*capture, &mut b);
                    atom(&a, name, &mut b)
                }
                ModuleExport::Indirect {
                    module,
                    import,
                    name,
                } => {
                    b.push(1);
                    leb(*module, &mut b);
                    atom(&a, import, &mut b);
                    atom(&a, name, &mut b)
                }
            }
        }
        leb(m.star_exports.len() as u32, &mut b);
        for i in &m.star_exports {
            leb(*i, &mut b)
        }
        leb(m.imports.len() as u32, &mut b);
        for i in &m.imports {
            leb(i.capture, &mut b);
            atom(&a, &i.name, &mut b);
            leb(i.module, &mut b)
        }
        b.push(u8::from(m.has_top_level_await));
    }
    write_function(f, &a, &mut b)?;
    let h = csum(&b.bytes[5..]);
    b.bytes[1..5].copy_from_slice(&h.to_le_bytes());
    Ok(b.bytes)
}

fn wtf8(s: &[u16]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in char::decode_utf16(s.iter().copied()) {
        match r {
            Ok(c) => {
                let mut buf = [0; 4];
                out.extend(c.encode_utf8(&mut buf).as_bytes())
            }
            Err(e) => {
                let u = u32::from(e.unpaired_surrogate());
                out.extend([
                    (0xe0 | (u >> 12)) as u8,
                    (0x80 | ((u >> 6) & 63)) as u8,
                    (0x80 | (u & 63)) as u8,
                ]);
            }
        }
    }
    out
}
fn from_wtf8(b: &[u8]) -> Result<Vec<u16>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < b.len() {
        let x = b[at];
        let (n, mut c) = match x {
            0..=127 => (1, u32::from(x)),
            0xc2..=0xdf => (2, u32::from(x & 31)),
            0xe0..=0xef => (3, u32::from(x & 15)),
            0xf0..=0xf4 => (4, u32::from(x & 7)),
            _ => return Err("invalid WTF8".into()),
        };
        let tail = b.get(at + 1..at + n).ok_or("truncated WTF8")?;
        for x in tail {
            if x & 0xc0 != 0x80 {
                return Err("invalid WTF8 continuation".into());
            }
            c = (c << 6) | u32::from(x & 63)
        }
        if c > 0x10ffff || (n == 2 && c < 128) || (n == 3 && c < 2048) || (n == 4 && c < 65536) {
            return Err("noncanonical WTF8".into());
        }
        if out.len() + if c <= 65535 { 1 } else { 2 } > 256 * 1024 * 1024 {
            return Err("capacity: native source string budget".into());
        }
        if c <= 65535 {
            out.push(c as u16)
        } else {
            let x = c - 65536;
            out.extend([0xd800 + (x >> 10) as u16, 0xdc00 + (x & 1023) as u16])
        }
        at += n;
    }
    Ok(out)
}
struct Reader<'a> {
    b: &'a [u8],
    at: usize,
    nodes: usize,
    budget: usize,
    limits: ImageLimits,
    work: limits::Work,
    state_cells: usize,
    code_bytes: usize,
}
impl<'a> Reader<'a> {
    fn charge(&mut self, n: usize) -> Result<(), String> {
        self.work.charge(n)?;
        self.budget = self
            .budget
            .checked_sub(n)
            .ok_or("capacity: decoded allocation budget")?;
        Ok(())
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        self.work.charge(n)?;
        let end = self
            .at
            .checked_add(n)
            .ok_or("capacity: image length overflow")?;
        let data = self.b.get(self.at..end).ok_or("truncated image")?;
        self.at = end;
        Ok(data)
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn leb(&mut self) -> Result<u32, String> {
        let mut n = 0;
        for shift in (0..35).step_by(7) {
            let v = self.byte()?;
            if shift == 28 && v > 15 {
                return Err("LEB overflow".into());
            }
            n |= u32::from(v & 127) << shift;
            if v & 128 == 0 {
                return Ok(n);
            }
        }
        Err("LEB overflow".into())
    }
    fn count(&mut self, max: u32) -> Result<u32, String> {
        let n = self.leb()?;
        if n > max {
            Err("capacity: native table budget".into())
        } else {
            Ok(n)
        }
    }
    fn text(&mut self) -> Result<Vec<u16>, String> {
        let n = self.leb()?;
        let len = (n >> 1) as usize;
        if len > 8 * 1024 * 1024 {
            return Err("capacity: string budget".into());
        }
        self.charge(len.saturating_mul(2))?;
        if n & 1 != 0 {
            Ok(self
                .take(len * 2)?
                .chunks_exact(2)
                .map(|x| u16::from_le_bytes([x[0], x[1]]))
                .collect())
        } else {
            Ok(self.take(len)?.iter().map(|x| u16::from(*x)).collect())
        }
    }
    fn atom(&mut self, atoms: &[Vec<u16>]) -> Result<Vec<u16>, String> {
        let n = self.leb()?;
        if n & 1 != 0 {
            return Err("integer atom unsupported".into());
        }
        let value = lookup(atoms, n >> 1)?;
        self.charge(value.len().saturating_mul(2))?;
        Ok(value.to_vec())
    }
}
fn lookup(atoms: &[Vec<u16>], id: u32) -> Result<&[u16], String> {
    if id < crate::production::ATOM_END {
        if id >= crate::production::BUILTIN_SYMBOL_START {
            return Err("symbol atoms require a typed symbol model".into());
        }
        return crate::production::BUILTIN_ATOMS
            .get(id as usize)
            .copied()
            .ok_or("builtin atom index".into());
    }
    atoms
        .get((id - crate::production::ATOM_END) as usize)
        .map(Vec::as_slice)
        .ok_or("atom index out of bounds".into())
}

fn flag(v: u16, n: u16) -> bool {
    v & (1 << n) != 0
}
fn read_function(
    r: &mut Reader<'_>,
    atoms: &[Vec<u16>],
    depth: usize,
) -> Result<FunctionImage, String> {
    r.nodes += 1;
    if depth > r.limits.max_depth || r.nodes > r.limits.max_functions {
        return Err("capacity: function graph budget".into());
    }
    if r.byte()? != 12 {
        return Err("expected function tag".into());
    }
    let bits = u16::from_le_bytes(r.take(2)?.try_into().unwrap());
    if bits & !4095 != 0 || bits & 2048 == 0 {
        return Err("unsupported function flags/debug layout".into());
    }
    let flags = FunctionFlags {
        has_prototype: flag(bits, 0),
        simple_parameters: flag(bits, 1),
        derived_constructor: flag(bits, 2),
        need_home_object: flag(bits, 3),
        kind: ((bits >> 4) & 3) as u8,
        new_target: flag(bits, 6),
        super_call: flag(bits, 7),
        super_allowed: flag(bits, 8),
        arguments_allowed: flag(bits, 9),
        backtrace_barrier: flag(bits, 10),
    };
    let strict = match r.byte()? {
        0 => false,
        1 => true,
        _ => return Err("strict flag".into()),
    };
    let function_name = String::from_utf16(&r.atom(atoms)?).map_err(|_| "function name UTF16")?;
    let arg_count = r.count(r.limits.max_table_entries as u32)?;
    let local_count = r.count(r.limits.max_table_entries as u32)?;
    let defined_arg_count = r.count(r.limits.max_table_entries as u32)?;
    let stack_size = r.count(r.limits.max_table_entries as u32)?;
    let var_ref_count = r.count(r.limits.max_table_entries as u32)?;
    let capture_count = r.count(r.limits.max_table_entries as u32)?;
    let constant_count = r.count(r.limits.max_table_entries as u32)?;
    let code_len = r.count(r.limits.max_native_bytes as u32)?;
    let local_defs = r.count((r.limits.max_table_entries * 2) as u32)?;
    let width = crate::production::FINAL_OPCODES
        .iter()
        .map(|op| op.width as usize)
        .max()
        .unwrap();
    let minimum = code_len as usize / width + usize::from(code_len as usize % width != 0);
    if minimum.saturating_mul(local_count as usize + 1) > r.limits.function_state_cells() {
        return Err("capacity: native header verifier state cell budget".into());
    }
    if code_len as usize > r.b.len() - r.at
        || constant_count as usize > r.b.len() - r.at - code_len as usize
    {
        return Err("native table cannot fit remaining image".into());
    }

    r.charge(
        std::mem::size_of::<FunctionImage>()
            + local_defs as usize * std::mem::size_of::<VarDef>()
            + capture_count as usize * std::mem::size_of::<Capture>()
            + constant_count as usize * std::mem::size_of::<Constant>(),
    )?;
    if local_defs != 0 && local_defs != arg_count + local_count {
        return Err("vardef count mismatch".into());
    }
    let mut locals = Vec::new();
    for _ in 0..local_defs {
        let name = r.atom(atoms)?;
        let scope_level = r.count(r.limits.max_table_entries as u32)?;
        let next = match r.leb()? {
            0 => ScopeLink::End,
            u32::MAX => ScopeLink::Parameters,
            n if n <= 65535 => ScopeLink::Local(n - 1),
            _ => return Err("scope link field".into()),
        };
        let bits = r.byte()?;
        if bits & 128 != 0 {
            return Err("vardef flags".into());
        }
        let is_captured = bits & 64 != 0;
        let var_ref_idx = if is_captured {
            r.count(r.limits.max_table_entries as u32)?
        } else {
            0
        };
        locals.push(VarDef {
            name,
            scope_level,
            scope_next: next,
            is_const: bits & 16 != 0,
            is_lexical: bits & 32 != 0,
            is_captured,
            var_ref_idx,
            var_kind: bits & 15,
        })
    }
    let mut captures = Vec::new();
    for _ in 0..capture_count {
        let name = r.atom(atoms)?;
        let var_idx = r.count(r.limits.max_table_entries as u32)?;
        let v = r.count(511)?;
        captures.push(Capture {
            name,
            var_idx,
            closure_type: (v & 7) as u8,
            is_const: v & 8 != 0,
            is_lexical: v & 16 != 0,
            var_kind: ((v >> 5) & 15) as u8,
        })
    }
    let mut constants = Vec::new();
    for _ in 0..constant_count {
        let t = r.byte()?;
        constants.push(match t {
            1 => Constant::Null,
            2 => Constant::Undefined,
            3 => Constant::Bool(false),
            4 => Constant::Bool(true),
            5 => {
                let v = r.leb()?;
                Constant::Number(((v >> 1) as i32 ^ -((v & 1) as i32)) as f64)
            }
            6 => Constant::Number(f64::from_bits(u64::from_le_bytes(
                r.take(8)?.try_into().unwrap(),
            ))),
            7 => Constant::String(r.text()?),
            10 => {
                let n = r.count((1024 * 1024).min(r.limits.max_native_bytes) as u32)? as usize;
                r.charge(n)?;
                Constant::BigInt(r.take(n)?.to_vec())
            }
            11 => {
                let n = r.count(65535)? as usize;
                let mut cooked = Vec::with_capacity(n);
                for _ in 0..n {
                    cooked.push(match r.byte()? {
                        7 => Some(r.text()?),
                        2 => None,
                        _ => return Err("template cooked element".into()),
                    });
                }
                if r.byte()? != 11 || r.count(65535)? as usize != n {
                    return Err("template raw array".into());
                }
                let mut raw = Vec::with_capacity(n);
                for _ in 0..n {
                    if r.byte()? != 7 {
                        return Err("template raw element".into());
                    }
                    raw.push(r.text()?);
                }
                if r.byte()? != 2 {
                    return Err("template raw array has raw".into());
                }
                Constant::TemplateObject { cooked, raw }
            }
            12 => {
                r.at -= 1;
                Constant::Function(Box::new(read_function(r, atoms, depth + 1)?))
            }
            _ => return Err("unsupported constant tag".into()),
        })
    }
    r.code_bytes = r
        .code_bytes
        .checked_add(code_len as usize)
        .ok_or("capacity: native code length overflow")?;
    if r.code_bytes > r.limits.max_native_bytes {
        return Err("capacity: aggregate native code byte budget".into());
    }
    let code_slice =
        r.b.get(r.at..r.at + code_len as usize)
            .ok_or("truncated native code")?;
    let instructions = limits::instructions(code_slice, &r.limits, &mut r.work)?;
    r.state_cells = r
        .state_cells
        .checked_add(instructions.saturating_mul(local_count as usize + 1))
        .ok_or("capacity: state cell overflow")?;
    if r.state_cells > r.limits.max_state_cells {
        return Err("capacity: aggregate verifier state cell budget".into());
    }
    r.charge(code_len as usize)?;
    let mut code = r.take(code_len as usize)?.to_vec();
    let decoded = crate::decode_schema(&code, r.limits.code(), crate::production::FINAL_OPCODES)
        .map_err(limits::decode_error)?;
    let mut code_atoms = Vec::new();
    let mut code_atom_indices = BTreeMap::new();
    let mut updates = Vec::new();
    for i in decoded.instructions() {
        if atom_format(i.opcode().format) {
            let id = u32::from_le_bytes(i.operands()[..4].try_into().unwrap());
            let atom = lookup(atoms, id)?;
            let index = if let Some(n) = code_atom_indices.get(atom) {
                *n
            } else {
                r.charge(atom.len().saturating_mul(2))?;
                let n = code_atoms.len();
                code_atoms.push(atom.to_vec());
                code_atom_indices.insert(atom, n);
                n
            };
            updates.push((i.offset() + 1, index as u32));
        }
    }
    for (offset, id) in updates {
        code[offset..offset + 4].copy_from_slice(&id.to_le_bytes())
    }
    // lunarstorm patch: user-catch table, see `FunctionImage::user_catches`.
    // Bounded by the code length; entries must be strictly ascending offsets
    // inside the code (the verifier then checks each is a `catch` target).
    let catch_count = r.count(u32::try_from(code.len()).map_err(|_| "code length")?.saturating_add(1))?;
    let user_catches = if catch_count == 0 {
        None
    } else {
        let n = (catch_count - 1) as usize;
        r.charge(n * std::mem::size_of::<u32>())?;
        let mut table = Vec::with_capacity(n);
        let mut at = 0u32;
        for i in 0..n {
            let delta = r.leb()?;
            if i > 0 && delta == 0 {
                return Err("user catch table must be strictly ascending".into());
            }
            at = at.checked_add(delta).ok_or("user catch offset overflow")?;
            if at as usize >= code.len() {
                return Err("user catch offset outside code".into());
            }
            table.push(at);
        }
        Some(table)
    };
    let filename = String::from_utf16(&r.atom(atoms)?).map_err(|_| "filename UTF16")?;
    let (mut line, mut col) = (r.count(i32::MAX as u32)?, r.count(i32::MAX as u32)?);
    if line == 0 || col == 0 {
        return Err("initial positions".into());
    }
    let plen = r.count(r.limits.max_native_bytes as u32)?;
    let position_bytes = r.take(plen as usize)?;
    let parser_work = r.work.remaining();
    let mut pr = Reader {
        b: position_bytes,
        at: 0,
        nodes: 0,
        budget: r.budget,
        limits: r.limits,
        work: limits::Work::new(&ImageLimits {
            max_work: r.work.remaining(),
            ..r.limits
        }),
        state_cells: 0,
        code_bytes: 0,
    };
    let initial = DebugLocation {
        code_offset: 0,
        line,
        column: col,
    };
    let mut debug_locations: Vec<DebugLocation> = Vec::new();
    let mut pc = 0u32;
    let mut records = 0usize;
    while pr.at < pr.b.len() {
        if records == r.limits.max_positions {
            return Err("capacity: native location count budget".into());
        }
        records += 1;
        let op = pr.byte()?;
        if op == 0 {
            pc = pc.checked_add(pr.leb()?).ok_or("pc overflow")?;
            line = delta(line, pr.leb()?)?;
        } else {
            pc = pc
                .checked_add(u32::from((op - 1) / 5))
                .ok_or("pc overflow")?;
            line = u32::try_from(i64::from(line) + i64::from((op - 1) % 5) - 1)
                .map_err(|_| "line overflow")?;
        }
        col = delta(col, pr.leb()?)?;
        if line == 0 || col == 0 || line > i32::MAX as u32 || col > i32::MAX as u32 {
            return Err("native location coordinate bounds".into());
        }
        if debug_locations.len() >= r.limits.max_positions {
            return Err("capacity: native location count budget".into());
        }
        r.charge(std::mem::size_of::<DebugLocation>())?;
        let next = DebugLocation {
            code_offset: pc,
            line,
            column: col,
        };
        if debug_locations.is_empty() && pc > 0 {
            if r.limits.max_positions < 2 {
                return Err("capacity: canonical native location count budget".into());
            }
            r.charge(std::mem::size_of::<DebugLocation>())?;
            debug_locations.push(initial);
        }
        if debug_locations.last().is_some_and(|p| p.code_offset == pc) {
            *debug_locations.last_mut().unwrap() = next;
        } else {
            debug_locations.push(next);
        }
        if debug_locations.len() > r.limits.max_positions {
            return Err("position count".into());
        }
    }
    let parsed_work = parser_work - pr.work.remaining();
    r.work.charge(parsed_work)?;
    let slen = r.count(r.limits.max_native_bytes as u32)?;
    r.charge(slen as usize * 2)?;
    let source = if slen > 0 {
        Some(from_wtf8(r.take(slen as usize)?)?)
    } else {
        None
    };
    if debug_locations.is_empty() && (initial.line != 1 || initial.column != 1) {
        if r.limits.max_positions == 0 {
            return Err("capacity: native location count budget".into());
        }
        r.charge(std::mem::size_of::<DebugLocation>())?;
        debug_locations.push(initial);
    }
    Ok(FunctionImage {
        code,
        constants,
        local_count,
        arg_count,
        defined_arg_count,
        stack_size,
        strict,
        function_name,
        filename,
        debug_locations,
        flags,
        locals,
        captures,
        var_ref_count,
        atoms: code_atoms,
        source,
        user_catches,
    })
}
fn delta(base: u32, v: u32) -> Result<u32, String> {
    let d = ((v >> 1) as i32) ^ (-((v & 1) as i32));
    u32::try_from(i64::from(base) + i64::from(d)).map_err(|_| "position delta".into())
}
pub(super) fn decode_with_limits(bytes: &[u8], l: &ImageLimits) -> Result<(Image, usize), String> {
    l.validate()?;
    if l.max_functions == 0 {
        return Err("capacity: function graph budget".into());
    }
    if bytes.len() > l.max_native_bytes {
        return Err("capacity: native image byte budget".into());
    }
    let mut work = limits::Work::new(l);
    work.charge(bytes.len().saturating_mul(2))?;

    if !cfg!(target_endian = "little") || bytes.len() > MAX || bytes.len() < 6 {
        return Err("capacity: image target/length budget".into());
    }
    if bytes[0] != NATIVE_BC_VERSION {
        return Err("target version mismatch".into());
    }
    if u32::from_le_bytes(bytes[1..5].try_into().unwrap()) != csum(&bytes[5..]) {
        return Err("checksum mismatch".into());
    }
    let mut r = Reader {
        b: bytes,
        at: 5,
        nodes: 0,
        budget: l.max_graph_bytes,
        limits: *l,
        work,
        state_cells: 0,
        code_bytes: 0,
    };
    let n = r.count(r.limits.max_table_entries as u32)?;
    r.charge(n as usize * std::mem::size_of::<Vec<u16>>())?;
    let mut atoms = Vec::new();
    for _ in 0..n {
        if r.byte()? != 1 {
            return Err("atom type unsupported".into());
        }
        atoms.push(r.text()?)
    }
    let image = if r.b.get(r.at) == Some(&13) {
        r.at += 1;
        let name = r.atom(&atoms)?;
        let count = r.count(r.limits.max_table_entries as u32)?;
        r.charge(count as usize * std::mem::size_of::<Vec<u16>>())?;
        let mut required_modules = Vec::new();
        let mut required_attributes = Vec::new();
        for _ in 0..count {
            required_modules.push(r.atom(&atoms)?);
            // lunarstorm patch: the request's import attributes.
            let pairs = r.count(r.limits.max_table_entries as u32)?;
            r.charge(pairs as usize * 2 * std::mem::size_of::<Vec<u16>>())?;
            let mut attributes = Vec::new();
            for _ in 0..pairs {
                attributes.push((r.atom(&atoms)?, r.atom(&atoms)?));
            }
            required_attributes.push(attributes);
        }
        // No request with attributes is the canonical empty table.
        if required_attributes.iter().all(Vec::is_empty) {
            required_attributes.clear();
        }
        let count = r.count(r.limits.max_table_entries as u32)?;
        r.charge(count as usize * std::mem::size_of::<ModuleExport>())?;
        let mut exports = Vec::new();
        for _ in 0..count {
            exports.push(match r.byte()? {
                0 => ModuleExport::Local {
                    capture: r.count(r.limits.max_table_entries as u32)?,
                    name: r.atom(&atoms)?,
                },
                1 => ModuleExport::Indirect {
                    module: r.count(r.limits.max_table_entries as u32)?,
                    import: r.atom(&atoms)?,
                    name: r.atom(&atoms)?,
                },
                _ => return Err("module export type".into()),
            })
        }
        let count = r.count(r.limits.max_table_entries as u32)?;
        r.charge(count as usize * std::mem::size_of::<u32>())?;
        let mut star_exports = Vec::new();
        for _ in 0..count {
            star_exports.push(r.count(r.limits.max_table_entries as u32)?)
        }
        let count = r.count(r.limits.max_table_entries as u32)?;
        r.charge(count as usize * std::mem::size_of::<ModuleImport>())?;
        let mut imports = Vec::new();
        for _ in 0..count {
            imports.push(ModuleImport {
                capture: r.count(r.limits.max_table_entries as u32)?,
                name: r.atom(&atoms)?,
                module: r.count(r.limits.max_table_entries as u32)?,
            })
        }
        let has_top_level_await = match r.byte()? {
            0 => false,
            1 => true,
            _ => return Err("module TLA flag".into()),
        };
        let function = read_function(&mut r, &atoms, 0)?;
        Image::Module(ModuleImage {
            name,
            required_modules,
            required_attributes,
            exports,
            star_exports,
            imports,
            has_top_level_await,
            function,
        })
    } else {
        Image::Function(read_function(&mut r, &atoms, 0)?)
    };
    if r.at != bytes.len() {
        return Err("trailing object data".into());
    }
    Ok((image, l.max_work - r.work.remaining()))
}

#[cfg(test)]
mod representation_budget_tests {
    use super::*;
    fn raw(mut payload: Vec<u8>) -> Vec<u8> {
        let mut b = vec![NATIVE_BC_VERSION];
        b.extend(csum(&payload).to_le_bytes());
        b.append(&mut payload);
        b
    }
    fn capacity(result: Result<VerifiedImage, String>, part: &str) {
        let error = result.unwrap_err();
        assert!(error.starts_with("capacity:"), "{error}");
        assert!(error.contains(part), "{error}");
    }
    #[test]
    fn oversized_atom_and_string_headers_fail_before_tables_or_strings_are_allocated() {
        let mut payload = Vec::new();
        leb(u32::MAX, &mut payload);
        capacity(decode_and_verify(&raw(payload)), "table");
        let mut payload = vec![1, 1];
        leb(u32::MAX, &mut payload);
        capacity(decode_and_verify(&raw(payload)), "string");
    }
    #[test]
    fn native_header_state_lower_bound_fails_before_declared_code_or_vardefs_are_allocated() {
        let mut payload = vec![1, 1, 0, 12, 2, 8, 0];
        leb(crate::production::ATOM_END * 2, &mut payload);
        for value in [0, 65535, 0, 0, 0, 0, 0, 16 * 1024 * 1024, 0] {
            leb(value, &mut payload)
        }
        capacity(decode_and_verify(&raw(payload)), "state cell");
    }
    #[test]
    fn representation_local_count_is_checked_before_vardef_allocation() {
        let f = FunctionImage {
            code: vec![crate::production::ids::return_undef],
            local_count: u32::MAX,
            ..Default::default()
        };
        let bytes = encode(&f).unwrap();
        capacity(decode_and_verify(&bytes), "table");
    }
}
