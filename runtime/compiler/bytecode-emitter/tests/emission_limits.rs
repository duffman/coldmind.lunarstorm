//! Emission limits at their boundaries: IR nodes, functions, locals, atoms, code size and
//! temporaries; a caller cannot relax hard limits.

use qjs_bytecode_emitter::{EmissionLimits, JsEmitOptions, emit_program_with_limits};
fn check(source: &str, count: usize, set: impl Fn(&mut EmissionLimits, usize), category: &str) {
    let program =
        qjs_js_parser_frontend::compile_program(source, "budget.js", qjs_js_parser_frontend::SourceMode::Script)
            .unwrap();
    for n in [count - 1, count, count + 1] {
        let mut limits = EmissionLimits::default();
        set(&mut limits, n);
        let result = emit_program_with_limits(&program, &JsEmitOptions::default(), &limits);
        if n < count {
            assert!(
                result
                    .as_ref()
                    .is_err_and(|e| e.starts_with("capacity:") && e.contains(category)),
                "{category} limit{n}: {result:?}"
            )
        } else {
            assert!(result.is_ok(), "{category} limit{n}: {result:?}")
        }
    }
}
#[test]
fn ir_node_boundaries() {
    check("1;", 5, |l, n| l.max_ir_nodes = n, "IR nodes")
}
#[test]
fn function_boundaries() {
    check(
        "function f(){return 1;}f();",
        2,
        |l, n| l.max_functions = n,
        "functions",
    )
}
#[test]
fn local_boundaries() {
    // The script entry needs its one local; without it the entry representation check refuses
    // the program before the locals budget is reached.
    check(
        "1;",
        1,
        |l, n| l.max_locals = n,
        "script entry representation",
    )
}
#[test]
fn atom_boundaries() {
    check("let key=1;key;", 1, |l, n| l.max_atoms = n, "atoms")
}
#[test]
fn code_and_location_boundaries() {
    let image = qjs_bytecode_emitter::compile_script("1+2;", "budget.js").unwrap();
    check(
        "1+2;",
        image.function().code.len(),
        |l, n| l.max_code_bytes = n,
        "code bytes",
    );
    check(
        "1+2;",
        image.function().debug_locations.len(),
        |l, n| l.max_positions = n,
        "positions",
    );
}
#[test]
fn caller_cannot_relax_hard_profile() {
    let mut l = EmissionLimits::default();
    l.max_code_bytes += 1;
    assert!(l.validate().unwrap_err().starts_with("capacity:"));
}
#[test]
fn source_nesting_instruction_constant_and_atom_unit_boundaries() {
    check("1;", 2, |l, n| l.max_source_units = n, "source units");
    check("1;", 2, |l, n| l.max_nesting = n, "nesting");
    check("1;", 1, |l, n| l.max_constants = n, "constants");
    check("1;", 21, |l, n| l.max_atom_units = n, "atom units");
    let image = qjs_bytecode_emitter::compile_script("1+2;", "budget.js").unwrap();
    check(
        "1+2;",
        image.function().debug_locations.len(),
        |l, n| l.max_instructions = n,
        "instructions",
    );
}
#[test]
fn generated_temporary_slots_and_nested_functions_share_admission() {
    let source = "function f(){let o={};o.x=1;return o.x;}f();";
    let program =
        qjs_js_parser_frontend::compile_program(source, "budget.js", qjs_js_parser_frontend::SourceMode::Script)
            .unwrap();
    let limits = EmissionLimits {
        max_locals: 1,
        ..Default::default()
    };
    assert!(
        emit_program_with_limits(&program, &JsEmitOptions::default(), &limits)
            .unwrap_err()
            .starts_with("capacity:")
    );
    let limits = EmissionLimits {
        max_instructions: 5,
        ..Default::default()
    };
    assert!(
        emit_program_with_limits(&program, &JsEmitOptions::default(), &limits)
            .unwrap_err()
            .contains("instructions")
    );
}
#[test]
fn requested_image_policy_applies_to_script_and_module_before_verification_work() {
    for (source, mode) in [
        ("1+2;", qjs_js_parser_frontend::SourceMode::Script),
        ("export const value=42;", qjs_js_parser_frontend::SourceMode::Module),
    ] {
        let program = qjs_js_parser_frontend::compile_program(source, "budget.js", mode).unwrap();
        let image_limits = qjs_bytecode_image_verifier::image::ImageLimits {
            max_work: 0,
            ..Default::default()
        };
        let error = qjs_bytecode_emitter::emit_program_with_policies(
            &program,
            &JsEmitOptions::default(),
            &EmissionLimits::default(),
            &image_limits,
        )
        .unwrap_err();
        assert!(error.starts_with("capacity:"), "{error}");
    }
}
#[test]
fn scalar_native_boundaries_are_checked_before_output_allocation() {
    let unit = qjs_bytecode_emitter::compile(
        "function main(){return 42;}",
        "budget.js",
        &qjs_bytecode_emitter::CompileOptions::default(),
    )
    .unwrap();
    let final_code = qjs_bytecode_emitter::emit_target(&unit.target, &unit.metadata).unwrap();
    for n in [
        final_code.code.len() - 1,
        final_code.code.len(),
        final_code.code.len() + 1,
    ] {
        let limits = EmissionLimits {
            max_code_bytes: n,
            ..Default::default()
        };
        assert_eq!(
            qjs_bytecode_emitter::emit_target_with_limits(&unit.target, &unit.metadata, &limits)
                .is_ok(),
            n >= final_code.code.len()
        );
    }
}
#[test]
fn scalar_lower_forwards_requested_validation_work() {
    let function = qjs_js_parser_frontend::compile("function main(){return 42;}", "budget.js").unwrap();
    let needed = (1..1000)
        .find(|work| {
            qjs_scalar_ssa_ir::validate_with_limits(
                &function,
                &qjs_scalar_ssa_ir::ValidationLimits {
                    max_work: *work,
                    ..Default::default()
                },
            )
            .is_ok()
        })
        .expect("small scalar validation");
    let limits = EmissionLimits {
        max_work: needed - 1,
        ..Default::default()
    };
    let error = qjs_bytecode_emitter::lower_with_limits(&function, &limits).unwrap_err();
    assert!(
        error.starts_with("capacity: scalar IR validation"),
        "{error}"
    );
}
