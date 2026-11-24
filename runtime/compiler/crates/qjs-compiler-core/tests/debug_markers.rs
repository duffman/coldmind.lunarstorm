use qjs_bytecode::{
    Limits,
    image::{Constant, FunctionImage},
};
use qjs_compiler_core::{JsEmitOptions, compile_script, compile_script_with_options};

#[test]
fn debug_handlers_distinguish_user_catches_from_finally_cleanup() {
    // lunarstorm patch: the user/cleanup distinction moved from jetbunny's
    // `debug_catch` flag operand to `FunctionImage::user_catches`. Same
    // assertions: plain compiles carry nothing, the debug compile marks the
    // user catch and not the finally, malformed metadata is rejected.
    let source = "try { try { throw 7; } finally { 1; } } catch(e) { e; }";
    let plain = compile_script(source, "handlers.js").unwrap();
    let debug = compile_script_with_options(
        source,
        "handlers.js",
        &JsEmitOptions {
            debug_markers: true,
        },
    )
    .unwrap();
    let catch_targets = |f: &FunctionImage| {
        let mut targets = qjs_bytecode::decode_schema(
            &f.code,
            Limits::default(),
            qjs_bytecode::production::FINAL_OPCODES,
        )
        .unwrap()
        .instructions()
        .iter()
        .filter(|i| i.opcode().name == "catch")
        .map(|i| u32::try_from(i.branch_target().unwrap()).unwrap())
        .collect::<Vec<_>>();
        targets.sort_unstable();
        targets
    };
    assert!(
        qjs_bytecode::production::FINAL_OPCODES
            .iter()
            .all(|op| op.name != "debug_catch"),
        "debug_catch is no longer an opcode"
    );
    assert_eq!(plain.function().user_catches, None);
    // Debug markers shift offsets, so compare the handler count, not offsets.
    assert_eq!(catch_targets(plain.function()).len(), catch_targets(debug.function()).len());
    let targets = catch_targets(debug.function());
    assert_eq!(targets.len(), 2, "one user catch and one finally handler");
    let table = debug.function().user_catches.clone().unwrap();
    assert_eq!(table.len(), 1, "only the user catch is listed");
    assert!(targets.contains(&table[0]));
    let decoded = qjs_bytecode::image::decode_and_verify(debug.bytes()).unwrap();
    assert_eq!(decoded.function().user_catches, Some(table.clone()), "the table round-trips");
    let finally_target = *targets.iter().find(|t| **t != table[0]).unwrap();
    let mut malformed = debug.function().clone();
    malformed.user_catches = Some(vec![table[0] + 1]);
    assert!(
        qjs_bytecode::image::encode(&malformed).is_err(),
        "entries that are not catch targets are rejected before native loading"
    );
    malformed.user_catches = Some(vec![table[0].max(finally_target), table[0].min(finally_target)]);
    assert!(
        qjs_bytecode::image::encode(&malformed).is_err(),
        "unordered tables are rejected before native loading"
    );
}
fn markers(function: &FunctionImage) -> Vec<(u32, u32)> {
    let decoded = qjs_bytecode::decode_schema(
        &function.code,
        Limits::default(),
        qjs_bytecode::production::FINAL_OPCODES,
    )
    .unwrap();
    decoded
        .instructions()
        .iter()
        .filter(|i| i.opcode().name == "debug_stmt")
        .map(|i| {
            (
                i.offset() as u32,
                u32::from_ne_bytes(i.operands().try_into().unwrap()),
            )
        })
        .collect()
}
fn child<'a>(function: &'a FunctionImage, name: &str) -> &'a FunctionImage {
    function
        .constants
        .iter()
        .find_map(|c| {
            if let Constant::Function(f) = c
                && f.function_name == name
            {
                Some(f.as_ref())
            } else {
                None
            }
        })
        .unwrap()
}
#[test]
fn markers_are_opt_in_with_sequential_ids_and_original_positions() {
    let source = "function outer(x){function inner(y){return x+y;}return inner(2);}outer(40);";
    let plain = compile_script(source, "markers.js").unwrap();
    assert!(markers(plain.function()).is_empty());
    let explicit =
        compile_script_with_options(source, "markers.js", &JsEmitOptions::default()).unwrap();
    assert_eq!(plain.bytes(), explicit.bytes());
    let debug = compile_script_with_options(
        source,
        "markers.js",
        &JsEmitOptions {
            debug_markers: true,
        },
    )
    .unwrap();
    for function in [
        debug.function(),
        child(debug.function(), "outer"),
        child(child(debug.function(), "outer"), "inner"),
    ] {
        let markers = markers(function);
        assert!(!markers.is_empty());
        for (id, (offset, operand)) in markers.iter().enumerate() {
            assert_eq!(*operand >> 16, id as u32);
            assert_eq!(*operand & 0xffff, 0);
            assert!(
                function
                    .debug_locations
                    .iter()
                    .any(|p| p.code_offset == *offset && p.line > 0 && p.column > 0)
            );
        }
    }
}
#[test]
fn nested_lexical_markers_restore_the_exact_enclosing_head() {
    let source = "function f(a){let x=1;{let y=2;x+y;}x+a;}f(40);";
    let debug = compile_script_with_options(
        source,
        "scopes.js",
        &JsEmitOptions {
            debug_markers: true,
        },
    )
    .unwrap();
    let function = child(debug.function(), "f");
    let heads: Vec<_> = markers(function).iter().map(|(_, v)| v & 0xffff).collect();
    let local = |name: &str| {
        function.locals[function.arg_count as usize..]
            .iter()
            .position(|v| v.name == name.encode_utf16().collect::<Vec<_>>())
            .unwrap() as u32
    };
    let x = local("x");
    let y = local("y");
    assert_eq!(heads, vec![x + 1, x + 1, y + 1, y + 1, x + 1]);
    assert_eq!(
        function.locals[(function.arg_count + y) as usize]
            .scope_next
            .local_index(),
        Some(x)
    );
}
#[test]
fn loop_markers_follow_lexical_closure_lifetimes() {
    for source in [
        "let f;for(let i=0;i<2;i++){let x=i;f=function(){return x+i;};if(i==0)continue;}f();",
        "function f(){let x=0;while(x<2){let y=x;x++;if(x==1)continue;}do{let z=x;x--;}while(x>0);return x;}f();",
    ] {
        compile_script_with_options(
            source,
            "loops.js",
            &JsEmitOptions {
                debug_markers: true,
            },
        )
        .unwrap_or_else(|e| panic!("{source}: {e}"));
    }
}

