use qjs_bytecode_image_verifier::{ids, validate_structure, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // false ? 1 : 42, using header-generated identities. if_false8's
    // displacement is based at byte 2 (its operand), so +4 targets byte 6.
    let mut bytes = vec![
        ids::push_false,
        ids::if_false8,
        4,
        ids::push_1,
        ids::goto8,
        6,
        ids::push_i32,
    ];
    bytes.extend_from_slice(&42_i32.to_ne_bytes());
    bytes.push(ids::r#return);
    let decoded = validate_structure(&bytes, Limits::default())?;
    println!("Final QuickJS-NG instructions (structural checks only):");
    for instruction in decoded.instructions() {
        let opcode = instruction.opcode();
        print!(
            "{:04} {:20} {:?}",
            instruction.offset(),
            opcode.name,
            instruction.operands()
        );
        if let Some(target) = instruction.branch_target() {
            print!(" -> {target}");
        }
        println!();
    }
    Ok(())
}
