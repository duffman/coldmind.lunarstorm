// Copyright (c) 2017-2018 Fabrice Bellard.
// Copyright (c) 2026 Jetbunny contributors.
// SPDX-License-Identifier: MIT
// Native register-allocation algorithm ported from the pinned libregexp.c.
// The complete upstream MIT notice is retained in vendor/quickjs-ng/LICENSE.
//! Rust-owned native regexp compiler primitives for the pinned regex VM.
//! The native matcher remains C; these routines never invoke lre_compile.
//! Instruction identities come directly from the pinned header, not copied IDs.
use std::sync::OnceLock;
const HEADER: usize = 8;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_INSTRUCTIONS: usize = 1_000_000;
#[derive(Clone, Copy)]
struct Opcode {
    name: &'static str,
    size: usize,
}
fn schema() -> &'static [Opcode] {
    static SCHEMA: OnceLock<Vec<Opcode>> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        include_str!("../../../vendor/quickjs-ng/libregexp-opcode.h")
            .lines()
            .filter_map(|line| {
                let definition = line.trim().strip_prefix("DEF(")?.split(')').next()?;
                let (name, size) = definition.split_once(',')?;
                Some(Opcode {
                    name: name.trim(),
                    size: size.trim().parse().expect("pinned regexp opcode size"),
                })
            })
            .collect()
    })
}
fn opcode(name: &str) -> Result<u8, String> {
    schema()
        .iter()
        .position(|op| op.name == name)
        .and_then(|n| u8::try_from(n).ok())
        .ok_or_else(|| format!("unknown pinned regexp opcode {name}"))
}
fn width(body: &[u8], pc: usize) -> Result<(Opcode, usize), String> {
    let op = *schema()
        .get(*body.get(pc).ok_or("truncated regexp opcode")? as usize)
        .ok_or("invalid regexp opcode")?;
    if op.name == "invalid" {
        return Err("invalid regexp opcode".into());
    }
    let mut size = op.size;
    let extra = match op.name {
        "range" | "range_i" | "range32" | "range32_i" => {
            let bytes: [u8; 2] = body
                .get(pc + 1..pc + 3)
                .ok_or("truncated regexp range")?
                .try_into()
                .unwrap();
            usize::from(u16::from_ne_bytes(bytes))
                * if op.name.starts_with("range32") { 8 } else { 4 }
        }
        "back_reference"
        | "back_reference_i"
        | "backward_back_reference"
        | "backward_back_reference_i" => {
            usize::from(*body.get(pc + 1).ok_or("truncated regexp capture list")?)
        }
        _ => 0,
    };
    size = size
        .checked_add(extra)
        .ok_or("capacity: regexp instruction width")?;
    if pc.checked_add(size).is_none_or(|end| end > body.len()) {
        return Err("truncated regexp operand".into());
    }
    Ok((op, size))
}
/// Port of the native compiler's linear register-allocation pass. This accepts
/// compiler intermediates, not an executable-bytecode safety token.
pub fn allocate_registers(body: &mut [u8]) -> Result<u8, String> {
    if body.len() > MAX_BYTES {
        return Err("capacity: regexp code bytes".into());
    }
    let (mut pc, mut depth, mut peak, mut instructions) = (0, 0usize, 0usize, 0usize);
    while pc < body.len() {
        instructions += 1;
        if instructions > MAX_INSTRUCTIONS {
            return Err("capacity: regexp instruction count".into());
        }
        let (op, size) = width(body, pc)?;
        match op.name {
            "set_i32" | "set_char_pos" => {
                if depth >= 255 {
                    return Err("capacity: regexp registers".into());
                }
                body[pc + 1] = depth as u8;
                depth += 1;
                peak = peak.max(depth);
            }
            "check_advance" | "loop" | "loop_split_goto_first" | "loop_split_next_first" => {
                depth = depth
                    .checked_sub(1)
                    .ok_or("regexp register stack underflow")?;
                body[pc + 1] = depth as u8;
            }
            "loop_check_adv_split_goto_first" | "loop_check_adv_split_next_first" => {
                depth = depth
                    .checked_sub(2)
                    .ok_or("regexp register stack underflow")?;
                body[pc + 1] = depth as u8;
            }
            _ => {}
        }
        pc += size;
    }
    if depth != 0 {
        return Err("unbalanced regexp compiler register scopes".into());
    }
    Ok(peak as u8)
}
/// Validate all static destinations before a compiler program can be published.
/// This structural pass is not sufficient to authorize arbitrary regex bytecode.
pub fn validate_destinations(body: &[u8]) -> Result<(), String> {
    if body.len() > MAX_BYTES {
        return Err("capacity: regexp code bytes".into());
    }
    let mut boundaries = std::collections::BTreeSet::new();
    let mut branches = Vec::new();
    let mut pc = 0;
    while pc < body.len() {
        if boundaries.len() >= MAX_INSTRUCTIONS {
            return Err("capacity: regexp instruction count".into());
        }
        boundaries.insert(pc);
        let (op, size) = width(body, pc)?;
        let operand = match op.name {
            "goto" | "split_goto_first" | "split_next_first" | "lookahead"
            | "negative_lookahead" => Some(pc + 1),
            "loop" => Some(pc + 2),
            "loop_split_goto_first"
            | "loop_split_next_first"
            | "loop_check_adv_split_goto_first"
            | "loop_check_adv_split_next_first" => Some(pc + 6),
            _ => None,
        };
        if let Some(at) = operand {
            let offset = i32::from_ne_bytes(body[at..at + 4].try_into().unwrap());
            let destination = (pc + size) as i64 + i64::from(offset);
            branches
                .push(usize::try_from(destination).map_err(|_| "negative regexp branch target")?);
        }
        pc += size;
    }
    if branches.iter().any(|target| !boundaries.contains(target)) {
        return Err("regexp branch is not an instruction boundary".into());
    }
    Ok(())
}
/// Native regex bytes produced by a closed, typed construction path. Arbitrary
/// input buffers cannot acquire this token through the structural helpers.
pub struct LiteralProgram {
    bytes: Vec<u8>,
}
impl LiteralProgram {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn flags(&self) -> u16 {
        u16::from_ne_bytes(self.bytes[..2].try_into().unwrap())
    }
}
/// Compile a regex AST's literal UTF16 sequence (not a regex source parser).
/// Ignore-case compilation needs canonical folding and is deliberately not
/// accepted by this constructor; callers cannot silently lose that behavior.
pub fn compile_literal_sequence(units: &[u16], flags: u16) -> Result<LiteralProgram, String> {
    if units.len() > (MAX_BYTES - 32) / 5 {
        return Err("capacity: regexp literal sequence".into());
    }
    compile_pattern(&Pattern::Literal(units.to_vec()), flags)
}