#[test]
fn debug_materializes_captured_this_once_without_adding_default_bindings() {
    let source = "function f(x){return x;}f(42);";
    let plain = compile_script(source, "this.js").unwrap();
    assert!(
        !child(plain.function(), "f")
            .locals
            .iter()
            .any(|v| v.name == "this".encode_utf16().collect::<Vec<_>>())
    );
    let debug = compile_script_with_options(
        source,
        "this.js",
        &JsEmitOptions {
            debug_markers: true,
        },
    )
    .unwrap();
    let function = child(debug.function(), "f");
    let binding = function
        .locals
        .iter()
        .find(|v| v.name == "this".encode_utf16().collect::<Vec<_>>())
        .unwrap();
    assert!(binding.is_captured);
    assert!(!binding.is_lexical);
    assert_eq!(binding.scope_level, 0);
    let decoded = qjs_bytecode::decode_schema(
        &function.code,
        Limits::default(),
        qjs_bytecode::production::FINAL_OPCODES,
    )
    .unwrap();
    assert_eq!(
        decoded
            .instructions()
            .iter()
            .filter(|i| i.opcode().name == "push_this")
            .count(),
        1
    );
}

#[test]
fn astral_and_crlf_marker_columns_roundtrip_without_span_metadata() {
    let source = "/*😀*/ let a=1; a++;\r\n/*😀*/ a++;";
    let debug = compile_script_with_options(
        source,
        "utf16-markers.js",
        &JsEmitOptions {
            debug_markers: true,
        },
    )
    .unwrap();
    assert!(debug.spans().is_some());
    assert_eq!(
        debug.function().source,
        Some(source.encode_utf16().collect())
    );
    let reloaded = qjs_bytecode::image::decode_and_verify(debug.bytes()).unwrap();
    assert!(reloaded.spans().is_none());
    for function in [debug.function(), reloaded.function()] {
        let positions: Vec<_> = markers(function)
            .iter()
            .map(|(offset, _)| {
                let position = function
                    .debug_locations
                    .iter()
                    .find(|position| position.code_offset == *offset)
                    .expect("each executable marker has its own exact location");
                (position.line, position.column)
            })
            .collect();
        assert_eq!(positions, vec![(1, 8), (1, 17), (2, 8)]);
        assert_eq!(function.source, Some(source.encode_utf16().collect()));
    }
}

#[test]
fn materialized_arguments_metadata_distinguishes_inactive_body_collision() {
    let image = compile_script_with_options(
        "function f(a=1){var arguments=42;return arguments;}",
        "arguments-scope.js",
        &JsEmitOptions {
            debug_markers: true,
        },
    )
    .unwrap();
    let function = child(image.function(), "f");
    let kinds: Vec<_> = function
        .locals
        .iter()
        .filter(|v| v.scope_level == 0 && v.name == "arguments".encode_utf16().collect::<Vec<_>>())
        .map(|v| v.var_kind)
        .collect();
    assert_eq!(
        kinds,
        vec![qjs_bytecode::image::VAR_KIND_ARGUMENTS, 0],
        "actual initializer and inactive body declaration need distinct metadata"
    );
}

