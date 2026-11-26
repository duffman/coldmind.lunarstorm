//! Basic emission: arithmetic without the C compiler, strict boolean `!`, numbers, jumps
//! both ways and full-width local operands.

use qjs_bytecode_emitter::*;
#[test]
fn rust_emits_arithmetic_without_native_finalizer() {
    let unit = compile(
        "function main(){return 6*7;}",
        "answer.js",
        &CompileOptions::default(),
    )
    .unwrap();
    let final_code = emit_target(&unit.target, &unit.metadata)
        .expect("valid scalar target must emit final code");
    assert_eq!(final_code.local_count, 3);
    assert_eq!(final_code.stack_size, 2);
    assert_eq!(final_code.constants.len(), 2);
    assert!(!final_code.code.is_empty());
    assert!(!final_code.positions.is_empty());
}

fn metadata() -> SourceMetadata {
    SourceMetadata {
        source: None,
        filename: "constructed.ir".into(),
        source_hash: "0".repeat(64),
        source_len: 1,
        function_name: "main".into(),
        strict: false,
        function_span: qjs_scalar_ssa_ir::Span { start: 0, end: 1 },
    }
}
fn target(ops: Vec<Op>, locals: usize, labels: u32) -> Target {
    Target {
        instructions: ops
            .into_iter()
            .map(|op| Instruction {
                op,
                span: qjs_scalar_ssa_ir::Span { start: 0, end: 1 },
            })
            .collect(),
        locals: (0..locals)
            .map(|_| Local {
                value: None,
                ty: qjs_scalar_ssa_ir::Type::Number,
            })
            .collect(),
        label_count: labels,
    }
}
#[test]
fn strict_boolean_not_is_logical_and_serializes_in_rust() {
    let unit = compile(
        "function main(){\"use strict\";return !false;}",
        "not.js",
        &CompileOptions::default(),
    )
    .unwrap();
    let image = unit.finalize().unwrap();
    assert!(image.function().strict);
    assert_eq!(image.bytes()[0], 94); // lunarstorm patch: native format version 94 (user-catch table)
    let decoded = qjs_bytecode_image_verifier::decode_schema(
        &image.function().code,
        qjs_bytecode_image_verifier::Limits::default(),
        qjs_bytecode_image_verifier::production::FINAL_OPCODES,
    )
    .unwrap();
    assert!(
        decoded
            .instructions()
            .iter()
            .any(|i| i.opcode().name == "lnot")
    );
    assert!(
        !decoded
            .instructions()
            .iter()
            .any(|i| i.opcode().name == "not")
    );
}
#[test]
fn optimized_numbers_preserve_ieee_bits() {
    for source in [
        "function main(){return -0;}",
        "function main(){return 0/0;}",
        "function main(){return 1/0;}",
    ] {
        let unit = compile(source, "ieee.js", &CompileOptions { optimize: true }).unwrap();
        let emitted = emit_target(&unit.target, &unit.metadata).unwrap();
        let last = emitted.constants.last().unwrap();
        let qjs_scalar_ssa_ir::Scalar::Number(number) = last else {
            panic!("number")
        };
        if source.contains("-0") {
            assert_eq!(number.to_bits(), (-0.0f64).to_bits());
        } else if source.contains("0/0") {
            assert!(number.is_nan());
        } else {
            assert_eq!(*number, f64::INFINITY);
        }
        unit.finalize().unwrap();
    }
}
#[test]
fn forward_and_backward_jumps_use_operand_address() {
    let t = target(
        vec![
            Op::Constant(qjs_scalar_ssa_ir::Scalar::Bool(false)),
            Op::IfFalse(1),
            Op::Label(0),
            Op::Jump(0),
            Op::Label(1),
            Op::Constant(qjs_scalar_ssa_ir::Scalar::Number(42.0)),
            Op::Return,
        ],
        0,
        2,
    );
    let f = emit_target(&t, &metadata()).unwrap();
    let decoded = qjs_bytecode_image_verifier::validate_structure_schema(
        &f.code,
        qjs_bytecode_image_verifier::Limits::default(),
        qjs_bytecode_image_verifier::production::FINAL_OPCODES,
    )
    .unwrap();
    let branches: Vec<_> = decoded
        .instructions()
        .iter()
        .filter_map(|i| i.branch_target().map(|target| (i.offset(), target)))
        .collect();
    assert_eq!(branches, vec![(1, 11), (6, 6)]);
    assert_eq!(f.stack_size, 1);
}
#[test]
fn rejects_bad_labels_slots_and_stack_joins() {
    let bads = [
        target(vec![Op::Jump(1)], 0, 1),
        target(
            vec![
                Op::Label(0),
                Op::Label(0),
                Op::Constant(qjs_scalar_ssa_ir::Scalar::Number(0.0)),
                Op::Return,
            ],
            0,
            1,
        ),
        target(vec![Op::GetLocal(0), Op::Return], 0, 0),
        target(vec![Op::Return], 0, 0),
        target(
            vec![
                Op::Constant(qjs_scalar_ssa_ir::Scalar::Bool(true)),
                Op::IfFalse(0),
                Op::Constant(qjs_scalar_ssa_ir::Scalar::Number(1.0)),
                Op::Label(0),
                Op::Constant(qjs_scalar_ssa_ir::Scalar::Number(2.0)),
                Op::Return,
            ],
            0,
            1,
        ),
    ];
    for t in bads {
        assert!(emit_target(&t, &metadata()).is_err());
    }
}
#[test]
fn branch_mutations_finalize_without_original_passes() {
    for source in [
        "function main(){let x=1;if(true){x=42;}else{x=7;}return x;}",
        "function main(){let x=1;if(false){x=42;}return x;}",
        "function main(){if(true){return 42;}else{return 7;}}",
    ] {
        for optimize in [false, true] {
            compile(source, "branches.js", &CompileOptions { optimize })
                .unwrap()
                .finalize()
                .unwrap();
        }
    }
}

#[test]
fn full_width_local_operands_do_not_truncate_at_255() {
    let t = target(
        vec![
            Op::Constant(qjs_scalar_ssa_ir::Scalar::Number(42.0)),
            Op::PutLocal(299),
            Op::GetLocal(299),
            Op::Return,
        ],
        300,
        0,
    );
    let final_code = emit_target(&t, &metadata()).unwrap();
    let decoded = qjs_bytecode_image_verifier::decode_schema(
        &final_code.code,
        qjs_bytecode_image_verifier::Limits::default(),
        qjs_bytecode_image_verifier::production::FINAL_OPCODES,
    )
    .unwrap();
    for instruction in &decoded.instructions()[1..3] {
        assert_eq!(
            u16::from_ne_bytes(instruction.operands().try_into().unwrap()),
            299
        );
    }
    let unit = CompilationUnit {
        target: t,
        metadata: metadata(),
    };
    unit.finalize().unwrap();
}
