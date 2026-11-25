use qjs_bytecode::{image::*, production::ids};
fn function() -> FunctionImage {
    FunctionImage {
        code: vec![ids::push_1, ids::r#return],
        stack_size: 1,
        ..Default::default()
    }
}
#[test]
fn caller_work_quota_is_enforced_at_boundary() {
    let f = function();
    let l = ImageLimits {
        max_work: 0,
        ..Default::default()
    };
    assert!(encode_with_limits(&f, &l).is_err());
}
fn capacity<T: std::fmt::Debug>(result: Result<T, String>) {
    let e = result.unwrap_err();
    assert!(e.starts_with("capacity:"), "{e}");
}
fn child() -> FunctionImage {
    let mut f = function();
    f.constants = vec![Constant::Function(Box::new(function()))];
    f
}
fn metadata() -> SpanMetadata {
    SpanMetadata {
        source_unit: SourceUnit::Utf16,
        source_len: 1,
        source_digest: source_digest_utf16(&[b'x' as u16]),
        functions: vec![FunctionSpans {
            path: vec![],
            function_start: 0,
            function_end: 1,
            positions: vec![qjs_bytecode::snapshot::Position {
                code_offset: 0,
                source_start: 0,
                source_end: 1,
            }],
        }],
    }
}
#[test]
fn limits_are_tighten_only_and_representation_boundaries_are_exact() {
    let f = function();
    let d = ImageLimits::default();
    capacity(encode_with_limits(
        &f,
        &ImageLimits {
            max_work: d.max_work + 1,
            ..d
        },
    ));
    capacity(encode_with_limits(
        &f,
        &ImageLimits {
            max_instructions: 1,
            ..d
        },
    ));
    encode_with_limits(
        &f,
        &ImageLimits {
            max_instructions: 2,
            ..d
        },
    )
    .unwrap();
    capacity(encode_with_limits(
        &f,
        &ImageLimits {
            max_state_cells: 1,
            ..d
        },
    ));
    encode_with_limits(
        &f,
        &ImageLimits {
            max_state_cells: 2,
            ..d
        },
    )
    .unwrap();
    let bytes = encode(&f).unwrap();
    capacity(encode_with_limits(
        &f,
        &ImageLimits {
            max_native_bytes: bytes.len() - 1,
            ..d
        },
    ));
    assert_eq!(
        encode_with_limits(
            &f,
            &ImageLimits {
                max_native_bytes: bytes.len(),
                ..d
            }
        )
        .unwrap(),
        bytes
    );
    capacity(decode_and_verify_with_limits(
        &bytes,
        &ImageLimits {
            max_native_bytes: bytes.len() - 1,
            ..d
        },
    ));
    decode_and_verify_with_limits(
        &bytes,
        &ImageLimits {
            max_native_bytes: bytes.len(),
            ..d
        },
    )
    .unwrap();
}
#[test]
fn graph_depth_function_and_state_counts_are_aggregate() {
    let f = child();
    let d = ImageLimits::default();
    capacity(encode_with_limits(
        &f,
        &ImageLimits {
            max_functions: 1,
            ..d
        },
    ));
    capacity(encode_with_limits(&f, &ImageLimits { max_depth: 0, ..d }));
    capacity(encode_with_limits(
        &f,
        &ImageLimits {
            max_state_cells: 3,
            ..d
        },
    ));
    let l = ImageLimits {
        max_functions: 2,
        max_depth: 1,
        max_state_cells: 4,
        ..d
    };
    let bytes = encode_with_limits(&f, &l).unwrap();
    decode_and_verify_with_limits(&bytes, &l).unwrap();
    capacity(decode_and_verify_with_limits(
        &bytes,
        &ImageLimits {
            max_functions: 1,
            ..d
        },
    ));
    capacity(decode_and_verify_with_limits(
        &bytes,
        &ImageLimits {
            max_state_cells: 3,
            ..d
        },
    ));
}
#[test]
fn global_atoms_and_positions_obey_caller_limits() {
    let mut f = function();
    let d = ImageLimits::default();
    encode_with_limits(
        &f,
        &ImageLimits {
            max_table_entries: 1,
            ..d
        },
    )
    .unwrap();
    f.function_name = "named".into();
    capacity(encode_with_limits(
        &f,
        &ImageLimits {
            max_table_entries: 1,
            ..d
        },
    ));
    let bytes = encode(&f).unwrap();
    capacity(decode_and_verify_with_limits(
        &bytes,
        &ImageLimits {
            max_table_entries: 1,
            ..d
        },
    ));
    f.debug_locations = vec![
        DebugLocation {
            code_offset: 0,
            line: 1,
            column: 1,
        },
        DebugLocation {
            code_offset: 1,
            line: 1,
            column: 2,
        },
    ];
    capacity(encode_with_limits(
        &f,
        &ImageLimits {
            max_positions: 1,
            ..d
        },
    ));
    let bytes = encode(&f).unwrap();
    capacity(decode_and_verify_with_limits(
        &bytes,
        &ImageLimits {
            max_positions: 1,
            ..d
        },
    ));
}
#[test]
fn state_and_instruction_guards_precede_decoded_vector_allocations() {
    let f = FunctionImage {
        code: vec![ids::return_undef; 1000],
        local_count: 65535,
        ..Default::default()
    };
    capacity(encode(&f));
    let f = FunctionImage {
        code: vec![ids::nop; 1_000_001],
        ..Default::default()
    };
    capacity(encode(&f));
    let f = FunctionImage {
        local_count: 65536,
        ..function()
    };
    capacity(encode(&f));
    let f = FunctionImage {
        constants: vec![Constant::Bool(true); 65536],
        ..function()
    };
    capacity(encode(&f));
}
#[test]
fn semantic_work_boundary_is_enforced_after_preflight() {
    let f = function();
    let d = ImageLimits::default();
    let minimum = (1..1000)
        .find(|n| encode_with_limits(&f, &ImageLimits { max_work: *n, ..d }).is_ok())
        .unwrap();
    assert!(minimum > 4);
    capacity(encode_with_limits(
        &f,
        &ImageLimits {
            max_work: minimum - 1,
            ..d
        },
    ));
    encode_with_limits(
        &f,
        &ImageLimits {
            max_work: minimum,
            ..d
        },
    )
    .unwrap();
}
#[test]
fn serialization_checks_artifact_capacity_before_building_envelope() {
    let image = verify_and_encode(&function(), Some(metadata())).unwrap();
    let bytes = image.serialize_artifact().unwrap();
    let d = ImageLimits::default();
    capacity(image.serialize_artifact_with_limits(&ImageLimits {
        max_artifact_bytes: bytes.len() - 1,
        ..d
    }));
    assert_eq!(
        image
            .serialize_artifact_with_limits(&ImageLimits {
                max_artifact_bytes: bytes.len(),
                ..d
            })
            .unwrap(),
        bytes
    );
    capacity(deserialize_artifact_with_limits(
        &bytes,
        &ImageLimits {
            max_artifact_bytes: bytes.len() - 1,
            ..d
        },
    ));
}
#[test]
fn metadata_and_native_allocations_share_one_graph_allowance() {
    let f = function();
    let raw = encode(&f).unwrap();
    let image = verify_and_encode(&f, Some(metadata())).unwrap();
    let artifact = image.serialize_artifact().unwrap();
    let d = ImageLimits::default();
    let native_min = (1..4000)
        .find(|n| {
            decode_and_verify_with_limits(
                &raw,
                &ImageLimits {
                    max_graph_bytes: *n,
                    ..d
                },
            )
            .is_ok()
        })
        .unwrap();
    let span_cost = std::mem::size_of::<FunctionSpans>()
        + std::mem::size_of::<qjs_bytecode::snapshot::Position>();
    let between = native_min + span_cost - 1;
    assert!(between >= span_cost);
    decode_and_verify_with_limits(
        &raw,
        &ImageLimits {
            max_graph_bytes: between,
            ..d
        },
    )
    .unwrap();
    capacity(deserialize_artifact_with_limits(
        &artifact,
        &ImageLimits {
            max_graph_bytes: between,
            ..d
        },
    ));
    deserialize_artifact_with_limits(
        &artifact,
        &ImageLimits {
            max_graph_bytes: native_min + span_cost,
            ..d
        },
    )
    .unwrap();
}
#[test]
fn whole_operation_work_does_not_reset_between_metadata_and_payload() {
    let f = function();
    let image = verify_and_encode(&f, Some(metadata())).unwrap();
    let artifact = image.serialize_artifact().unwrap();
    let d = ImageLimits::default();
    let plain = (1..8000)
        .find(|n| {
            verify_and_encode_with_limits(&f, None, &ImageLimits { max_work: *n, ..d }).is_ok()
        })
        .unwrap();
    let combined = (1..8000)
        .find(|n| {
            verify_and_encode_with_limits(&f, Some(metadata()), &ImageLimits { max_work: *n, ..d })
                .is_ok()
        })
        .unwrap();
    assert!(combined > plain);
    capacity(verify_and_encode_with_limits(
        &f,
        Some(metadata()),
        &ImageLimits {
            max_work: combined - 1,
            ..d
        },
    ));
    let minimum = (1..8000)
        .find(|n| {
            image
                .serialize_artifact_with_limits(&ImageLimits { max_work: *n, ..d })
                .is_ok()
        })
        .unwrap();
    capacity(image.serialize_artifact_with_limits(&ImageLimits {
        max_work: minimum - 1,
        ..d
    }));
    assert_eq!(
        image
            .serialize_artifact_with_limits(&ImageLimits {
                max_work: minimum,
                ..d
            })
            .unwrap(),
        artifact
    );
}
#[test]
fn eval_artifacts_bind_complete_spans_profile_target_and_external_layout() {
    use qjs_bytecode::image::eval::*;
    let raw = EnvLayout {
        locals: vec![EnvBinding {
            name: vec![b'x' as u16],
            is_captured: true,
            var_ref_idx: 0,
            initialized: true,
            live: true,
            ..Default::default()
        }],
        var_ref_count: 1,
        ..Default::default()
    };
    let layout = verify_env_layout(raw.clone()).unwrap();
    let f = FunctionImage {
        code: vec![ids::get_var_ref, 0, 0, ids::r#return],
        captures: vec![Capture {
            name: vec![b'x' as u16],
            closure_type: 0,
            var_idx: 0,
            ..Default::default()
        }],
        stack_size: 1,
        ..Default::default()
    };
    let image = verify_and_encode_eval(&f, &layout, Some(metadata())).unwrap();
    let bytes = image.serialize_artifact().unwrap();
    let copy = deserialize_eval_artifact(&bytes, &layout).unwrap();
    assert_eq!(copy.bytes(), image.bytes());
    assert_eq!(copy.spans(), image.spans());
    assert_eq!(copy.profile(), COMPILER_PROFILE);
    assert_eq!(copy.layout_digest(), layout.digest());
    assert!(deserialize_artifact(&bytes).is_err());
    assert!(decode_and_verify(image.bytes()).is_err());
    let mut other = raw;
    other.locals[0].initialized = false;
    let other = verify_env_layout(other).unwrap();
    assert!(deserialize_eval_artifact(&bytes, &other)
        .unwrap_err()
        .contains("environment digest"));
    for i in 0..bytes.len() {
        let mut bad = bytes.clone();
        bad[i] ^= 1;
        assert!(deserialize_eval_artifact(&bad, &layout).is_err(), "byte{i}");
    }
    let d = ImageLimits::default();
    capacity(image.serialize_artifact_with_limits(&ImageLimits {
        max_artifact_bytes: bytes.len() - 1,
        ..d
    }));
    capacity(deserialize_eval_artifact_with_limits(
        &bytes,
        &layout,
        &ImageLimits {
            max_artifact_bytes: bytes.len() - 1,
            ..d
        },
    ));
    capacity(verify_and_encode_eval_with_limits(
        &f,
        &layout,
        None,
        &ImageLimits { max_work: 0, ..d },
    ));
}
#[test]
fn module_and_eval_constructors_enforce_shared_work_not_fresh_metadata_work() {
    let mut f = FunctionImage {
        code: vec![
            ids::push_this,
            ids::if_false,
            5,
            0,
            0,
            0,
            ids::return_undef,
            ids::undefined,
            ids::return_async,
        ],
        stack_size: 1,
        strict: true,
        ..Default::default()
    };
    f.flags.kind = 2;
    let m = ModuleImage {
        name: vec![b'm' as u16],
        required_modules: vec![],
        required_attributes: vec![],
        exports: vec![],
        star_exports: vec![],
        imports: vec![],
        has_top_level_await: false,
        function: f,
    };
    let d = ImageLimits::default();
    let a = (1..8000)
        .find(|n| {
            verify_and_encode_module_with_limits(&m, None, &ImageLimits { max_work: *n, ..d })
                .is_ok()
        })
        .unwrap();
    let b = (1..8000)
        .find(|n| {
            verify_and_encode_module_with_limits(
                &m,
                Some(metadata()),
                &ImageLimits { max_work: *n, ..d },
            )
            .is_ok()
        })
        .unwrap();
    assert!(b > a);
    capacity(verify_and_encode_module_with_limits(
        &m,
        Some(metadata()),
        &ImageLimits {
            max_work: b - 1,
            ..d
        },
    ));
    use qjs_bytecode::image::eval::*;
    let layout = verify_env_layout(EnvLayout::default()).unwrap();
    let f = function();
    let a = (1..8000)
        .find(|n| {
            verify_and_encode_eval_with_limits(
                &f,
                &layout,
                None,
                &ImageLimits { max_work: *n, ..d },
            )
            .is_ok()
        })
        .unwrap();
    let b = (1..8000)
        .find(|n| {
            verify_and_encode_eval_with_limits(
                &f,
                &layout,
                Some(metadata()),
                &ImageLimits { max_work: *n, ..d },
            )
            .is_ok()
        })
        .unwrap();
    assert!(b > a);
    capacity(verify_and_encode_eval_with_limits(
        &f,
        &layout,
        Some(metadata()),
        &ImageLimits {
            max_work: b - 1,
            ..d
        },
    ));
}
