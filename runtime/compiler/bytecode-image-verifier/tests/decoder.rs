//! The bytecode decoder: operand widths of every opcode, limits applied before unbounded
//! work, and jump targets that must stay inside the function.

use qjs_bytecode_image_verifier::{decode, ids, validate_structure, Error, Limits, OperandFormat, FINAL_OPCODES};

#[test]
fn decodes_straight_line_operands_without_copying() {
    let bytes = [
        ids::push_i32,
        42,
        0,
        0,
        0,
        ids::push_1,
        ids::add,
        ids::r#return,
    ];
    let decoded = decode(&bytes, Limits::default()).unwrap();
    let instructions = decoded.instructions();
    assert_eq!(instructions.len(), 4);
    assert_eq!(instructions[0].offset(), 0);
    assert_eq!(instructions[0].opcode().name, "push_i32");
    assert_eq!(instructions[0].operands(), &[42, 0, 0, 0]);
    assert_eq!(instructions[1].offset(), 5);
    assert_eq!(instructions[3].offset(), 7);
    assert!(std::ptr::eq(
        instructions[0].operands().as_ptr(),
        bytes[1..].as_ptr()
    ));
    validate_structure(&bytes, Limits::default()).unwrap();
}

#[test]
fn decodes_exact_width_of_every_final_opcode() {
    for opcode in FINAL_OPCODES {
        let mut bytes = vec![0; usize::from(opcode.width)];
        bytes[0] = opcode.id;
        let decoded = decode(&bytes, Limits::default()).unwrap();
        assert_eq!(decoded.instructions().len(), 1, "{}", opcode.name);
        assert_eq!(
            decoded.instructions()[0].bytes().len(),
            usize::from(opcode.width)
        );
        for length in 1..usize::from(opcode.width) {
            assert!(
                matches!(
                    decode(&bytes[..length], Limits::default()),
                    Err(Error::TruncatedInstruction { .. })
                ),
                "{} at length {length}",
                opcode.name
            );
        }
    }
}

#[test]
fn rejects_invalid_and_unknown_ids() {
    assert!(matches!(
        decode(&[ids::invalid], Limits::default()),
        Err(Error::InvalidOpcode {
            offset: 0,
            opcode: 0
        })
    ));
    let unknown = (1..=u8::MAX)
        .find(|id| !FINAL_OPCODES.iter().any(|op| op.id == *id))
        .unwrap();
    assert!(matches!(decode(&[ids::nop, unknown], Limits::default()),
        Err(Error::UnknownOpcode { offset: 1, opcode }) if opcode == unknown));
}

#[test]
fn applies_limits_before_unbounded_decoding() {
    assert!(matches!(
        decode(
            &[ids::nop; 3],
            Limits {
                max_bytes: 2,
                max_instructions: 3
            }
        ),
        Err(Error::ByteLimitExceeded {
            length: 3,
            maximum: 2
        })
    ));
    assert!(matches!(
        decode(
            &[ids::nop; 3],
            Limits {
                max_bytes: 3,
                max_instructions: 2
            }
        ),
        Err(Error::InstructionLimitExceeded {
            offset: 2,
            maximum: 2
        })
    ));
    assert!(decode(
        &[],
        Limits {
            max_bytes: 0,
            max_instructions: 0
        }
    )
    .unwrap()
    .instructions()
    .is_empty());
}

#[test]
fn validates_short_and_long_forward_backward_branches_from_operand_address() {
    let mut cases = vec![
        vec![ids::goto8, 1, ids::nop],
        vec![ids::nop, ids::goto8, 254],
        vec![ids::goto8, 255], // -1 from operand byte reaches opcode zero.
    ];
    cases.push([&[ids::goto16][..], &2_i16.to_ne_bytes(), &[ids::nop]].concat());
    cases.push([&[ids::nop, ids::goto16][..], &(-2_i16).to_ne_bytes()].concat());
    cases.push([&[ids::goto][..], &4_i32.to_ne_bytes(), &[ids::nop]].concat());
    cases.push([&[ids::nop, ids::goto][..], &(-2_i32).to_ne_bytes()].concat());
    for bytes in cases {
        validate_structure(&bytes, Limits::default()).unwrap();
    }
}