/// Owned lowering tree for regexp code generation. Parsing/early errors belong
/// to the OXC frontend; the bytecode writer never reparses or regenerates source.
#[derive(Clone, Debug)]
pub enum Pattern {
    Empty,
    Never,
    Literal(Vec<u16>),
    Sequence(Vec<Pattern>),
    Alternatives(Vec<Pattern>),
    Ranges {
        ranges: Vec<(u32, u32)>,
        negate: bool,
    },
    Dot,
    Space {
        negate: bool,
    },
    LineStart,
    LineEnd,
    WordBoundary {
        negate: bool,
    },
    Capture {
        index: u8,
        body: Box<Pattern>,
    },
    BackReference(Vec<u8>),
    LookAhead {
        positive: bool,
        body: Box<Pattern>,
    },
    Repeat {
        body: Box<Pattern>,
        minimum: u32,
        maximum: Option<u32>,
        greedy: bool,
    },
}
struct Compiler {
    code: Vec<u8>,
    flags: u16,
    work: usize,
    count: usize,
}
impl Compiler {
    fn charge(&mut self, amount: usize) -> Result<(), String> {
        self.work = self
            .work
            .checked_add(amount)
            .ok_or("capacity: regexp compiler work")?;
        if self.work > 64_000_000 {
            return Err("capacity: regexp compiler work".into());
        }
        Ok(())
    }
    fn emit(&mut self, name: &str, operands: &[u8]) -> Result<usize, String> {
        self.charge(1 + operands.len())?;
        if self.count >= MAX_INSTRUCTIONS
            || self.code.len().saturating_add(1 + operands.len()) > MAX_BYTES - HEADER
        {
            return Err("capacity: regexp code".into());
        }
        let at = self.code.len();
        self.code.push(opcode(name)?);
        self.code.extend(operands);
        self.count += 1;
        Ok(at)
    }
    fn branch(&mut self, name: &str) -> Result<usize, String> {
        self.emit(name, &0i32.to_ne_bytes())
    }
    fn patch(&mut self, at: usize, target: usize) -> Result<(), String> {
        let (_, size) = width(&self.code, at)?;
        let delta = i32::try_from(target as i64 - (at + size) as i64)
            .map_err(|_| "capacity: regexp branch displacement")?;
        self.code[at + size - 4..at + size].copy_from_slice(&delta.to_ne_bytes());
        Ok(())
    }
    fn capture_range(&mut self, root: &Pattern) -> Result<Option<(u8, u8)>, String> {
        let mut pending = vec![root];
        let (mut low, mut high) = (u8::MAX, 0);
        while let Some(node) = pending.pop() {
            self.charge(1)?;
            match node {
                Pattern::Capture { index, body } => {
                    low = low.min(*index);
                    high = high.max(*index);
                    pending.push(body);
                }
                Pattern::Sequence(nodes) | Pattern::Alternatives(nodes) => {
                    self.charge(nodes.len())?;
                    pending.extend(nodes);
                }
                Pattern::Repeat { body, .. } | Pattern::LookAhead { body, .. } => {
                    pending.push(body)
                }
                _ => {}
            }
        }
        Ok((high != 0).then_some((low, high)))
    }
    fn nullable(&mut self, node: &Pattern, depth: usize) -> Result<bool, String> {
        self.charge(1)?;
        if depth > 256 {
            return Err("capacity: regexp nesting".into());
        }
        Ok(match node {
            Pattern::Empty
            | Pattern::LineStart
            | Pattern::LineEnd
            | Pattern::WordBoundary { .. }
            | Pattern::LookAhead { .. }
            | Pattern::BackReference(_) => true,
            Pattern::Never | Pattern::Dot | Pattern::Space { .. } | Pattern::Ranges { .. } => false,
            Pattern::Literal(units) => units.is_empty(),
            Pattern::Capture { body, .. } => self.nullable(body, depth + 1)?,
            Pattern::Repeat { body, minimum, .. } => {
                *minimum == 0 || self.nullable(body, depth + 1)?
            }
            Pattern::Sequence(nodes) => {
                let mut result = true;
                for node in nodes {
                    result &= self.nullable(node, depth + 1)?;
                }
                result
            }
            Pattern::Alternatives(nodes) => {
                let mut result = false;
                for node in nodes {
                    result |= self.nullable(node, depth + 1)?;
                }
                result
            }
        })
    }
    fn repeated_body(&mut self, body: &Pattern, depth: usize) -> Result<(), String> {
        if let Some((first, last)) = self.capture_range(body)? {
            self.emit("save_reset", &[first, last])?;
        }
        self.node(body, depth)
    }
    fn literal(&mut self, units: &[u16]) -> Result<(), String> {
        self.charge(units.len())?;
        let unicode = self.flags & ((1 << 4) | (1 << 8)) != 0;
        let mut at = 0;
        while at < units.len() {
            let first = units[at];
            at += 1;
            if let Some(&second) = units.get(at).filter(|&&second| {
                unicode && (0xd800..=0xdbff).contains(&first) && (0xdc00..=0xdfff).contains(&second)
            }) {
                at += 1;
                let value =
                    0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second) - 0xdc00;
                self.emit("char32", &value.to_ne_bytes())?;
            } else {
                self.emit("char", &first.to_ne_bytes())?;
            }
        }
        Ok(())
    }
    fn never(&mut self) -> Result<(), String> {
        let start = self.branch("negative_lookahead")?;
        self.emit("negative_lookahead_match", &[])?;
        self.patch(start, self.code.len())
    }
    fn ranges(&mut self, ranges: &[(u32, u32)], negate: bool) -> Result<(), String> {
        self.charge(ranges.len())?;
        let ceiling = if self.flags & ((1 << 4) | (1 << 8)) != 0 {
            0x10ffff
        } else {
            0xffff
        };
        if ranges.len() > 65535 {
            return Err("capacity: regexp ranges".into());
        }
        if ranges.iter().any(|(a, b)| a > b || *b > ceiling) {
            return Err("regexp range outside character domain".into());
        }
        let mut sorted = ranges.to_vec();
        sorted.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::new();
        for (a, b) in sorted {
            match merged.last_mut() {
                Some(previous) if a <= previous.1.saturating_add(1) => {
                    previous.1 = previous.1.max(b);
                }
                _ => merged.push((a, b)),
            }
        }
        if negate {
            let mut inverse = Vec::new();
            let mut next = 0;
            for (a, b) in merged {
                if next < a {
                    inverse.push((next, a - 1));
                }
                next = b + 1;
            }
            if next <= ceiling {
                inverse.push((next, ceiling));
            }
            merged = inverse;
        }
        if merged.is_empty() {
            return self.never();
        }
        if merged.len() > 65535 {
            return Err("capacity: regexp normalized ranges".into());
        }
        // The native 16bit range format treats a final FFFF as infinity.
        // Unicode BMP-only sets ending there therefore require range32.
        let wide = merged.iter().any(|(_, end)| *end > 0xffff)
            || (ceiling > 0xffff && merged.last().is_some_and(|(_, end)| *end == 0xffff));
        let size = 2 + merged.len() * if wide { 8 } else { 4 };
        self.charge(size)?;
        let mut bytes = Vec::with_capacity(size);
        bytes.extend((merged.len() as u16).to_ne_bytes());
        for (a, b) in merged {
            if wide {
                bytes.extend(a.to_ne_bytes());
                bytes.extend(b.to_ne_bytes());
            } else {
                bytes.extend((a as u16).to_ne_bytes());
                bytes.extend((b as u16).to_ne_bytes());
            }
        }
        self.emit(if wide { "range32" } else { "range" }, &bytes)?;
        Ok(())
    }
    fn node(&mut self, node: &Pattern, depth: usize) -> Result<(), String> {
        self.charge(1)?;
        if depth > 256 {
            return Err("capacity: regexp nesting".into());
        }
        match node {
            Pattern::Empty => {}
            Pattern::Never => self.never()?,
            Pattern::Literal(units) => self.literal(units)?,
            Pattern::Sequence(nodes) => {
                self.charge(nodes.len())?;
                for node in nodes {
                    self.node(node, depth + 1)?;
                }
            }
            Pattern::Alternatives(nodes) => {
                if nodes.is_empty() {
                    self.never()?;
                } else {
                    self.charge(nodes.len())?;
                    let mut exits = Vec::new();
                    for node in &nodes[..nodes.len() - 1] {
                        let next = self.branch("split_next_first")?;
                        self.node(node, depth + 1)?;
                        exits.push(self.branch("goto")?);
                        self.patch(next, self.code.len())?;
                    }
                    self.node(nodes.last().unwrap(), depth + 1)?;
                    let end = self.code.len();
                    for exit in exits {
                        self.patch(exit, end)?;
                    }
                }
            }
            Pattern::Ranges { ranges, negate } => self.ranges(ranges, *negate)?,
            Pattern::Dot => {
                self.emit(
                    if self.flags & (1 << 3) != 0 {
                        "any"
                    } else {
                        "dot"
                    },
                    &[],
                )?;
            }
            Pattern::Space { negate } => {
                self.emit(if *negate { "not_space" } else { "space" }, &[])?;
            }
            Pattern::LineStart => {
                self.emit(
                    if self.flags & (1 << 2) != 0 {
                        "line_start_m"
                    } else {
                        "line_start"
                    },
                    &[],
                )?;
            }
            Pattern::LineEnd => {
                self.emit(
                    if self.flags & (1 << 2) != 0 {
                        "line_end_m"
                    } else {
                        "line_end"
                    },
                    &[],
                )?;
            }
            Pattern::WordBoundary { negate } => {
                self.emit(
                    if *negate {
                        "not_word_boundary"
                    } else {
                        "word_boundary"
                    },
                    &[],
                )?;
            }
            Pattern::Capture { index, body } => {
                self.emit("save_start", &[*index])?;
                self.node(body, depth + 1)?;
                self.emit("save_end", &[*index])?;
            }
            Pattern::BackReference(indices) => {
                let mut data = Vec::with_capacity(1 + indices.len());
                data.push(indices.len() as u8);
                data.extend(indices);
                self.emit("back_reference", &data)?;
            }
            Pattern::LookAhead { positive, body } => {
                let after = self.branch(if *positive {
                    "lookahead"
                } else {
                    "negative_lookahead"
                })?;
                self.node(body, depth + 1)?;
                self.emit(
                    if *positive {
                        "lookahead_match"
                    } else {
                        "negative_lookahead_match"
                    },
                    &[],
                )?;
                self.patch(after, self.code.len())?;
            }
            Pattern::Repeat {
                body,
                minimum,
                maximum,
                greedy,
            } => {
                if maximum.is_some_and(|max| max < *minimum) {
                    return Err("regexp quantifier bounds".into());
                }
                if let Some(maximum) = maximum {
                    if *maximum != 0 {
                        let initial_skip = if *minimum == 0 {
                            Some(self.branch(if *greedy {
                                "split_next_first"
                            } else {
                                "split_goto_first"
                            })?)
                        } else {
                            None
                        };
                        let mut counter = vec![0];
                        counter.extend(maximum.to_ne_bytes());
                        self.emit("set_i32", &counter)?;
                        let needs_advance = self.nullable(body, depth + 1)?;
                        let body_start = self.code.len();
                        if needs_advance {
                            self.emit("set_char_pos", &[0])?;
                        }
                        self.repeated_body(body, depth + 1)?;
                        let mut operands = vec![0];
                        operands.extend((maximum - minimum).to_ne_bytes());
                        operands.extend(0i32.to_ne_bytes());
                        let branch = self.emit(
                            match (needs_advance, *greedy) {
                                (true, true) => "loop_check_adv_split_goto_first",
                                (true, false) => "loop_check_adv_split_next_first",
                                (false, true) => "loop_split_goto_first",
                                (false, false) => "loop_split_next_first",
                            },
                            &operands,
                        )?;
                        self.patch(branch, body_start)?;
                        if let Some(skip) = initial_skip {
                            self.patch(skip, self.code.len())?;
                        }
                    }
                } else {
                    if *minimum != 0 {
                        let mut counter = vec![0];
                        counter.extend(minimum.to_ne_bytes());
                        self.emit("set_i32", &counter)?;
                        let body_start = self.code.len();
                        self.repeated_body(body, depth + 1)?;
                        let mut operands = vec![0];
                        operands.extend(0i32.to_ne_bytes());
                        let branch = self.emit("loop", &operands)?;
                        self.patch(branch, body_start)?;
                    }
                    let start = self.code.len();
                    let exit = self.branch(if *greedy {
                        "split_next_first"
                    } else {
                        "split_goto_first"
                    })?;
                    let needs_advance = self.nullable(body, depth + 1)?;
                    if needs_advance {
                        self.emit("set_char_pos", &[0])?;
                    }
                    self.repeated_body(body, depth + 1)?;
                    if needs_advance {
                        self.emit("check_advance", &[0])?;
                    }
                    let again = self.branch("goto")?;
                    self.patch(again, start)?;
                    self.patch(exit, self.code.len())?;
                }
            }
        }
        Ok(())
    }
}
/// Compile the owned regexp tree into the native matcher's instruction format.
/// Case folding/named-group metadata are intentionally separate backend passes;
/// their flags cannot be asserted until those passes are implemented.
pub fn compile_pattern(pattern: &Pattern, flags: u16) -> Result<LiteralProgram, String> {
    if flags & !0x17f != 0
        || flags & (1 << 1) != 0
        || flags & (1 << 4) != 0 && flags & (1 << 8) != 0
    {
        return Err("unsupported regexp compiler flags".into());
    }
    let mut compiler = Compiler {
        code: Vec::new(),
        flags,
        work: 0,
        count: 0,
    };
    let mut pending = vec![(pattern, 0usize)];
    let mut captures = std::collections::BTreeSet::new();
    let mut references = [false; 256];
    while let Some((node, depth)) = pending.pop() {
        compiler.charge(1)?;
        if depth > 256 {
            return Err("capacity: regexp nesting".into());
        }
        match node {
            Pattern::Capture { index, body } => {
                if *index == 0
                    || *index == 255
                    || usize::from(*index) != captures.len() + 1
                    || !captures.insert(*index)
                {
                    return Err("regexp capture identity".into());
                }
                pending.push((body, depth + 1));
            }
            Pattern::Sequence(nodes) | Pattern::Alternatives(nodes) => {
                compiler.charge(nodes.len())?;
                for node in nodes.iter().rev() {
                    pending.push((node, depth + 1));
                }
            }
            Pattern::Repeat { body, .. } | Pattern::LookAhead { body, .. } => {
                pending.push((body, depth + 1))
            }
            Pattern::BackReference(indices) => {
                if indices.is_empty() || indices.len() > 255 {
                    return Err("regexp backreference list".into());
                }
                compiler.charge(indices.len())?;
                for index in indices {
                    references[usize::from(*index)] = true;
                }
            }
            _ => {}
        }
    }
    let capture_count = captures.len() + 1;
    if captures
        .iter()
        .copied()
        .ne((1..capture_count).map(|n| n as u8))
        || references
            .iter()
            .enumerate()
            .any(|(index, used)| *used && (index == 0 || index >= capture_count))
    {
        return Err("regexp capture numbering".into());
    }
    if flags & (1 << 5) == 0 {
        let body = compiler.branch("split_goto_first")?;
        compiler.emit("any", &[])?;
        let again = compiler.branch("goto")?;
        compiler.patch(again, 0)?;
        compiler.patch(body, compiler.code.len())?;
    }
    compiler.emit("save_start", &[0])?;
    compiler.node(pattern, 0)?;
    compiler.emit("save_end", &[0])?;
    compiler.emit("match", &[])?;
    let registers = allocate_registers(&mut compiler.code)?;
    validate_destinations(&compiler.code)?;
    let mut bytes = Vec::with_capacity(HEADER + compiler.code.len());
    bytes.extend(flags.to_ne_bytes());
    bytes.extend([capture_count as u8, registers]);
    bytes.extend((compiler.code.len() as u32).to_ne_bytes());
    bytes.extend(compiler.code);
    Ok(LiteralProgram { bytes })
}
