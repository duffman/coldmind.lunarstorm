//! Native function images: round trips, truncation and checksums, nested functions,
//! scope links, BigInt constants and module records.

use qjs_bytecode_image_verifier::{image::*, production::ids};
fn function() -> FunctionImage {
    FunctionImage {
        code: vec![ids::push_const8, 0, ids::r#return],
        constants: vec![Constant::Number(-0.0)],
        local_count: 0,
        arg_count: 0,
        defined_arg_count: 0,
        stack_size: 1,
        strict: true,
        function_name: "main".into(),
        filename: "😀.js".into(),
        debug_locations: vec![
            DebugLocation {
                code_offset: 0,
                line: 1,
                column: 11,
            },
            DebugLocation {
                code_offset: 2,
                line: 10,
                column: 1,
            },
        ],
        ..Default::default()
    }
}
#[test]
fn rust_image_roundtrip_preserves_bits_positions_and_native_version() {
    let f = function();
    let bytes = encode(&f).unwrap();
    assert_eq!(bytes[0], 94); // lunarstorm patch: native format version 94 (user-catch table)
    let i = decode_and_verify(&bytes).unwrap();
    assert_eq!(i.function().code, f.code);
    assert_eq!(i.function().debug_locations, f.debug_locations);
    match i.function().constants[0] {
        Constant::Number(n) => assert_eq!(n.to_bits(), (-0.0f64).to_bits()),
        _ => panic!(),
    };
    assert_eq!(i.function().filename, f.filename);
}
#[test]
fn truncated_and_checksum_covered_corruption_is_rejected_without_claiming_raw_authenticity() {
    let bytes = encode(&function()).unwrap();
    for end in 0..bytes.len() {
        assert!(decode_and_verify(&bytes[..end]).is_err());
    }
    for n in 0..bytes.len() {
        let mut bad = bytes.clone();
        bad[n] ^= 0x40;
        let uncovered = (bytes.len() - 5) % 4 == 0 && n >= bytes.len() - 4;
        if !uncovered {
            assert!(decode_and_verify(&bad).is_err(), "covered byte {n}");
        } else if let Ok(image) = decode_and_verify(&bad) {
            // The pinned checksum ignores an exact four-byte tail. A valid
            // changed location is untrusted native data, not Rust provenance.
            assert_eq!(image.profile(), NATIVE_PROFILE);
            assert_ne!(image.digest(), decode_and_verify(&bytes).unwrap().digest());
        }
    }
}
#[test]
fn writer_rejects_invalid_cfg_and_position_overflow() {
    let mut f = function();
    f.code = vec![ids::get_loc0, ids::r#return];
    assert!(encode(&f).is_err());
    let mut f = function();
    f.debug_locations[0].line = u32::MAX;
    assert!(encode(&f).is_err());
    let mut f = function();
    f.debug_locations.reverse();
    assert!(encode(&f).is_err());
}

#[test]
fn encoded_checksum_matches_pinned_native_routine_at_all_alignments() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::temp_dir().join(format!("qjs-checksum-oracle-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let exe = dir.join("probe");
    let vendor = root.join("../vendor/quickjs-ng");
    let output = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()))
        .args(["-std=c11", "-D_GNU_SOURCE", "-DQJS_DISABLE_PARSER", "-I"])
        .arg(&vendor)
        .arg(root.join("tools/checksum_probe.c"))
        .args([
            vendor.join("dtoa.c"),
            vendor.join("libregexp.c"),
            vendor.join("libunicode.c"),
        ])
        .args(["-lm", "-pthread", "-o"])
        .arg(&exe)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let oracle = |bytes: &[u8]| {
        let mut child = Command::new(&exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap()
    };
    assert_eq!(oracle(&[1, 2, 3, 4]), 0);
    assert_eq!(oracle(&[1, 2, 3, 4, 5, 6, 7, 8]), 477168129);
    assert_eq!(oracle(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]), 4074047494);
    let mut alignments = std::collections::BTreeSet::new();
    for n in 0..12 {
        let mut f = function();
        f.filename = "x".repeat(n);
        if n % 2 == 0 {
            f.constants = vec![Constant::Bool(true)];
        }
        let bytes = encode(&f).unwrap();
        alignments.insert((bytes.len() - 5) % 4);
        let header = u32::from_le_bytes(bytes[1..5].try_into().unwrap());
        assert_eq!(
            header,
            oracle(&bytes[5..]),
            "payload len {}",
            bytes.len() - 5
        );
    }
    // Wide atom spellings move by two bytes; a compact integer instruction moves by one.
    for n in 0..4 {
        let mut f = function();
        f.filename = "x".repeat(n);
        f.code = vec![ids::push_1, ids::r#return];
        f.debug_locations = vec![DebugLocation {
            code_offset: 0,
            line: 1,
            column: 1,
        }];
        let bytes = encode(&f).unwrap();
        alignments.insert((bytes.len() - 5) % 4);
        assert_eq!(
            u32::from_le_bytes(bytes[1..5].try_into().unwrap()),
            oracle(&bytes[5..])
        );
    }
    assert_eq!(alignments.len(), 4);
    let mut property = general(
        vec![
            ids::object,
            ids::push_i8,
            42,
            ids::define_field,
            0,
            0,
            0,
            0,
            ids::get_field,
            0,
            0,
            0,
            0,
            ids::r#return,
        ],
        2,
    );
    property.atoms = vec!["answer".encode_utf16().collect()];
    let mut global = general(
        vec![
            ids::define_var,
            0,
            0,
            0,
            0,
            2,
            ids::push_i8,
            42,
            ids::put_var,
            0,
            0,
            0,
            0,
            ids::get_var,
            0,
            0,
            0,
            0,
            ids::r#return,
        ],
        1,
    );
    global.atoms = vec!["answer".encode_utf16().collect()];
    let mut string = general(vec![ids::push_const8, 0, ids::r#return], 1);
    string.constants = vec![Constant::String("hello😀".encode_utf16().collect())];
    let mut native_cases = vec![
        (nested_fixture(), "42"),
        (property, "42"),
        (global, "42"),
        (string, "hello😀"),
        (general(vec![ids::undefined, ids::r#return], 1), "undefined"),
        (general(vec![ids::null, ids::r#return], 1), "null"),
        (
            general(
                vec![
                    ids::special_object,
                    2,
                    ids::special_object,
                    2,
                    ids::strict_eq,
                    ids::r#return,
                ],
                2,
            ),
            "true",
        ),
    ];
    for (bytes, expected) in bigint_cases() {
        let mut f = general(vec![ids::push_const8, 0, ids::r#return], 1);
        f.constants = vec![Constant::BigInt(bytes)];
        native_cases.push((f, expected));
    }
    let module_bytes = encode_module(&module_fixture()).unwrap();
    decode_and_verify(&module_bytes).unwrap();
    let mut child = Command::new(&exe)
        .arg("module")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&module_bytes)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "module native load/link/evaluate: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "42");
    let mut dep = module_fixture();
    dep.name = "dependency.mjs".encode_utf16().collect();
    dep.function.code[8] = 41;
    let dep_bytes = encode_module(&dep).unwrap();
    let mut main = module_fixture();
    main.required_modules = vec![dep.name.clone()];
    main.function.captures.push(Capture {
        name: "imported".encode_utf16().collect(),
        closure_type: 7,
        is_const: true,
        is_lexical: true,
        ..Default::default()
    });
    main.imports = vec![ModuleImport {
        capture: 1,
        name: "answer".encode_utf16().collect(),
        module: 0,
    }];
    main.exports.push(ModuleExport::Indirect {
        module: 0,
        import: "answer".encode_utf16().collect(),
        name: "forwarded".encode_utf16().collect(),
    });
    main.star_exports = vec![0];
    main.function.code = vec![
        ids::push_this,
        ids::if_false,
        5,
        0,
        0,
        0,
        ids::return_undef,
        ids::get_var_ref_check,
        1,
        0,
        ids::push_1,
        ids::add,
        ids::put_var_ref_check_init,
        0,
        0,
        ids::undefined,
        ids::return_async,
    ];
    main.function.stack_size = 2;
    for namespace in [false, true] {
        if namespace {
            main.imports[0].name = vec![b'*' as u16];
            main.function.captures[1].closure_type = 6;
            main.function.atoms = vec!["answer".encode_utf16().collect()];
            main.function
                .code
                .splice(10..10, [ids::get_field, 0, 0, 0, 0]);
        }
        let main_bytes = encode_module(&main).unwrap();
        decode_and_verify(&main_bytes).unwrap();
        let mut input = (dep_bytes.len() as u32).to_le_bytes().to_vec();
        input.extend(&dep_bytes);
        input.extend(main_bytes);
        let mut child = Command::new(&exe)
            .arg("graph")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "module import/link/evaluate: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "42 41");
    }
    for (root, eval) in eval_native_fixtures() {
        let root = encode(&root).unwrap();
        let mut input = (root.len() as u32).to_le_bytes().to_vec();
        input.extend(root);
        input.extend(eval.bytes());
        let mut child = Command::new(&exe)
            .arg("eval")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "native external capture: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "143");
    }
    let marker_function = FunctionImage {
        code: vec![
            ids::set_loc_uninitialized,
            0,
            0,
            ids::push_i8,
            42,
            ids::put_loc_check_init,
            0,
            0,
            ids::debug_stmt,
            1,
            0,
            0,
            0,
            ids::get_loc_check,
            0,
            0,
            ids::r#return,
        ],
        local_count: 1,
        var_ref_count: 1,
        stack_size: 1,
        locals: vec![VarDef {
            name: vec![b'x' as u16],
            is_captured: true,
            is_lexical: true,
            scope_level: 1,
            ..Default::default()
        }],
        debug_locations: vec![
            DebugLocation {
                code_offset: 0,
                line: 4,
                column: 8,
            },
            DebugLocation {
                code_offset: 8,
                line: 4,
                column: 8,
            },
        ],
        ..Default::default()
    };
    let bytes = encode(&marker_function).unwrap();
    decode_and_verify(&bytes).unwrap();
    let mut child = Command::new(&exe)
        .arg("marker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "native debug marker scope/position: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "42 1");
    for (f, expected) in arguments_native_fixtures() {
        native_cases.push((f, expected));
    }
    let mut strict_callee = arguments_child(
        true,
        false,
        vec![
            ids::get_loc,
            0,
            0,
            ids::get_field,
            0,
            0,
            0,
            0,
            ids::r#return,
        ],
    );
    strict_callee.atoms = vec!["callee".encode_utf16().collect()];
    let bytes = encode(&call_arguments_child(strict_callee, false)).unwrap();
    decode_and_verify(&bytes).unwrap();
    let mut child = Command::new(&exe)
        .arg("run")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("TypeError"));
    for (f, expected) in native_cases {
        let bytes = encode(&f).unwrap();
        decode_and_verify(&bytes).unwrap();
        let mut child = Command::new(&exe)
            .arg("run")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "native load/run: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), expected);
    }
}

fn general(code: Vec<u8>, stack_size: u32) -> FunctionImage {
    FunctionImage {
        code,
        stack_size,
        function_name: String::new(),
        filename: "general.js".into(),
        ..Default::default()
    }
}
fn nested_fixture() -> FunctionImage {
    let mut child = general(
        vec![
            ids::get_var_ref_check,
            0,
            0,
            ids::get_arg,
            0,
            0,
            ids::add,
            ids::r#return,
        ],
        2,
    );
    child.function_name = "addCaptured".into();
    child.arg_count = 1;
    child.defined_arg_count = 1;
    child.locals = vec![VarDef {
        name: vec![b'a' as u16],
        ..Default::default()
    }];
    child.captures = vec![Capture {
        name: vec![b'x' as u16],
        closure_type: 0,
        is_lexical: true,
        ..Default::default()
    }];
    child.source = Some(
        "function addCaptured(a) { return x + a; }"
            .encode_utf16()
            .collect(),
    );
    let mut f = general(
        vec![
            ids::set_loc_uninitialized,
            0,
            0,
            ids::push_i8,
            40,
            ids::put_loc_check_init,
            0,
            0,
            ids::fclosure8,
            0,
            ids::push_2,
            ids::call,
            1,
            0,
            ids::r#return,
        ],
        2,
    );
    f.local_count = 1;
    f.var_ref_count = 1;
    f.locals = vec![VarDef {
        name: vec![b'x' as u16],
        is_lexical: true,
        is_captured: true,
        ..Default::default()
    }];
    f.constants = vec![Constant::Function(Box::new(child))];
    f
}
#[test]
fn recursive_images_preserve_lexical_metadata_strings_and_wtf8_source() {
    let mut f = nested_fixture();
    f.source = Some(vec![0x61, 0xd800, 0x62, 0xdc00, 0xd83d, 0xde00]);
    f.constants.push(Constant::String(vec![0xd800, 0, 0xdc00]));
    let image = decode_and_verify(&encode(&f).unwrap()).unwrap();
    assert_eq!(image.function().source, f.source);
    assert_eq!(image.function().constants, f.constants);
    assert_eq!(image.function().locals, f.locals);
    assert_eq!(image.digest().len(), 32);
    assert_eq!(
        image.target_fingerprint(),
        qjs_bytecode_image_verifier::production::TARGET_FINGERPRINT
    );
}
#[test]
fn malformed_nested_metadata_and_unsupported_states_are_rejected() {
    let mut f = nested_fixture();
    if let Constant::Function(child) = &mut f.constants[0] {
        child.captures[0].var_idx = 1;
    }
    assert!(encode(&f).is_err());
    let mut f = nested_fixture();
    f.locals.clear();
    assert!(encode(&f).is_err());
    let mut f = nested_fixture();
    f.var_ref_count = 0;
    assert!(encode(&f).is_err());
    let mut f = nested_fixture();
    if let Constant::Function(child) = &mut f.constants[0] {
        child.captures[0].closure_type = 7;
    }
    assert!(encode(&f).is_err());
    let mut f = nested_fixture();
    if let Constant::Function(child) = &mut f.constants[0] {
        child.code = vec![ids::get_var_ref, 0, 0, ids::r#return];
    }
    assert!(encode(&f).is_err());
    let mut f = nested_fixture();
    if let Constant::Function(child) = &mut f.constants[0] {
        child.flags.kind = 1;
    }
    assert!(encode(&f).is_err());
    let mut f = general(vec![ids::fclosure8, 0, ids::r#return], 1);
    f.constants = vec![Constant::Number(1.)];
    assert!(encode(&f).is_err());
    for code in [
        vec![ids::get_arg, 0, 0, ids::r#return],
        vec![ids::get_var, 0, 0, 0, 0, ids::r#return],
        vec![ids::close_loc, 0, 0, ids::return_undef],
        vec![ids::call, 1, 0, ids::r#return],
        vec![ids::r#yield, ids::r#return],
    ] {
        assert!(encode(&general(code, 2)).is_err());
    }
}

#[test]
fn nullish_test_has_independent_stack_semantics() {
    for value in [ids::undefined, ids::null, ids::push_0] {
        let f = general(vec![value, ids::is_undefined_or_null, ids::r#return], 1);
        decode_and_verify(&encode(&f).unwrap()).unwrap();
    }
    assert!(encode(&general(vec![ids::is_undefined_or_null, ids::r#return], 1)).is_err());
}

#[test]
fn scope_links_are_local_indices_after_argument_prefix() {
    let mut f = general(vec![ids::return_undef], 0);
    f.arg_count = 1;
    f.defined_arg_count = 1;
    f.local_count = 1;
    f.locals = vec![
        VarDef::default(),
        VarDef {
            scope_next: ScopeLink::Local(0),
            ..Default::default()
        },
    ];
    assert!(
        encode(&f).is_err(),
        "local zero links to itself, not argument zero"
    );
    f.locals[1].scope_next = ScopeLink::Local(1);
    assert!(encode(&f).is_err(), "scope target is outside local slots");
    f.locals[1].scope_next = ScopeLink::End;
    decode_and_verify(&encode(&f).unwrap()).unwrap();
}

#[test]
fn bigint_constants_require_canonical_signed_bytes() {
    for bad in [vec![0], vec![1, 0], vec![0xff, 0xff], vec![0x80, 0xff]] {
        let mut f = general(vec![ids::push_const8, 0, ids::r#return], 1);
        f.constants = vec![Constant::BigInt(bad)];
        assert!(
            encode(&f).is_err(),
            "redundant signed bytes must not obtain a proof"
        );
    }
}

fn bigint_cases() -> Vec<(Vec<u8>, &'static str)> {
    vec![
        (vec![], "0"),
        (vec![1], "1"),
        (vec![127], "127"),
        (vec![128, 0], "128"),
        (vec![255, 0], "255"),
        (vec![255], "-1"),
        (vec![128], "-128"),
        (vec![127, 255], "-129"),
        (vec![0, 255], "-256"),
        (vec![0, 0, 0, 128], "-2147483648"),
        (vec![255, 255, 255, 127], "2147483647"),
        (vec![0, 0, 0, 0, 1], "4294967296"),
        (vec![0, 0, 0, 0, 0, 0, 0, 128, 0], "9223372036854775808"),
        (vec![0, 0, 0, 0, 0, 0, 0, 0, 1], "18446744073709551616"),
    ]
}
#[test]
fn bigint_values_roundtrip_and_special_this_function_is_bounded() {
    for (bytes, _) in bigint_cases() {
        let mut f = general(vec![ids::push_const8, 0, ids::r#return], 1);
        f.constants = vec![Constant::BigInt(bytes)];
        assert_eq!(
            decode_and_verify(&encode(&f).unwrap())
                .unwrap()
                .function()
                .constants,
            f.constants
        );
    }
    for value in [0, 1, 3, 4, 7, 255] {
        assert!(encode(&general(vec![ids::special_object, value, ids::r#return], 1)).is_err());
    }
    // var_object (sloppy direct-eval scopes) and import.meta need no owner metadata.
    for value in [5, 6] {
        encode(&general(vec![ids::special_object, value, ids::r#return], 1)).unwrap();
    }
    decode_and_verify(&encode(&general(vec![ids::special_object, 2, ids::r#return], 1)).unwrap())
        .unwrap();
}

fn module_fixture() -> ModuleImage {
    let mut f = general(
        vec![
            ids::push_this,
            ids::if_false,
            5,
            0,
            0,
            0,
            ids::return_undef,
            ids::push_i8,
            42,
            ids::put_var_ref_check_init,
            0,
            0,
            ids::undefined,
            ids::return_async,
        ],
        1,
    );
    f.strict = true;
    f.flags.kind = 2;
    f.captures = vec![Capture {
        name: "answer".encode_utf16().collect(),
        closure_type: 6,
        is_lexical: true,
        ..Default::default()
    }];
    ModuleImage {
        name: "fixture.mjs".encode_utf16().collect(),
        required_modules: vec![],
        required_attributes: vec![],
        exports: vec![ModuleExport::Local {
            capture: 0,
            name: "answer".encode_utf16().collect(),
        }],
        star_exports: vec![],
        imports: vec![],
        has_top_level_await: false,
        function: f,
    }
}
#[test]
fn module_records_roundtrip_with_distinct_private_proof() {
    let m = module_fixture();
    let bytes = encode_module(&m).unwrap();
    let image = decode_and_verify(&bytes).unwrap();
    assert_eq!(image.module(), Some(&m));
    assert_eq!(image.function(), &m.function);
    assert!(
        encode(&m.function).is_err(),
        "ordinary function loader cannot prove a module body"
    );
}
#[test]
fn module_indices_bindings_and_two_phase_entry_are_checked() {
    let mut m = module_fixture();
    m.exports.push(m.exports[0].clone());
    assert!(encode_module(&m).is_err());
    let mut m = module_fixture();
    m.exports = vec![ModuleExport::Local {
        capture: 1,
        name: vec![],
    }];
    assert!(encode_module(&m).is_err());
    let mut m = module_fixture();
    m.exports = vec![ModuleExport::Indirect {
        module: 0,
        import: vec![],
        name: vec![],
    }];
    assert!(encode_module(&m).is_err());
    let mut m = module_fixture();
    m.star_exports = vec![0];
    assert!(encode_module(&m).is_err());
    let mut m = module_fixture();
    m.has_top_level_await = true;
    encode_module(&m).unwrap();
    let mut m = module_fixture();
    m.function.strict = false;
    assert!(encode_module(&m).is_err());
    let mut m = module_fixture();
    m.function.captures[0].closure_type = 0;
    assert!(encode_module(&m).is_err());
    let mut m = module_fixture();
    m.function.captures[0].closure_type = 7;
    assert!(encode_module(&m).is_err());
    let mut m = module_fixture();
    m.imports = vec![ModuleImport {
        capture: 0,
        name: vec![b'x' as u16],
        module: 0,
    }];
    assert!(encode_module(&m).is_err());
    let mut m = module_fixture();
    m.function.code[6] = ids::nop;
    assert!(
        encode_module(&m).is_err(),
        "initialization must not reach return_async"
    );
    let mut m = module_fixture();
    m.function.code.pop();
    m.function.code.push(ids::r#return);
    assert!(
        encode_module(&m).is_err(),
        "evaluation must use return_async"
    );
    let mut m = module_fixture();
    m.function.code = vec![ids::undefined, ids::return_async];
    assert!(encode_module(&m).is_err(), "phase prologue required");
}

#[test]
fn module_import_and_reexport_index_spaces_roundtrip() {
    let mut m = module_fixture();
    m.required_modules = vec!["dependency.mjs".encode_utf16().collect()];
    m.function.captures.push(Capture {
        name: vec![b'x' as u16],
        closure_type: 7,
        var_idx: 0,
        is_const: true,
        is_lexical: true,
        ..Default::default()
    });
    m.imports = vec![ModuleImport {
        capture: 1,
        name: vec![b'x' as u16],
        module: 0,
    }];
    m.exports.push(ModuleExport::Indirect {
        module: 0,
        import: vec![b'x' as u16],
        name: vec![b'y' as u16],
    });
    m.star_exports = vec![0];
    assert_eq!(
        decode_and_verify(&encode_module(&m).unwrap())
            .unwrap()
            .module(),
        Some(&m)
    );
    let mut bad = m.clone();
    bad.imports[0].capture = 5;
    assert!(encode_module(&bad).is_err());
    let mut bad = m.clone();
    bad.imports.push(bad.imports[0].clone());
    assert!(encode_module(&bad).is_err());
    let mut bad = m.clone();
    bad.function.captures[1].var_idx = 1;
    assert!(encode_module(&bad).is_err());
    let mut bad = m.clone();
    bad.function.captures[1].closure_type = 6;
    assert!(encode_module(&bad).is_err());
    let mut bad = m.clone();
    bad.function.captures[1].is_const = false;
    assert!(encode_module(&bad).is_err());
    let mut bad = m.clone();
    bad.function.captures[1].name = bad.function.captures[0].name.clone();
    assert!(encode_module(&bad).is_err());
    m.imports[0].name = vec![b'*' as u16];
    m.function.captures[1].closure_type = 6;
    decode_and_verify(&encode_module(&m).unwrap()).unwrap();
}
#[test]
fn postfix_updates_have_two_results_and_require_operand() {
    for op in [ids::post_inc, ids::post_dec] {
        let f = general(vec![ids::push_1, op, ids::drop, ids::r#return], 2);
        decode_and_verify(&encode(&f).unwrap()).unwrap();
        assert!(encode(&general(vec![op, ids::drop, ids::r#return], 2)).is_err());
        assert!(encode(&general(f.code, 1)).is_err());
    }
}

fn eval_native_fixtures() -> Vec<(FunctionImage, eval::VerifiedEvalImage)> {
    use eval::*;
    let mut selected = general(
        vec![
            ids::push_i8,
            40,
            ids::put_loc,
            0,
            0,
            ids::get_var,
            0,
            0,
            0,
            0,
            ids::call,
            0,
            0,
            ids::drop,
            ids::get_loc,
            0,
            0,
            ids::get_arg,
            0,
            0,
            ids::add,
            ids::get_var_ref,
            0,
            0,
            ids::add,
            ids::r#return,
        ],
        2,
    );
    selected.function_name = "selected".into();
    selected.arg_count = 1;
    selected.defined_arg_count = 1;
    selected.local_count = 1;
    selected.var_ref_count = 2;
    selected.locals = vec![
        VarDef {
            name: vec![b'a' as u16],
            is_captured: true,
            var_ref_idx: 0,
            ..Default::default()
        },
        VarDef {
            name: vec![b'x' as u16],
            is_captured: true,
            var_ref_idx: 1,
            ..Default::default()
        },
    ];
    selected.captures = vec![Capture {
        name: "outer".encode_utf16().collect(),
        closure_type: 0,
        var_idx: 0,
        ..Default::default()
    }];
    selected.atoms = vec!["probeEval".encode_utf16().collect()];
    let mut root = general(
        vec![
            ids::push_i8,
            100,
            ids::put_loc,
            0,
            0,
            ids::fclosure8,
            0,
            ids::push_2,
            ids::call,
            1,
            0,
            ids::r#return,
        ],
        2,
    );
    root.local_count = 1;
    root.var_ref_count = 1;
    root.locals = vec![VarDef {
        name: "outer".encode_utf16().collect(),
        is_captured: true,
        ..Default::default()
    }];
    root.constants = vec![Constant::Function(Box::new(selected))];
    let env = verify_env_layout(EnvLayout {
        args: vec![EnvBinding {
            name: vec![b'a' as u16],
            is_captured: true,
            var_ref_idx: 0,
            initialized: true,
            live: true,
            ..Default::default()
        }],
        locals: vec![EnvBinding {
            name: vec![b'x' as u16],
            is_captured: true,
            var_ref_idx: 1,
            initialized: true,
            live: true,
            ..Default::default()
        }],
        refs: vec![EnvBinding {
            name: "outer".encode_utf16().collect(),
            initialized: true,
            live: true,
            ..Default::default()
        }],
        var_ref_count: 2,
        ..Default::default()
    })
    .unwrap();
    [("x", 0), ("a", 1), ("outer", 2)]
        .into_iter()
        .map(|(name, kind)| {
            let mut f = general(
                vec![
                    ids::get_var_ref,
                    0,
                    0,
                    ids::push_1,
                    ids::add,
                    ids::dup,
                    ids::put_var_ref,
                    0,
                    0,
                    ids::r#return,
                ],
                2,
            );
            f.function_name = "<eval>".into();
            f.captures = vec![Capture {
                name: name.encode_utf16().collect(),
                closure_type: kind,
                var_idx: 0,
                ..Default::default()
            }];
            let image = decode_eval_and_verify(&encode_eval(&f, &env).unwrap(), &env).unwrap();
            (root.clone(), image)
        })
        .collect()
}

fn arguments_child(strict: bool, duplicate: bool, body: Vec<u8>) -> FunctionImage {
    let mut code = vec![ids::special_object, u8::from(!strict), ids::put_loc, 0, 0];
    code.extend(body);
    let mut f = general(code, 3);
    f.strict = strict;
    f.arg_count = 2;
    f.defined_arg_count = 2;
    f.local_count = 1;
    f.var_ref_count = 3;
    f.flags.arguments_allowed = true;
    f.locals = vec![
        VarDef {
            name: if duplicate { vec![] } else { vec![b'a' as u16] },
            is_captured: true,
            var_ref_idx: 0,
            ..Default::default()
        },
        VarDef {
            name: vec![b'b' as u16],
            is_captured: true,
            var_ref_idx: 1,
            ..Default::default()
        },
        VarDef {
            name: "arguments".encode_utf16().collect(),
            is_captured: true,
            var_ref_idx: 2,
            ..Default::default()
        },
    ];
    f
}
fn call_arguments_child(child: FunctionImage, escaped: bool) -> FunctionImage {
    let mut code = vec![ids::fclosure8, 0, ids::push_1, ids::push_2, ids::call, 2, 0];
    if escaped {
        code.extend([ids::push_0, ids::get_array_el]);
    }
    code.push(ids::r#return);
    let mut root = general(code, 3);
    root.constants = vec![Constant::Function(Box::new(child))];
    root
}
fn arguments_native_fixtures() -> Vec<(FunctionImage, &'static str)> {
    let mut out = Vec::new();
    for strict in [true, false] {
        let body = vec![
            ids::push_i8,
            7,
            ids::put_arg,
            0,
            0,
            ids::get_loc,
            0,
            0,
            ids::push_0,
            ids::get_array_el,
            ids::r#return,
        ];
        out.push((
            call_arguments_child(arguments_child(strict, false, body), false),
            if strict { "1" } else { "7" },
        ));
        let body = vec![
            ids::get_loc,
            0,
            0,
            ids::push_0,
            ids::push_i8,
            7,
            ids::put_array_el,
            ids::get_arg,
            0,
            0,
            ids::r#return,
        ];
        out.push((
            call_arguments_child(arguments_child(strict, false, body), false),
            if strict { "1" } else { "7" },
        ));
        let body = vec![
            ids::get_loc,
            0,
            0,
            ids::get_loc,
            0,
            0,
            ids::strict_eq,
            ids::r#return,
        ];
        out.push((
            call_arguments_child(arguments_child(strict, false, body), false),
            "true",
        ));
        let body = vec![ids::get_loc, 0, 0, ids::r#return];
        out.push((
            call_arguments_child(arguments_child(strict, false, body), true),
            "1",
        ));
    }
    let body = vec![
        ids::push_i8,
        7,
        ids::put_arg,
        1,
        0,
        ids::get_loc,
        0,
        0,
        ids::push_0,
        ids::get_array_el,
        ids::push_i8,
        10,
        ids::mul,
        ids::get_loc,
        0,
        0,
        ids::push_1,
        ids::get_array_el,
        ids::add,
        ids::r#return,
    ];
    out.push((
        call_arguments_child(arguments_child(false, true, body), false),
        "17",
    ));
    let body = vec![
        ids::get_loc,
        0,
        0,
        ids::push_0,
        ids::push_i8,
        7,
        ids::put_array_el,
        ids::get_arg,
        1,
        0,
        ids::r#return,
    ];
    out.push((
        call_arguments_child(arguments_child(false, true, body), false),
        "2",
    ));
    let body = vec![
        ids::get_loc,
        0,
        0,
        ids::get_field,
        0,
        0,
        0,
        0,
        ids::special_object,
        2,
        ids::strict_eq,
        ids::r#return,
    ];
    let mut f = arguments_child(false, false, body);
    f.atoms = vec!["callee".encode_utf16().collect()];
    out.push((call_arguments_child(f, false), "true"));
    let body = vec![
        ids::push_i8,
        9,
        ids::put_loc,
        0,
        0,
        ids::get_loc,
        0,
        0,
        ids::r#return,
    ];
    out.push((
        call_arguments_child(arguments_child(false, false, body), false),
        "9",
    ));
    let mut shadow = general(vec![ids::get_arg, 0, 0, ids::r#return], 1);
    shadow.arg_count = 1;
    shadow.defined_arg_count = 1;
    shadow.var_ref_count = 1;
    shadow.locals = vec![VarDef {
        name: "arguments".encode_utf16().collect(),
        is_captured: true,
        ..Default::default()
    }];
    shadow.flags.arguments_allowed = true;
    let mut root = general(
        vec![
            ids::fclosure8,
            0,
            ids::push_i8,
            9,
            ids::call,
            1,
            0,
            ids::r#return,
        ],
        2,
    );
    root.constants = vec![Constant::Function(Box::new(shadow))];
    out.push((root, "9"));
    out
}

#[test]
fn module_capture_cannot_forge_materialized_arguments_provenance() {
    let mut module = module_fixture();
    module
        .required_modules
        .push("dep.mjs".encode_utf16().collect());
    module.function.captures.push(Capture {
        name: "imported".encode_utf16().collect(),
        closure_type: 7,
        is_const: true,
        is_lexical: true,
        ..Default::default()
    });
    module.imports.push(ModuleImport {
        capture: 1,
        name: "value".encode_utf16().collect(),
        module: 0,
    });
    assert!(encode_module(&module).is_ok());
    for slot in 0..module.function.captures.len() {
        let mut forged = module.clone();
        for kind in [
            VAR_KIND_ARGUMENTS,
            VAR_KIND_BODY_ANCHOR,
            VAR_KIND_BODY_ARGUMENTS_ANCHOR,
        ] {
            forged.function.captures[slot].var_kind = kind;
            assert!(
                encode_module(&forged).is_err(),
                "module capture {slot} forged provenance tag {kind}"
            );
        }
    }
}