#[test]
fn validates_every_final_target_bearing_opcode_with_real_relative_base() {
    for opcode in FINAL_OPCODES {
        let (operand_offset, displacement_width) = match opcode.format {
            OperandFormat::Label8 => (1, 1),
            OperandFormat::Label16 => (1, 2),
            OperandFormat::Label | OperandFormat::LabelU16 => (1, 4),
            OperandFormat::AtomLabelU8 | OperandFormat::AtomLabelU16 => (5, 4),
            _ => continue,
        };
        let mut bytes = vec![0; usize::from(opcode.width) + 1];
        bytes[0] = opcode.id;
        let last = bytes.len() - 1;
        bytes[last] = ids::nop;
        let displacement = (usize::from(opcode.width) - operand_offset) as i32;
        let encoded = match displacement_width {
            1 => vec![displacement as i8 as u8],
            2 => (displacement as i16).to_ne_bytes().to_vec(),
            _ => displacement.to_ne_bytes().to_vec(),
        };
        bytes[operand_offset..operand_offset + displacement_width].copy_from_slice(&encoded);
        let decoded = validate_structure(&bytes, Limits::default()).unwrap();
        assert_eq!(
            decoded.instructions()[0].branch_target(),
            Some(last as i128),
            "{}",
            opcode.name
        );

        let backwards = -(operand_offset as i32);
        let encoded = match displacement_width {
            1 => vec![backwards as i8 as u8],
            2 => (backwards as i16).to_ne_bytes().to_vec(),
            _ => backwards.to_ne_bytes().to_vec(),
        };
        bytes[operand_offset..operand_offset + displacement_width].copy_from_slice(&encoded);
        validate_structure(&bytes, Limits::default()).unwrap();

        bytes[operand_offset..operand_offset + displacement_width].fill(0);
        assert!(
            matches!(validate_structure(&bytes, Limits::default()),
            Err(Error::BranchNotInstructionBoundary { target, .. }) if target == operand_offset),
            "{}",
            opcode.name
        );
    }
}

#[test]
fn rejects_targets_below_zero_at_buffer_end_and_above_buffer_end() {
    for bytes in [
        vec![ids::goto8, 254],
        vec![ids::goto8, 1],
        vec![ids::goto8, 127],
        [&[ids::goto][..], &i32::MIN.to_ne_bytes()].concat(),
        [&[ids::goto][..], &i32::MAX.to_ne_bytes()].concat(),
    ] {
        assert!(matches!(
            validate_structure(&bytes, Limits::default()),
            Err(Error::BranchOutOfRange { .. })
        ));
    }
}

#[test]
fn rejects_targets_in_another_instructions_operands() {
    let bytes = [ids::goto8, 2, ids::push_i32, 0, 0, 0, 0, ids::r#return];
    assert!(matches!(
        validate_structure(&bytes, Limits::default()),
        Err(Error::BranchNotInstructionBoundary {
            offset: 0,
            target: 3
        })
    ));
}

#[test]
fn final_short_opcodes_use_final_metadata_despite_temporary_numeric_overlap() {
    assert_eq!(ids::push_minus1, qjs_bytecode_image_verifier::TEMPORARY_OPCODE_START);
    let decoded = decode(&[ids::push_minus1], Limits::default()).unwrap();
    assert_eq!(decoded.instructions()[0].opcode().name, "push_minus1");
    assert!(!FINAL_OPCODES
        .iter()
        .any(|op| op.name == "enter_scope" || op.name == "source_loc"));
}

#[test]
fn metadata_exposes_declared_stack_effect_without_claiming_semantic_verification() {
    let call = FINAL_OPCODES.iter().find(|op| op.id == ids::call).unwrap();
    assert_eq!(
        (call.pop, call.push, call.format),
        (1, 1, OperandFormat::Npop)
    );
    // Structural validation deliberately accepts a stack-underflowing instruction.
    validate_structure(&[ids::drop], Limits::default()).unwrap();
}
