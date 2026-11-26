use qjs_scalar_ssa_ir::*;

const S: Span = Span { start: 0, end: 1 };

fn number(id: u32, value: f64) -> Instruction {
    Instruction {
        result: ValueId(id),
        kind: InstructionKind::Constant(Scalar::Number(value)),
        span: S,
    }
}

fn boolean(id: u32, value: bool) -> Instruction {
    Instruction {
        result: ValueId(id),
        kind: InstructionKind::Constant(Scalar::Bool(value)),
        span: S,
    }
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

fn parameter(id: u32, ty: Type) -> Parameter {
    Parameter {
        value: ValueId(id),
        ty,
        span: S,
    }
}

fn ret(value: u32) -> Terminator {
    Terminator {
        kind: TerminatorKind::Return(ValueId(value)),
        span: S,
    }
}

fn edge(target: u32, arguments: &[u32]) -> Edge {
    Edge {
        target: BlockId(target),
        arguments: arguments.iter().copied().map(ValueId).collect(),
    }
}

fn simple() -> Function {
    Function {
        source_len: 20,
        entry: BlockId(0),
        return_type: Type::Number,
        blocks: vec![Block {
            id: BlockId(0),
            params: vec![],
            span: S,
            instructions: vec![
                number(0, 6.0),
                number(1, 7.0),
                binary(2, BinaryOp::Multiply, 0, 1),
            ],
            terminator: ret(2),
        }],
    }
}

fn diamond() -> Function {
    Function {
        source_len: 20,
        entry: BlockId(0),
        return_type: Type::Number,
        blocks: vec![
            Block {
                id: BlockId(0),
                params: vec![parameter(0, Type::Bool)],
                span: S,
                instructions: vec![
                    number(1, 6.0),
                    number(2, 7.0),
                    binary(3, BinaryOp::Multiply, 1, 2),
                ],
                terminator: Terminator {
                    kind: TerminatorKind::Branch {
                        condition: ValueId(0),
                        then_edge: edge(1, &[3]),
                        else_edge: edge(2, &[1]),
                    },
                    span: S,
                },
            },
            Block {
                id: BlockId(1),
                params: vec![parameter(4, Type::Number)],
                span: S,
                instructions: vec![],
                terminator: Terminator {
                    kind: TerminatorKind::Jump(edge(3, &[4])),
                    span: S,
                },
            },
            Block {
                id: BlockId(2),
                params: vec![parameter(5, Type::Number)],
                span: S,
                instructions: vec![binary(6, BinaryOp::Add, 5, 1)],
                terminator: Terminator {
                    kind: TerminatorKind::Jump(edge(3, &[6])),
                    span: S,
                },
            },
            Block {
                id: BlockId(3),
                params: vec![parameter(7, Type::Number)],
                span: S,
                instructions: vec![],
                terminator: ret(7),
            },
        ],
    }
}

fn has_error(function: &Function, kind: ValidationErrorKind) {
    let errors = validate(function).expect_err("invalid IR must be rejected");
    assert!(
        errors.iter().any(|error| error.kind == kind),
        "expected {kind:?}; got {errors:?}"
    );
}

fn numeric(result: Scalar) -> f64 {
    match result {
        Scalar::Number(value) => value,
        other => panic!("expected Number, got {other:?}"),
    }
}

#[test]
fn validates_executes_and_folds_to_42_preserving_span() {
    let mut function = simple();
    function.blocks[0].instructions[2].span = Span { start: 2, end: 9 };
    assert_eq!(validate(&function), Ok(()));
    assert_eq!(execute(&function, &[], 100).unwrap(), Scalar::Number(42.0));
    assert_eq!(
        fold_constants(&mut function).unwrap().folded_instructions,
        1
    );
    assert_eq!(
        function.blocks[0].instructions[2].kind,
        InstructionKind::Constant(Scalar::Number(42.0))
    );
    assert_eq!(
        function.blocks[0].instructions[2].span,
        Span { start: 2, end: 9 }
    );
    assert_eq!(execute(&function, &[], 100).unwrap(), Scalar::Number(42.0));
    assert_eq!(
        fold_constants(&mut function).unwrap().folded_instructions,
        0
    );
}

#[test]
fn branching_and_block_arguments_have_identical_before_after_results() {
    let mut function = diamond();
    let before_true = execute(&function, &[Scalar::Bool(true)], 100).unwrap();
    let before_false = execute(&function, &[Scalar::Bool(false)], 100).unwrap();
    assert_eq!(before_true, Scalar::Number(42.0));
    assert_eq!(before_false, Scalar::Number(12.0));
    assert_eq!(
        fold_constants(&mut function).unwrap().folded_instructions,
        1
    );
    assert_eq!(
        execute(&function, &[Scalar::Bool(true)], 100).unwrap(),
        before_true
    );
    assert_eq!(
        execute(&function, &[Scalar::Bool(false)], 100).unwrap(),
        before_false
    );
    assert!(matches!(
        function.blocks[2].instructions[0].kind,
        InstructionKind::Binary { .. }
    ));
}

#[test]
fn folds_transitively_even_when_block_storage_is_not_execution_order() {
    let mut function = simple();
    function.blocks[0].terminator = Terminator {
        kind: TerminatorKind::Jump(edge(8, &[])),
        span: S,
    };
    function.blocks.insert(
        0,
        Block {
            id: BlockId(8),
            params: vec![],
            span: S,
            instructions: vec![binary(3, BinaryOp::Add, 2, 0)],
            terminator: ret(3),
        },
    );
    assert_eq!(
        fold_constants(&mut function).unwrap().folded_instructions,
        2
    );
    assert_eq!(execute(&function, &[], 100).unwrap(), Scalar::Number(48.0));
}

#[test]
fn folding_preserves_negative_zero_nan_and_infinity_semantics() {
    for (left, right, op) in [
        (-0.0, 1.0, BinaryOp::Multiply),
        (0.0, -1.0, BinaryOp::Divide),
        (0.0, 0.0, BinaryOp::Divide),
        (f64::NAN, 0.0, BinaryOp::Multiply),
        (f64::INFINITY, 0.0, BinaryOp::Multiply),
        (-5.0, 2.0, BinaryOp::Remainder),
    ] {
        let mut function = simple();
        function.blocks[0].instructions =
            vec![number(0, left), number(1, right), binary(2, op, 0, 1)];
        let before = numeric(execute(&function, &[], 100).unwrap());
        assert_eq!(
            fold_constants(&mut function).unwrap().folded_instructions,
            1
        );
        let after = numeric(execute(&function, &[], 100).unwrap());
        assert_eq!(before.to_bits(), after.to_bits());
    }
}

#[test]
fn does_not_simplify_unknown_number_times_zero() {
    let mut function = simple();
    function.blocks[0].params = vec![parameter(0, Type::Number)];
    function.blocks[0].instructions = vec![number(1, 0.0), binary(2, BinaryOp::Multiply, 0, 1)];
    assert_eq!(
        fold_constants(&mut function).unwrap().folded_instructions,
        0
    );
    assert!(numeric(execute(&function, &[Scalar::Number(f64::INFINITY)], 100).unwrap()).is_nan());
    assert_eq!(
        numeric(execute(&function, &[Scalar::Number(-2.0)], 100).unwrap()).to_bits(),
        (-0.0_f64).to_bits()
    );
}

#[test]
fn numeric_comparison_and_boolean_negation_execute() {
    let mut function = simple();
    function.return_type = Type::Bool;
    function.blocks[0].instructions[2] = binary(2, BinaryOp::LessThan, 0, 1);
    function.blocks[0].instructions.push(Instruction {
        result: ValueId(3),
        kind: InstructionKind::Unary {
            op: UnaryOp::Not,
            operand: ValueId(2),
        },
        span: S,
    });
    function.blocks[0].terminator = ret(3);
    assert_eq!(execute(&function, &[], 100).unwrap(), Scalar::Bool(false));
    assert_eq!(
        fold_constants(&mut function).unwrap().folded_instructions,
        1
    );
    assert_eq!(execute(&function, &[], 100).unwrap(), Scalar::Bool(false));
}

#[test]
fn negation_preserves_negative_zero() {
    let mut function = simple();
    function.blocks[0].instructions = vec![
        number(0, 0.0),
        Instruction {
            result: ValueId(1),
            kind: InstructionKind::Unary {
                op: UnaryOp::Negate,
                operand: ValueId(0),
            },
            span: S,
        },
    ];
    function.blocks[0].terminator = ret(1);
    assert_eq!(
        fold_constants(&mut function).unwrap().folded_instructions,
        1
    );
    assert_eq!(
        numeric(execute(&function, &[], 100).unwrap()).to_bits(),
        (-0.0_f64).to_bits()
    );
}

#[test]
fn rejects_undefined_values() {
    let mut function = simple();
    function.blocks[0].terminator = ret(100);
    has_error(&function, ValidationErrorKind::UndefinedValue);
}

#[test]
fn rejects_duplicate_blocks_and_values_including_parameters() {
    let mut function = simple();
    function.blocks.push(function.blocks[0].clone());
    has_error(&function, ValidationErrorKind::DuplicateBlock);
    let mut function = simple();
    function.blocks[0].params.push(parameter(0, Type::Number));
    has_error(&function, ValidationErrorKind::DuplicateValue);
}

#[test]
fn rejects_missing_entry_and_invalid_targets() {
    let mut function = simple();
    function.entry = BlockId(99);
    has_error(&function, ValidationErrorKind::InvalidEntry);
    function.entry = BlockId(0);
    function.blocks[0].terminator.kind = TerminatorKind::Jump(edge(99, &[]));
    has_error(&function, ValidationErrorKind::InvalidTarget);
}

#[test]
fn rejects_branch_condition_and_edge_arity_and_type_errors() {
    let mut function = diamond();
    if let TerminatorKind::Branch { condition, .. } = &mut function.blocks[0].terminator.kind {
        *condition = ValueId(1);
    }
    has_error(&function, ValidationErrorKind::TypeMismatch);
    let mut function = diamond();
    if let TerminatorKind::Branch { then_edge, .. } = &mut function.blocks[0].terminator.kind {
        then_edge.arguments.clear();
    }
    has_error(&function, ValidationErrorKind::ArgumentArity);
    let mut function = diamond();
    if let TerminatorKind::Branch { then_edge, .. } = &mut function.blocks[0].terminator.kind {
        then_edge.arguments[0] = ValueId(0);
    }
    has_error(&function, ValidationErrorKind::TypeMismatch);
}

#[test]
fn rejects_operand_and_return_type_errors() {
    let mut function = simple();
    function.blocks[0].instructions[0] = boolean(0, true);
    has_error(&function, ValidationErrorKind::TypeMismatch);
    let mut function = simple();
    function.return_type = Type::Bool;
    has_error(&function, ValidationErrorKind::TypeMismatch);
}

#[test]
fn rejects_invalid_spans_on_every_provenance_location() {
    for location in 0..4 {
        let mut function = diamond();
        let invalid = Span { start: 10, end: 3 };
        match location {
            0 => function.blocks[0].span = invalid,
            1 => function.blocks[0].params[0].span = invalid,
            2 => function.blocks[0].instructions[0].span = invalid,
            _ => function.blocks[0].terminator.span = invalid,
        }
        has_error(&function, ValidationErrorKind::InvalidSpan);
    }
    let mut function = simple();
    function.blocks[0].span = Span { start: 0, end: 21 };
    has_error(&function, ValidationErrorKind::InvalidSpan);
}

#[test]
fn rejects_use_before_definition_and_self_reference() {
    let mut function = simple();
    function.blocks[0].instructions.swap(0, 2);
    has_error(&function, ValidationErrorKind::UseBeforeDefinition);
    let mut function = simple();
    function.blocks[0].instructions[2] = binary(2, BinaryOp::Add, 2, 0);
    has_error(&function, ValidationErrorKind::UseBeforeDefinition);
}

#[test]
fn rejects_value_from_sibling_branch_without_block_parameter() {
    let mut function = diamond();
    function.blocks[3].terminator = ret(6);
    has_error(&function, ValidationErrorKind::DoesNotDominate);
}

#[test]
fn rejects_unreachable_blocks() {
    let mut function = simple();
    function.blocks.push(Block {
        id: BlockId(1),
        params: vec![],
        span: S,
        instructions: vec![number(9, 0.0)],
        terminator: ret(9),
    });
    has_error(&function, ValidationErrorKind::UnreachableBlock);
}

#[test]
fn loops_use_simultaneous_block_arguments_and_a_step_budget() {
    let function = Function {
        source_len: 1,
        entry: BlockId(0),
        return_type: Type::Number,
        blocks: vec![
            Block {
                id: BlockId(0),
                params: vec![],
                span: S,
                instructions: vec![number(0, 0.0), number(1, 1.0), number(2, 3.0)],
                terminator: Terminator {
                    kind: TerminatorKind::Jump(edge(1, &[0, 1])),
                    span: S,
                },
            },
            Block {
                id: BlockId(1),
                params: vec![parameter(3, Type::Number), parameter(4, Type::Number)],
                span: S,
                instructions: vec![
                    binary(5, BinaryOp::LessThan, 3, 2),
                    binary(6, BinaryOp::Add, 3, 1),
                ],
                terminator: Terminator {
                    kind: TerminatorKind::Branch {
                        condition: ValueId(5),
                        then_edge: edge(1, &[6, 3]),
                        else_edge: edge(2, &[4]),
                    },
                    span: S,
                },
            },
            Block {
                id: BlockId(2),
                params: vec![parameter(7, Type::Number)],
                span: S,
                instructions: vec![],
                terminator: ret(7),
            },
        ],
    };
    assert_eq!(execute(&function, &[], 100).unwrap(), Scalar::Number(2.0));
    assert!(matches!(
        execute(&function, &[], 2),
        Err(ExecutionError::StepLimit { limit: 2 })
    ));
}

#[test]
fn invalid_ir_is_rejected_before_execution_or_mutation() {
    let mut function = simple();
    function.blocks[0].terminator = ret(99);
    let before = function.clone();
    assert!(matches!(
        execute(&function, &[], 100),
        Err(ExecutionError::InvalidFunction(_))
    ));
    assert!(fold_constants(&mut function).is_err());
    assert_eq!(function, before);
}

#[test]
fn interpreter_checks_entry_arguments() {
    let function = diamond();
    assert!(matches!(
        execute(&function, &[], 100),
        Err(ExecutionError::ArgumentArity {
            expected: 1,
            actual: 0
        })
    ));
    assert!(matches!(
        execute(&function, &[Scalar::Number(1.0)], 100),
        Err(ExecutionError::ArgumentType {
            index: 0,
            expected: Type::Bool,
            actual: Type::Number
        })
    ));
}

#[test]
fn every_numeric_binary_operation_has_equivalent_folded_semantics() {
    for (op, expected) in [
        (BinaryOp::Add, Scalar::Number(13.0)),
        (BinaryOp::Subtract, Scalar::Number(-1.0)),
        (BinaryOp::Multiply, Scalar::Number(42.0)),
        (BinaryOp::Divide, Scalar::Number(6.0 / 7.0)),
        (BinaryOp::Remainder, Scalar::Number(6.0)),
        (BinaryOp::Equal, Scalar::Bool(false)),
        (BinaryOp::LessThan, Scalar::Bool(true)),
        (BinaryOp::LessEqual, Scalar::Bool(true)),
        (BinaryOp::GreaterThan, Scalar::Bool(false)),
        (BinaryOp::GreaterEqual, Scalar::Bool(false)),
    ] {
        let mut function = simple();
        function.return_type = expected.ty();
        function.blocks[0].instructions[2] = binary(2, op, 0, 1);
        assert_eq!(execute(&function, &[], 100).unwrap(), expected);
        assert_eq!(
            fold_constants(&mut function).unwrap().folded_instructions,
            1
        );
        assert_eq!(execute(&function, &[], 100).unwrap(), expected);
    }
}

#[test]
fn boolean_equality_executes_but_is_not_numeric_folding() {
    let mut function = simple();
    function.return_type = Type::Bool;
    function.blocks[0].instructions = vec![
        boolean(0, true),
        boolean(1, true),
        binary(2, BinaryOp::Equal, 0, 1),
    ];
    assert_eq!(execute(&function, &[], 4).unwrap(), Scalar::Bool(true));
    assert_eq!(
        fold_constants(&mut function).unwrap().folded_instructions,
        0
    );
    assert!(matches!(
        execute(&function, &[], 3),
        Err(ExecutionError::StepLimit { limit: 3 })
    ));
    function.blocks[0].instructions[1] = number(1, 1.0);
    has_error(&function, ValidationErrorKind::TypeMismatch);
}

#[test]
fn zero_length_spans_are_valid_and_empty_functions_are_invalid() {
    let mut function = simple();
    function.blocks[0].span = Span { start: 20, end: 20 };
    assert_eq!(validate(&function), Ok(()));
    function.blocks.clear();
    has_error(&function, ValidationErrorKind::InvalidEntry);
}

#[test]
fn nan_comparisons_and_signed_zero_equality_match_binary64() {
    for (left, right, op, expected) in [
        (f64::NAN, f64::NAN, BinaryOp::Equal, false),
        (f64::NAN, 1.0, BinaryOp::LessEqual, false),
        (1.0, f64::NAN, BinaryOp::GreaterEqual, false),
        (-0.0, 0.0, BinaryOp::Equal, true),
    ] {
        let mut function = simple();
        function.return_type = Type::Bool;
        function.blocks[0].instructions =
            vec![number(0, left), number(1, right), binary(2, op, 0, 1)];
        assert_eq!(execute(&function, &[], 4).unwrap(), Scalar::Bool(expected));
        assert_eq!(
            fold_constants(&mut function).unwrap().folded_instructions,
            1
        );
        assert_eq!(execute(&function, &[], 4).unwrap(), Scalar::Bool(expected));
    }
}
