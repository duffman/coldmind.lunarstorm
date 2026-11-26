use std::collections::BTreeMap;

use crate::*;

pub(crate) fn numeric_binary(op: BinaryOp, left: f64, right: f64) -> Scalar {
    match op {
        BinaryOp::Add => Scalar::Number(left + right),
        BinaryOp::Subtract => Scalar::Number(left - right),
        BinaryOp::Multiply => Scalar::Number(left * right),
        BinaryOp::Divide => Scalar::Number(left / right),
        BinaryOp::Remainder => Scalar::Number(left % right),
        BinaryOp::Equal => Scalar::Bool(left == right),
        BinaryOp::LessThan => Scalar::Bool(left < right),
        BinaryOp::LessEqual => Scalar::Bool(left <= right),
        BinaryOp::GreaterThan => Scalar::Bool(left > right),
        BinaryOp::GreaterEqual => Scalar::Bool(left >= right),
    }
}

fn evaluate(kind: &InstructionKind, values: &BTreeMap<ValueId, Scalar>) -> Scalar {
    match *kind {
        InstructionKind::Constant(value) => value,
        InstructionKind::Unary {
            op: UnaryOp::Negate,
            operand,
        } => {
            let Scalar::Number(value) = values[&operand] else {
                unreachable!("validated numeric operand")
            };
            Scalar::Number(-value)
        }
        InstructionKind::Unary {
            op: UnaryOp::Not,
            operand,
        } => {
            let Scalar::Bool(value) = values[&operand] else {
                unreachable!("validated boolean operand")
            };
            Scalar::Bool(!value)
        }
        InstructionKind::Binary { op, left, right } => match (values[&left], values[&right]) {
            (Scalar::Number(left), Scalar::Number(right)) => numeric_binary(op, left, right),
            (Scalar::Bool(left), Scalar::Bool(right)) if op == BinaryOp::Equal => {
                Scalar::Bool(left == right)
            }
            _ => unreachable!("validated binary operands"),
        },
    }
}

fn spend(steps: &mut usize, limit: usize) -> Result<(), ExecutionError> {
    if *steps == limit {
        return Err(ExecutionError::StepLimit { limit });
    }
    *steps += 1;
    Ok(())
}

/// Validate and interpret a scalar function with a finite execution budget.
///
/// `arguments` bind the entry block's parameters. One step is one instruction or
/// terminator; parameter binding does not consume steps. On an edge, every
/// argument is read before any destination parameter is assigned.
pub fn execute(
    function: &Function,
    arguments: &[Scalar],
    step_limit: usize,
) -> Result<Scalar, ExecutionError> {
    validate(function).map_err(ExecutionError::InvalidFunction)?;
    let blocks: BTreeMap<_, _> = function
        .blocks
        .iter()
        .map(|block| (block.id, block))
        .collect();
    let entry = blocks[&function.entry];
    if entry.params.len() != arguments.len() {
        return Err(ExecutionError::ArgumentArity {
            expected: entry.params.len(),
            actual: arguments.len(),
        });
    }
    let mut values = BTreeMap::new();
    for (index, (parameter, argument)) in entry.params.iter().zip(arguments).enumerate() {
        if parameter.ty != argument.ty() {
            return Err(ExecutionError::ArgumentType {
                index,
                expected: parameter.ty,
                actual: argument.ty(),
            });
        }
        values.insert(parameter.value, *argument);
    }
    let mut current = function.entry;
    let mut steps = 0;
    loop {
        let block = blocks[&current];
        for instruction in &block.instructions {
            spend(&mut steps, step_limit)?;
            values.insert(instruction.result, evaluate(&instruction.kind, &values));
        }
        spend(&mut steps, step_limit)?;
        let edge = match &block.terminator.kind {
            TerminatorKind::Return(value) => return Ok(values[value]),
            TerminatorKind::Jump(edge) => edge,
            TerminatorKind::Branch {
                condition,
                then_edge,
                else_edge,
            } => {
                let Scalar::Bool(condition) = values[condition] else {
                    unreachable!("validated branch condition")
                };
                if condition {
                    then_edge
                } else {
                    else_edge
                }
            }
        };
        let arguments: Vec<_> = edge.arguments.iter().map(|value| values[value]).collect();
        for (parameter, argument) in blocks[&edge.target].params.iter().zip(arguments) {
            values.insert(parameter.value, argument);
        }
        current = edge.target;
    }
}
