use super::*;
use qjs_bytecode::snapshot::Position;
use std::collections::{BTreeMap, VecDeque};

/// Final native instructions and compiler-owned capacity analysis.
#[derive(Clone, Debug)]
pub struct FinalizedFunction {
    pub code: Vec<u8>,
    pub constants: Vec<Scalar>,
    pub local_count: u32,
    pub stack_size: u32,
    pub positions: Vec<Position>,
    pub debug_locations: Vec<qjs_bytecode::image::DebugLocation>,
}
fn width(op: &Op) -> usize {
    match op {
        Op::Label(_) => 0,
        Op::Constant(Scalar::Number(_)) | Op::Jump(_) | Op::IfFalse(_) => 5,
        Op::GetLocal(_) | Op::PutLocal(_) => 3,
        _ => 1,
    }
}
fn opcode(name: &str) -> Result<u8, String> {
    qjs_bytecode::production::FINAL_OPCODES
        .iter()
        .find(|op| op.name == name)
        .map(|op| op.id)
        .ok_or_else(|| format!("missing production opcode {name}"))
}
/// Resolve labels and emit final VM instructions directly in Rust.
///
/// This never invokes QuickJS compiler state or any native runtime operation.
pub fn emit_target(
    target: &Target,
    metadata: &SourceMetadata,
) -> Result<FinalizedFunction, String> {
    emit_target_with_limits(target, metadata, &crate::EmissionLimits::default())
}
pub fn emit_target_with_limits(
    target: &Target,
    metadata: &SourceMetadata,
    limits: &crate::EmissionLimits,
) -> Result<FinalizedFunction, String> {
    limits.validate()?;
    if target.instructions.len() > limits.max_ir_nodes {
        return Err("capacity: target IR nodes budget exceeded".into());
    }
    let count = target
        .instructions
        .iter()
        .filter(|i| !matches!(i.op, Op::Label(_)))
        .count();
    if count > limits.max_instructions {
        return Err("capacity: target instructions budget exceeded".into());
    }
    if count > limits.max_positions {
        return Err("capacity: target positions budget exceeded".into());
    }
    if target.locals.len() > limits.max_locals {
        return Err("capacity: target locals budget exceeded".into());
    }
    let length = target
        .instructions
        .iter()
        .try_fold(0usize, |n, i| n.checked_add(width(&i.op)))
        .ok_or("capacity: target code bytes overflow")?;
    if length > limits.max_code_bytes {
        return Err("capacity: target code bytes budget exceeded".into());
    }
    if target
        .instructions
        .iter()
        .filter(|i| matches!(i.op, Op::Constant(Scalar::Number(_))))
        .count()
        > limits.max_constants
    {
        return Err("capacity: target constants budget exceeded".into());
    }
    if target
        .instructions
        .len()
        .checked_mul(4)
        .is_none_or(|n| n > limits.max_work)
    {
        return Err("capacity: target analysis work budget exceeded".into());
    }
    if target.instructions.is_empty()
        || target.instructions.len() > 1_000_000
        || target.locals.len() >= 65535
        || target.label_count > 100_000
    {
        return Err("capacity: target size budget exceeded".into());
    }
    let source = metadata.source.as_ref();
    if source.is_some_and(|s| s.original().len() > limits.max_source_units) {
        return Err("capacity: source units budget exceeded".into());
    }
    let source_length = source.map_or(0, |s| s.original().len());
    let lookup_work = count
        .checked_mul((source_length.max(1).ilog2() as usize + 1) * 2)
        .and_then(|n| n.checked_add(source_length))
        .and_then(|n| n.checked_add(target.instructions.len() * 4))
        .ok_or("capacity: source mapping work overflow")?;
    if lookup_work > limits.max_work {
        return Err("capacity: source mapping work budget exceeded".into());
    }
    let line_map = source.map(|s| qjs_frontend::source::SourceLineMap::from_utf16(s.original()));
    let mut labels = BTreeMap::new();
    let mut offsets = Vec::with_capacity(target.instructions.len());
    let mut length = 0usize;
    for (index, instruction) in target.instructions.iter().enumerate() {
        let span = instruction.span;
        if span.start > span.end
            || span.end > metadata.source_len
            || span.start < metadata.function_span.start
            || span.end > metadata.function_span.end
        {
            return Err("source position outside function".into());
        }
        offsets.push(length);
        if let Op::Label(label) = instruction.op
            && (label >= target.label_count || labels.insert(label, (length, index)).is_some())
        {
            return Err("duplicate or out-of-range label".into());
        }
        if let Op::GetLocal(slot) | Op::PutLocal(slot) = instruction.op
            && slot as usize >= target.locals.len()
        {
            return Err("local index outside target table".into());
        }
        length = length
            .checked_add(width(&instruction.op))
            .ok_or("capacity: code size overflow")?;
    }
    if labels.len() != target.label_count as usize {
        return Err("missing target label".into());
    }
    if length > 16 * 1024 * 1024 {
        return Err("capacity: target code budget exceeded".into());
    }
    let stack_size = stack_capacity(target, &labels)?;
    let mut result = FinalizedFunction {
        code: Vec::with_capacity(length),
        constants: Vec::new(),
        local_count: target.locals.len() as u32,
        stack_size,
        positions: Vec::new(),
        debug_locations: Vec::new(),
    };
    for (index, instruction) in target.instructions.iter().enumerate() {
        let name = match instruction.op {
            Op::Label(_) => continue,
            Op::Constant(Scalar::Number(_)) => "push_const",
            Op::Constant(Scalar::Bool(true)) => "push_true",
            Op::Constant(Scalar::Bool(false)) => "push_false",
            Op::GetLocal(_) => "get_loc",
            Op::PutLocal(_) => "put_loc",
            Op::Negate => "neg",
            Op::Not => "lnot",
            Op::Add => "add",
            Op::Subtract => "sub",
            Op::Multiply => "mul",
            Op::Divide => "div",
            Op::Remainder => "mod",
            Op::Equal => "strict_eq",
            Op::LessThan => "lt",
            Op::LessEqual => "lte",
            Op::GreaterThan => "gt",
            Op::GreaterEqual => "gte",
            Op::Jump(_) => "goto",
            Op::IfFalse(_) => "if_false",
            Op::Return => "return",
        };
        if let Some(source) = &metadata.source {
            if !source.unpaired_surrogates().is_empty() {
                return Err("legacy UTF8 spans require original UTF8 source".into());
            }
            if source.parser_text().len() != metadata.source_len as usize {
                return Err("legacy source length disagrees with coordinate map".into());
            }
            let original = source
                .original_span(instruction.span.start as usize..instruction.span.start as usize)
                .ok_or("legacy span outside original source")?;
            let (line, column) = line_map
                .as_ref()
                .unwrap()
                .line_column(original.start)
                .ok_or("legacy span outside original source")?;
            result
                .debug_locations
                .push(qjs_bytecode::image::DebugLocation {
                    code_offset: offsets[index] as u32,
                    line,
                    column,
                });
        }
        result.positions.push(Position {
            code_offset: offsets[index] as u32,
            source_start: instruction.span.start,
            source_end: instruction.span.end,
        });
        result.code.push(opcode(name)?);
        match instruction.op {
            Op::Constant(Scalar::Number(number)) => {
                let slot = u32::try_from(result.constants.len())
                    .map_err(|_| "capacity: constant count overflow")?;
                result.constants.push(Scalar::Number(number));
                result.code.extend_from_slice(&slot.to_ne_bytes());
            }
            Op::GetLocal(slot) | Op::PutLocal(slot) => {
                result.code.extend_from_slice(&(slot as u16).to_ne_bytes())
            }
            Op::Jump(label) | Op::IfFalse(label) => {
                let &(destination, _) = labels.get(&label).ok_or("undefined branch label")?;
                if destination == length {
                    return Err("branch targets end of function".into());
                }
                // QuickJS relative displacements start at the operand, not after it.
                let displacement = i32::try_from(destination as i64 - (offsets[index] as i64 + 1))
                    .map_err(|_| "capacity: branch displacement overflow")?;
                result.code.extend_from_slice(&displacement.to_ne_bytes());
            }
            _ => {}
        }
    }
    Ok(result)
}
// This is the emitter's own symbolic CFG analysis. The verifier independently
// interprets final instruction semantics, indices and definite local assignment.
fn stack_capacity(target: &Target, labels: &BTreeMap<u32, (usize, usize)>) -> Result<u32, String> {
    let mut states = vec![None; target.instructions.len()];
    states[0] = Some(0usize);
    let mut work = VecDeque::from([0]);
    let mut maximum = 0usize;
    while let Some(i) = work.pop_front() {
        let op = &target.instructions[i].op;
        let depth = states[i].expect("queued state");
        let (pop, push) = match op {
            Op::Constant(_) | Op::GetLocal(_) => (0, 1),
            Op::PutLocal(_) | Op::IfFalse(_) | Op::Return => (1, 0),
            Op::Negate | Op::Not => (1, 1),
            Op::Label(_) | Op::Jump(_) => (0, 0),
            _ => (2, 1),
        };
        if depth < pop {
            return Err("target stack underflow".into());
        }
        let next = depth - pop + push;
        maximum = maximum.max(next);
        if maximum > 65535 {
            return Err("capacity: target stack capacity exceeded".into());
        }
        if matches!(op, Op::Return) {
            if next != 0 {
                return Err("return leaves stack values".into());
            }
            continue;
        }
        let mut successors = Vec::with_capacity(2);
        if let Op::Jump(label) | Op::IfFalse(label) = op {
            successors.push(labels.get(label).ok_or("undefined branch label")?.1);
        }
        if !matches!(op, Op::Jump(_)) {
            if i + 1 == target.instructions.len() {
                return Err("function falls through outside code".into());
            }
            successors.push(i + 1);
        }
        for successor in successors {
            match states[successor] {
                None => {
                    states[successor] = Some(next);
                    work.push_back(successor);
                }
                Some(old) if old != next => return Err("inconsistent target stack at join".into()),
                _ => {}
            }
        }
    }
    Ok(maximum as u32)
}
