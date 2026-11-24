//! Pure emitted-image CFG regressions. Native VM value checks live in the adapter tests.
use qjs_compiler_core::{CompilationUnit, SourceMetadata, lower};
use qjs_ir::*;
const S: Span = Span { start: 0, end: 1 };
fn constant(id: u32, value: Scalar) -> Instruction {
    Instruction {
        result: ValueId(id),
        kind: InstructionKind::Constant(value),
        span: S,
    }
}
fn number(id: u32, n: f64) -> Instruction {
    constant(id, Scalar::Number(n))
}
fn binary(id: u32, op: BinaryOp, left: u32, right: u32) -> Instruction {
    Instruction {
        result: ValueId(id),
        kind: InstructionKind::Binary {
            op,
            left: ValueId(left),
            right: ValueId(right),
        },
        span: S,
    }
}
fn edge(target: u32, args: &[u32]) -> Edge {
    Edge {
        target: BlockId(target),
        arguments: args.iter().copied().map(ValueId).collect(),
    }
}
fn block(id: u32, params: &[u32], instructions: Vec<Instruction>, kind: TerminatorKind) -> Block {
    Block {
        id: BlockId(id),
        params: params
            .iter()
            .map(|&id| Parameter {
                value: ValueId(id),
                ty: Type::Number,
                span: S,
            })
            .collect(),
        instructions,
        terminator: Terminator { kind, span: S },
        span: S,
    }
}
fn verified_emission(f: &Function) {
    let unit = CompilationUnit {
        target: lower(f).unwrap(),
        metadata: SourceMetadata {
            source: None,
            filename: "constructed-cfg.ir".into(),
            source_hash: "0".repeat(64),
            source_len: 1,
            function_name: "main".into(),
            strict: false,
            function_span: S,
        },
    };
    let values = f
        .blocks
        .iter()
        .map(|b| b.params.len() + b.instructions.len())
        .sum::<usize>();
    assert!(
        unit.target.locals.len() > values,
        "parameter copies need distinct temporary storage"
    );
    let image = unit.finalize().unwrap();
    assert_eq!(image.bytes()[0], 94); // lunarstorm patch: native format version 94 (user-catch table)
    assert!(image.function().stack_size >= 1);
}
#[test]
fn edge_arguments_use_parallel_copy_semantics() {
    let f = Function {
        source_len: 1,
        entry: BlockId(0),
        return_type: Type::Number,
        blocks: vec![
            block(
                0,
                &[],
                vec![
                    number(0, 1.0),
                    number(1, 2.0),
                    number(2, 0.0),
                    number(3, 1.0),
                    number(4, 3.0),
                    number(5, 10.0),
                ],
                TerminatorKind::Jump(edge(1, &[0, 1, 2])),
            ),
            block(
                1,
                &[6, 7, 8],
                vec![binary(9, BinaryOp::LessThan, 8, 4)],
                TerminatorKind::Branch {
                    condition: ValueId(9),
                    then_edge: edge(2, &[]),
                    else_edge: edge(3, &[]),
                },
            ),
            block(
                2,
                &[],
                vec![binary(10, BinaryOp::Add, 8, 3)],
                TerminatorKind::Jump(edge(1, &[7, 6, 10])),
            ),
            block(
                3,
                &[],
                vec![
                    binary(11, BinaryOp::Multiply, 6, 5),
                    binary(12, BinaryOp::Add, 11, 7),
                ],
                TerminatorKind::Return(ValueId(12)),
            ),
        ],
    };
    assert_eq!(execute(&f, &[], 100).unwrap(), Scalar::Number(21.0));
    verified_emission(&f);
}
#[test]
fn conditional_edge_copies_run_only_on_taken_edge() {
    for (condition, expected) in [(true, 37.0), (false, 91.0)] {
        let f = Function {
            source_len: 1,
            entry: BlockId(0),
            return_type: Type::Number,
            blocks: vec![
                block(
                    0,
                    &[],
                    vec![
                        constant(0, Scalar::Bool(condition)),
                        number(1, 37.0),
                        number(2, 91.0),
                    ],
                    TerminatorKind::Branch {
                        condition: ValueId(0),
                        then_edge: edge(1, &[1]),
                        else_edge: edge(1, &[2]),
                    },
                ),
                block(1, &[3], vec![], TerminatorKind::Return(ValueId(3))),
            ],
        };
        assert_eq!(execute(&f, &[], 100).unwrap(), Scalar::Number(expected));
        verified_emission(&f);
    }
}
