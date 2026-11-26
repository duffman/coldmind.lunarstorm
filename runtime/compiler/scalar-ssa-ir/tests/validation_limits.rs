use qjs_scalar_ssa_ir::*;
const S: Span = Span { start: 0, end: 1 };
fn chain(n: u32) -> Function {
    Function {
        source_len: 1,
        entry: BlockId(0),
        return_type: Type::Number,
        blocks: (0..n)
            .map(|id| Block {
                id: BlockId(id),
                params: vec![],
                span: S,
                instructions: if id == 0 {
                    vec![Instruction {
                        result: ValueId(0),
                        kind: InstructionKind::Constant(Scalar::Number(42.0)),
                        span: S,
                    }]
                } else {
                    vec![]
                },
                terminator: Terminator {
                    kind: if id + 1 == n {
                        TerminatorKind::Return(ValueId(0))
                    } else {
                        TerminatorKind::Jump(Edge {
                            target: BlockId(id + 1),
                            arguments: vec![],
                        })
                    },
                    span: S,
                },
            })
            .collect(),
    }
}
fn capacity(result: Result<(), Vec<ValidationError>>) {
    let errors = result.unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.kind == ValidationErrorKind::Capacity),
        "{errors:?}"
    );
}
#[test]
fn quadratic_dominator_cells_are_admitted_before_set_construction() {
    let function = chain(3);
    for cells in [8, 9, 10] {
        let result = validate_with_limits(
            &function,
            &ValidationLimits {
                max_state_cells: cells,
                ..Default::default()
            },
        );
        if cells < 9 {
            capacity(result);
        } else {
            result.unwrap();
        }
    }
}
#[test]
fn work_exhaustion_interrupts_dominator_analysis() {
    let function = chain(3);
    capacity(validate_with_limits(
        &function,
        &ValidationLimits {
            max_work: 1,
            ..Default::default()
        },
    ));
}

#[test]
fn structural_nodes_have_an_exact_boundary() {
    // Function + three blocks/terminators + one instruction + two edges.
    let function = chain(3);
    for nodes in [9, 10, 11] {
        let result = validate_with_limits(
            &function,
            &ValidationLimits {
                max_nodes: nodes,
                ..Default::default()
            },
        );
        if nodes < 10 {
            capacity(result);
        } else {
            result.unwrap();
        }
    }
}

#[test]
fn work_includes_dominator_clones_intersections_and_fixed_point_rescans() {
    let function = chain(3);
    // Structural admission is only ten nodes. Exhaust inside real dominator
    // iteration, then check the complete validation's exact work boundary.
    capacity(validate_with_limits(
        &function,
        &ValidationLimits {
            max_work: 50,
            ..Default::default()
        },
    ));
    for work in [98, 99, 100] {
        let result = validate_with_limits(
            &function,
            &ValidationLimits {
                max_work: work,
                ..Default::default()
            },
        );
        if work < 99 {
            capacity(result);
        } else {
            result.unwrap();
        }
    }
}

#[test]
fn override_cannot_expand_qualified_limits() {
    let defaults = ValidationLimits::default();
    for limits in [
        ValidationLimits {
            max_nodes: defaults.max_nodes + 1,
            ..defaults
        },
        ValidationLimits {
            max_state_cells: defaults.max_state_cells + 1,
            ..defaults
        },
        ValidationLimits {
            max_work: defaults.max_work + 1,
            ..defaults
        },
    ] {
        capacity(validate_with_limits(&chain(1), &limits));
    }
}

#[test]
fn default_validation_rejects_quadratic_state_before_cloning_it() {
    // 2829² > 8 million; the caller-owned input is only a linear chain.
    capacity(validate(&chain(2829)));
}

#[test]
fn tight_limits_preserve_sparse_ids_and_storage_order_independence() {
    let mut function = chain(3);
    for block in &mut function.blocks {
        block.id.0 = block.id.0 * 10 + 7;
        if let TerminatorKind::Jump(edge) = &mut block.terminator.kind {
            edge.target.0 = edge.target.0 * 10 + 7;
        }
    }
    function.entry = BlockId(7);
    function.blocks.reverse();
    validate_with_limits(
        &function,
        &ValidationLimits {
            max_state_cells: 9,
            ..Default::default()
        },
    )
    .unwrap();
}