#[test]
fn split_body_anchor_is_unique_and_cannot_forge_shadow_or_value_authority() {
    use qjs_bytecode::image::{VAR_KIND_BODY_ANCHOR, VAR_KIND_BODY_ARGUMENTS_ANCHOR, encode};
    for (source, kind) in [
        (
            "function f(x=1){var x=77;debugger;return()=>x;}",
            VAR_KIND_BODY_ANCHOR,
        ),
        (
            "function f(x=1){var arguments=77;debugger;return()=>x;}",
            VAR_KIND_BODY_ARGUMENTS_ANCHOR,
        ),
    ] {
        let image = compile_script_with_options(
            source,
            "body-anchor.js",
            &JsEmitOptions {
                debug_markers: true,
            },
        )
        .unwrap();
        let original = child(image.function(), "f");
        let anchors: Vec<_> = original
            .locals
            .iter()
            .enumerate()
            .filter(|(_, v)| {
                matches!(
                    v.var_kind,
                    VAR_KIND_BODY_ANCHOR | VAR_KIND_BODY_ARGUMENTS_ANCHOR
                )
            })
            .collect();
        assert_eq!(anchors.len(), 1);
        let (at, definition) = anchors[0];
        assert_eq!(definition.var_kind, kind);
        for change in 0..4 {
            let mut graph = image.function().clone();
            let bad = child_mut(&mut graph, "f");
            match change {
                0 => bad.locals[at].name = "forged".encode_utf16().collect(),
                1 => {
                    bad.locals[at].var_kind = if kind == VAR_KIND_BODY_ANCHOR {
                        VAR_KIND_BODY_ARGUMENTS_ANCHOR
                    } else {
                        VAR_KIND_BODY_ANCHOR
                    }
                }
                2 => bad.locals[at].is_lexical = false,
                _ => bad.locals[at].var_kind = 0,
            }
            assert!(
                encode(&graph).is_err(),
                "forged body anchor change {change} was admitted"
            );
        }
        let mut graph = image.function().clone();
        let bad = child_mut(&mut graph, "f");
        let anchor_slot = at - original.arg_count as usize;
        let decoded = qjs_bytecode::decode_schema(
            &bad.code,
            Limits::default(),
            qjs_bytecode::production::FINAL_OPCODES,
        )
        .unwrap();
        let initializer = |slot: usize| {
            decoded
                .instructions()
                .iter()
                .find(|i| {
                    i.opcode().name == "set_loc_uninitialized"
                        && u16::from_ne_bytes(i.operands().try_into().unwrap()) as usize == slot
                })
                .unwrap()
                .offset()
        };
        let parameter = original.locals[original.arg_count as usize..]
            .iter()
            .position(|v| v.scope_level == 1)
            .unwrap();
        let first = initializer(parameter);
        let body = initializer(anchor_slot);
        bad.code[first + 1..first + 3].copy_from_slice(&(anchor_slot as u16).to_ne_bytes());
        bad.code[body + 1..body + 3].copy_from_slice(&(parameter as u16).to_ne_bytes());
        let error = encode(&graph).unwrap_err();
        assert!(
            error.contains("body scope cannot activate before all parameters"),
            "{error}"
        );
    }
}

#[test]
fn body_anchor_cannot_be_renamed_into_a_child_capture() {
    use qjs_bytecode::image::{VAR_KIND_BODY_ANCHOR, encode};
    let image = compile_script_with_options(
        "function f(x=1){return()=>x;}",
        "anchor-capture.js",
        &JsEmitOptions {
            debug_markers: true,
        },
    )
    .unwrap();
    let mut graph = image.function().clone();
    let bad = child_mut(&mut graph, "f");
    let anchor = bad.locals[bad.arg_count as usize..]
        .iter()
        .position(|v| v.var_kind == VAR_KIND_BODY_ANCHOR)
        .unwrap();
    let child = bad
        .constants
        .iter_mut()
        .find_map(|c| {
            if let Constant::Function(f) = c {
                Some(f)
            } else {
                None
            }
        })
        .unwrap();
    child.captures[0].closure_type = 0;
    child.captures[0].var_idx = anchor as u32;
    child.captures[0].name = "forged".encode_utf16().collect();
    child.captures[0].is_lexical = true;
    child.captures[0].is_const = false;
    child.captures[0].var_kind = 0;
    let error = encode(&graph).unwrap_err();
    assert!(
        error.contains("body scope anchors cannot grant capture authority"),
        "{error}"
    );
}

fn child_mut<'a>(function: &'a mut FunctionImage, name: &str) -> &'a mut FunctionImage {
    function
        .constants
        .iter_mut()
        .find_map(|c| match c {
            Constant::Function(f) if f.function_name == name => Some(f.as_mut()),
            _ => None,
        })
        .unwrap()
}
#[test]
fn computed_class_parameter_lexical_chain_excludes_future_body_anchor() {
    for source in [
        "function pause(){debugger;return 'key'}function f(a=class C{[pause()](){}}){var hidden=77;return 0;}f();",
        "function pause(){debugger;return 'key'}function f(a=class{[pause()](){}}){var hidden=77;return 0;}f();",
    ] {
        compile_script_with_options(
            source,
            "computed.js",
            &JsEmitOptions {
                debug_markers: true,
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
    }
}
