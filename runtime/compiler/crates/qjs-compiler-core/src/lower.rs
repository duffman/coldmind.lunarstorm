use super::*;
use qjs_ir::{BinaryOp, BlockId, Edge, InstructionKind, TerminatorKind, UnaryOp};
use std::collections::BTreeMap;
pub fn lower_with_limits(f: &Function, limits: &crate::EmissionLimits) -> Result<Target, String> {
    crate::budget::scalar_ir(f, limits)?;
    // Program-wide emission caps exceed the per-function validator's hard
    // ceilings, which reject any increase; only ever tighten them.
    let ceiling = qjs_ir::ValidationLimits::default();
    qjs_ir::validate_with_limits(
        f,
        &qjs_ir::ValidationLimits {
            max_nodes: limits.max_ir_nodes.min(ceiling.max_nodes),
            max_work: limits.max_work.min(ceiling.max_work),
            ..ceiling
        },
    )
    .map_err(|errors| {
        if errors
            .iter()
            .any(|e| e.kind == qjs_ir::ValidationErrorKind::Capacity)
        {
            format!("capacity: scalar IR validation budget exceeded: {errors:?}")
        } else {
            format!("invalid IR: {errors:?}")
        }
    })?;
    let blocks: BTreeMap<_, _> = f.blocks.iter().map(|b| (b.id, b)).collect();
    if !blocks[&f.entry].params.is_empty() {
        return Err("entry arguments unsupported".into());
    }
    let mut values = BTreeMap::new();
    for b in &f.blocks {
        for p in &b.params {
            values.insert(p.value, p.ty);
        }
        for i in &b.instructions {
            values.insert(i.result, i.kind.result_type());
        }
    }
    let slots: BTreeMap<_, _> = values
        .keys()
        .enumerate()
        .map(|(i, v)| (*v, i as u32))
        .collect();
    let mut locals: Vec<_> = values
        .iter()
        .map(|(v, t)| Local {
            value: Some(*v),
            ty: *t,
        })
        .collect();
    let labels: BTreeMap<BlockId, u32> = blocks
        .keys()
        .enumerate()
        .map(|(i, b)| (*b, i as u32))
        .collect();
    let mut count = labels.len() as u32;
    let mut out = vec![Instruction {
        op: Op::Jump(labels[&f.entry]),
        span: blocks[&f.entry].span,
    }];
    for b in &f.blocks {
        let mut emit = |op, span| out.push(Instruction { op, span });
        emit(Op::Label(labels[&b.id]), b.span);
        for i in &b.instructions {
            match i.kind {
                InstructionKind::Constant(v) => emit(Op::Constant(v), i.span),
                InstructionKind::Unary { op, operand } => {
                    emit(Op::GetLocal(slots[&operand]), i.span);
                    emit(
                        match op {
                            UnaryOp::Negate => Op::Negate,
                            UnaryOp::Not => Op::Not,
                        },
                        i.span,
                    );
                }
                InstructionKind::Binary { op, left, right } => {
                    emit(Op::GetLocal(slots[&left]), i.span);
                    emit(Op::GetLocal(slots[&right]), i.span);
                    emit(
                        match op {
                            BinaryOp::Add => Op::Add,
                            BinaryOp::Subtract => Op::Subtract,
                            BinaryOp::Multiply => Op::Multiply,
                            BinaryOp::Divide => Op::Divide,
                            BinaryOp::Remainder => Op::Remainder,
                            BinaryOp::Equal => Op::Equal,
                            BinaryOp::LessThan => Op::LessThan,
                            BinaryOp::LessEqual => Op::LessEqual,
                            BinaryOp::GreaterThan => Op::GreaterThan,
                            BinaryOp::GreaterEqual => Op::GreaterEqual,
                        },
                        i.span,
                    );
                }
            }
            emit(Op::PutLocal(slots[&i.result]), i.span);
        }
        let s = b.terminator.span;
        match &b.terminator.kind {
            TerminatorKind::Return(v) => {
                emit(Op::GetLocal(slots[v]), s);
                emit(Op::Return, s);
            }
            TerminatorKind::Jump(e) => {
                copies(e, s, &blocks, &slots, &labels, &mut locals, &mut out)
            }
            TerminatorKind::Branch {
                condition,
                then_edge,
                else_edge,
            } => {
                let otherwise = count;
                count += 1;
                emit(Op::GetLocal(slots[condition]), s);
                emit(Op::IfFalse(otherwise), s);
                copies(
                    then_edge,
                    s,
                    &blocks,
                    &slots,
                    &labels,
                    &mut locals,
                    &mut out,
                );
                out.push(Instruction {
                    op: Op::Label(otherwise),
                    span: s,
                });
                copies(
                    else_edge,
                    s,
                    &blocks,
                    &slots,
                    &labels,
                    &mut locals,
                    &mut out,
                );
            }
        }
    }
    if locals.len() >= 65535 {
        return Err("capacity: local budget exceeded".into());
    }
    Ok(Target {
        instructions: out,
        locals,
        label_count: count,
    })
}
fn copies(
    e: &Edge,
    s: Span,
    blocks: &BTreeMap<BlockId, &qjs_ir::Block>,
    slots: &BTreeMap<ValueId, u32>,
    labels: &BTreeMap<BlockId, u32>,
    locals: &mut Vec<Local>,
    out: &mut Vec<Instruction>,
) {
    let mut emit = |op| out.push(Instruction { op, span: s });
    let first = locals.len() as u32;
    // Save every source before updating any destination: cycles and swaps are valid.
    for (a, p) in e.arguments.iter().zip(&blocks[&e.target].params) {
        let temp = locals.len() as u32;
        locals.push(Local {
            value: None,
            ty: p.ty,
        });
        emit(Op::GetLocal(slots[a]));
        emit(Op::PutLocal(temp));
    }
    for (i, p) in blocks[&e.target].params.iter().enumerate() {
        emit(Op::GetLocal(first + i as u32));
        emit(Op::PutLocal(slots[&p.value]));
    }
    emit(Op::Jump(labels[&e.target]));
}
