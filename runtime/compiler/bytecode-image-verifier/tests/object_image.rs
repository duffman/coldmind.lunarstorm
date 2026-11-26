//! Image verification of object literals: method definitions, flags and atoms, spread
//! and computed data properties.

use qjs_bytecode_image_verifier::{image::*, production::ids};

fn method(computed: bool, flags: u8) -> FunctionImage {
    let mut code = vec![ids::object];
    if computed {
        code.extend([ids::push_atom_value, 0, 0, 0, 0]);
    }
    code.extend([ids::fclosure8, 0]);
    if computed {
        code.extend([ids::define_method_computed, flags]);
    } else {
        code.extend([ids::define_method, 0, 0, 0, 0, flags]);
    }
    code.push(ids::r#return);
    FunctionImage {
        code,
        atoms: vec!["method".encode_utf16().collect()],
        constants: vec![Constant::Function(Box::new(FunctionImage {
            code: vec![ids::return_undef],
            ..Default::default()
        }))],
        stack_size: if computed { 3 } else { 2 },
        ..Default::default()
    }
}
fn spread(mask: u8) -> FunctionImage {
    FunctionImage {
        code: vec![
            ids::object,
            ids::object,
            ids::null,
            ids::copy_data_properties,
            mask,
            ids::drop,
            ids::drop,
            ids::r#return,
        ],
        stack_size: 3,
        ..Default::default()
    }
}
#[test]
fn enumerable_object_methods_and_accessors_have_exact_stack_effects() {
    for computed in [false, true] {
        for flags in [4, 5, 6] {
            let f = method(computed, flags);
            assert_eq!(
                decode_and_verify(&encode(&f).unwrap()).unwrap().function(),
                &f
            );
        }
    }
}
#[test]
fn method_flags_and_atoms_do_not_admit_unmodeled_class_definitions() {
    for computed in [false, true] {
        for flags in [0, 1, 2, 3, 7, 8, 12, 255] {
            assert!(encode(&method(computed, flags)).is_err());
        }
    }
    let mut f = method(false, 4);
    f.code[4] = 1;
    assert!(encode(&f).unwrap_err().contains("atom"));
    f = method(false, 4);
    f.code.truncate(8);
    assert!(encode(&f).is_err());
    f = method(false, 4);
    if let Constant::Function(child) = &mut f.constants[0] {
        child.flags.need_home_object = true;
    }
    // A method using `super` needs its home object; such methods are admitted.
    decode_and_verify(&encode(&f).unwrap()).unwrap();
}
#[test]
fn method_underflows_and_incorrect_declared_stack_are_rejected() {
    for computed in [false, true] {
        let mut f = method(computed, 4);
        f.code.remove(0);
        assert!(encode(&f).unwrap_err().contains("underflow"));
        let mut f = method(computed, 4);
        f.stack_size -= 1;
        assert!(encode(&f).unwrap_err().contains("stack"));
    }
}
#[test]
fn object_spread_accepts_only_the_proven_three_slot_selector() {
    let f = spread(6);
    assert_eq!(
        decode_and_verify(&encode(&f).unwrap()).unwrap().function(),
        &f
    );
    for mask in 0..=255 {
        if mask != 6 {
            assert!(encode(&spread(mask)).is_err(), "mask {mask}");
        }
    }
    let mut f = spread(6);
    f.code.remove(0);
    assert!(encode(&f).unwrap_err().contains("underflow"));
    let mut f = spread(6);
    f.code.remove(5);
    assert!(encode(&f).unwrap_err().contains("stack"));
}
#[test]
fn computed_data_definition_retains_key_and_prototype_assignment_consumes_value() {
    let f = FunctionImage {
        code: vec![
            ids::object,
            ids::push_atom_value,
            0,
            0,
            0,
            0,
            ids::push_1,
            ids::define_array_el,
            ids::drop,
            ids::null,
            ids::set_proto,
            ids::r#return,
        ],
        atoms: vec!["key".encode_utf16().collect()],
        stack_size: 3,
        ..Default::default()
    };
    decode_and_verify(&encode(&f).unwrap()).unwrap();
    let mut missing_drop = f.clone();
    missing_drop.code.remove(8);
    assert!(encode(&missing_drop).is_err());
    for opcode in [ids::define_array_el, ids::set_proto] {
        let mut malformed = f.clone();
        malformed.code = vec![ids::object, opcode, ids::r#return];
        assert!(encode(&malformed).unwrap_err().contains("underflow"));
    }
}
