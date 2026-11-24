use std::collections::BTreeMap;

use crate::{interpreter::numeric_binary, *};

fn numeric_constant(
    kind: &InstructionKind,
    constants: &BTreeMap<ValueId, Scalar>,
) -> Option<Scalar> {
    match *kind {
        InstructionKind::Unary {
            op: UnaryOp::Negate,
            operand,
        } => match constants.get(&operand)? {
            Scalar::Number(value) => Some(Scalar::Number(-value)),
            Scalar::Bool(_) => None,
        },
        InstructionKind::Binary { op, left, right } => {
            match (*constants.get(&left)?, *constants.get(&right)?) {
                (Scalar::Number(left), Scalar::Number(right)) => {
                    Some(numeric_binary(op, left, right))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Replace operations whose operands are known numeric constants.
///
/// Validation runs before mutation. The pass retains every instruction's result
/// ID and source span, leaves block parameters and control flow intact, and uses
/// exactly the interpreter's binary64 operations. It does not apply algebraic
/// identities, reassociate arithmetic, infer edge constants or fold boolean
/// operators. NaNs and signed zero are never normalized.
pub fn fold_constants(function: &mut Function) -> Result<FoldReport, Vec<ValidationError>> {
    validate(function)?;
    let mut constants: BTreeMap<_, _> = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| match instruction.kind {
            InstructionKind::Constant(value) => Some((instruction.result, value)),
            _ => None,
        })
        .collect();
    let mut report = FoldReport::default();
    // A fixed point handles arbitrary block storage order. Each successful
    // iteration removes at least one operation, so this always terminates.
    loop {
        let mut changed = false;
        for block in &mut function.blocks {
            for instruction in &mut block.instructions {
                if let Some(value) = numeric_constant(&instruction.kind, &constants) {
                    instruction.kind = InstructionKind::Constant(value);
                    constants.insert(instruction.result, value);
                    report.folded_instructions += 1;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    Ok(report)
}
