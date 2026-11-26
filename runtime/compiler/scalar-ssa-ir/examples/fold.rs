//! Example: builds typed scalar IR by hand, validates it, folds its constants and runs it.

use qjs_scalar_ssa_ir::{
    execute, fold_constants, validate, BinaryOp, Block, BlockId, Edge, Function, Instruction,
    InstructionKind, Parameter, Scalar, Span, Terminator, TerminatorKind, Type, ValueId,
};

fn main() -> Result<(), String> {
    // This example constructs typed IR directly; it does not parse JavaScript.
    let span = Span { start: 0, end: 5 };
    let mut function = Function {
        source_len: 5,
        entry: BlockId(0),
        return_type: Type::Number,
        blocks: vec![
            Block {
                id: BlockId(0),
                span,
                params: vec![Parameter {
                    value: ValueId(0),
                    ty: Type::Bool,
                    span,
                }],
                instructions: vec![
                    Instruction {
                        result: ValueId(1),
                        kind: InstructionKind::Constant(Scalar::Number(6.0)),
                        span,
                    },
                    Instruction {
                        result: ValueId(2),
                        kind: InstructionKind::Constant(Scalar::Number(7.0)),
                        span,
                    },
                    Instruction {
                        result: ValueId(3),
                        kind: InstructionKind::Binary {
                            op: BinaryOp::Multiply,
                            left: ValueId(1),
                            right: ValueId(2),
                        },
                        span,
                    },
                ],
                terminator: Terminator {
                    kind: TerminatorKind::Branch {
                        condition: ValueId(0),
                        then_edge: Edge {
                            target: BlockId(1),
                            arguments: vec![ValueId(3)],
                        },
                        else_edge: Edge {
                            target: BlockId(1),
                            arguments: vec![ValueId(1)],
                        },
                    },
                    span,
                },
            },
            Block {
                id: BlockId(1),
                span,
                params: vec![Parameter {
                    value: ValueId(4),
                    ty: Type::Number,
                    span,
                }],
                instructions: vec![],
                terminator: Terminator {
                    kind: TerminatorKind::Return(ValueId(4)),
                    span,
                },
            },
        ],
    };
    validate(&function).map_err(|errors| format!("validation failed: {errors:?}"))?;
    let before =
        execute(&function, &[Scalar::Bool(true)], 100).map_err(|error| error.to_string())?;
    let report = fold_constants(&mut function)
        .map_err(|errors| format!("fold validation failed: {errors:?}"))?;
    validate(&function).map_err(|errors| format!("post-fold validation failed: {errors:?}"))?;
    let after =
        execute(&function, &[Scalar::Bool(true)], 100).map_err(|error| error.to_string())?;
    assert_eq!(before, Scalar::Number(42.0));
    assert_eq!(before, after);
    println!("validation: passed");
    println!("folded instructions: {}", report.folded_instructions);
    println!("before: {before:?}; after: {after:?}");
    Ok(())
}
