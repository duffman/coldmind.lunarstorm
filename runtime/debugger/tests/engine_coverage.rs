//! Public engine contract checks. Rust fixtures exercise delegation and ownership;
//! they do not interpret JavaScript or stand in for a native adapter.
use coldmind_runtime::{debug::*, engine::*};
use std::{cell::Cell, mem::size_of, rc::Rc};

mod support;

fn source() -> ScriptSource {
    ScriptSource::new(JsString::from_utf8("const answer = 42;"), "app.js".into())
}

fn metadata(source: &ScriptSource) -> ArtifactMetadata {
    ArtifactMetadata {
        adapter_id: "validation-fixture".into(),
        native_version: "fixture-1".into(),
        format_version: 1,
        target_fingerprint: "fixture-target".into(),
        build_fingerprint: "fixture-build".into(),
        options: CompileOptions {
            mode: CodeMode::Script,
            debug_info: DebugInfo::Required,
            fingerprint: "fixture-options".into(),
        },
        source_digest: SourceDigest::of_utf16(&source.text),
    }
}

fn artifact(metadata: ArtifactMetadata, source: ScriptSource) -> CompiledArtifact {
    // SAFETY: Empty output is a trusted validation-only fixture. It is never
    // passed to an adapter or native deserializer; no native bytecode is forged.
    unsafe { CompiledArtifact::from_trusted_compiler_output(metadata, Vec::new(), source) }.unwrap()
}

#[test]
fn source_digest_uses_exact_utf16_little_endian_units_including_lone_surrogates() {
    let text = JsString::from_units(vec![0x0041, 0xd800, 0]);
    let digest = SourceDigest::of_utf16(&text);
    // SHA-256 of the fixed bytes 41 00 00 d8 00 00, not lossy UTF-8 text.
    assert_eq!(
        digest.as_bytes(),
        &[
            0x51, 0xc4, 0x0f, 0x88, 0x60, 0x08, 0x33, 0xf7, 0xef, 0xf3, 0xd9, 0xd2, 0x7a, 0x95,
            0xaa, 0x09, 0x71, 0xa5, 0x34, 0x79, 0x1d, 0x2f, 0x12, 0x3d, 0xf2, 0x57, 0xd8, 0x3e,
            0xad, 0xbb, 0x31, 0x0b,
        ]
    );
    assert_ne!(digest, SourceDigest::of_utf16(&JsString::from_utf8("A�\0")));
    assert_ne!(digest, SourceDigest::of_utf16(&JsString::default()));
}

#[test]
fn artifact_preserves_source_maps_and_exposes_only_matching_metadata() {
    let mut source = source();
    assert!(source.source_map.is_none());
    source.source_map = Some(SourceMapAttachment {
        json: b"{\"version\":3}".to_vec(),
        originals: vec![OriginalSource {
            origin: "app.ts".into(),
            text: Some(JsString::from_utf8("const answer: number = 42;")),
        }],
        preceding_maps: vec![b"preceding map fixture".to_vec()],
    });
    let expected = metadata(&source);
    let compiled = artifact(expected.clone(), source.clone());
    assert_eq!(compiled.metadata(), &expected);
    assert!(compiled.payload().is_empty());
    assert_eq!(compiled.source().text, source.text);
    assert_eq!(compiled.source().origin, "app.js");
    let map = compiled.source().source_map.as_ref().unwrap();
    assert_eq!(map.json, b"{\"version\":3}");
    assert_eq!(map.originals[0].origin, "app.ts");
    assert_eq!(
        map.originals[0].text.as_ref().unwrap().to_utf8().unwrap(),
        "const answer: number = 42;"
    );
    assert_eq!(map.preceding_maps, [b"preceding map fixture".to_vec()]);
    compiled.validate(&expected, false).unwrap();
    compiled.validate(&expected, true).unwrap();
}

#[test]
fn artifact_factory_rejects_a_digest_for_different_source() {
    let source = source();
    let mut wrong = metadata(&source);
    wrong.source_digest = SourceDigest::of_utf16(&JsString::from_utf8("different"));
    // SAFETY: This empty trusted fixture is used only to test metadata rejection,
    // and is never submitted to a native adapter or deserializer.
    let error =
        unsafe { CompiledArtifact::from_trusted_compiler_output(wrong, Vec::new(), source) }
            .unwrap_err();
    assert_eq!(error.code, FailureCode::InvalidArtifact);
    assert_eq!(
        error.message.unwrap().to_utf8().unwrap(),
        "source digest mismatch"
    );
}

#[test]
fn artifact_validation_checks_every_compatibility_field() {
    let source = source();
    let original = metadata(&source);
    let compiled = artifact(original.clone(), source);
    let mut incompatible = Vec::new();
    let mut changed = original.clone();
    changed.adapter_id.push_str("-other");
    incompatible.push(changed);
    let mut changed = original.clone();
    changed.native_version.push_str("-other");
    incompatible.push(changed);
    let mut changed = original.clone();
    changed.format_version += 1;
    incompatible.push(changed);
    let mut changed = original.clone();
    changed.target_fingerprint.push_str("-other");
    incompatible.push(changed);
    let mut changed = original.clone();
    changed.build_fingerprint.push_str("-other");
    incompatible.push(changed);
    let mut changed = original.clone();
    changed.options.mode = CodeMode::Module;
    incompatible.push(changed);
    let mut changed = original.clone();
    changed.options.debug_info = DebugInfo::Strip;
    incompatible.push(changed);
    let mut changed = original.clone();
    changed.options.fingerprint.push_str("-other");
    incompatible.push(changed);
    let mut changed = original;
    changed.source_digest = SourceDigest::of_utf16(&JsString::default());
    incompatible.push(changed);
    for expected in incompatible {
        assert_eq!(
            compiled.validate(&expected, false).unwrap_err().code,
            FailureCode::InvalidArtifact
        );
    }
}

#[test]
fn development_requires_debug_information_even_when_metadata_matches() {
    let source = source();
    let mut expected = metadata(&source);
    expected.options.debug_info = DebugInfo::Strip;
    let compiled = artifact(expected.clone(), source);
    compiled.validate(&expected, false).unwrap();
    assert_eq!(
        compiled.validate(&expected, true).unwrap_err().code,
        FailureCode::InvalidArtifact
    );
}

fn requirements(entry: EntryRequirement) -> EngineRequirements {
    EngineRequirements {
        entry,
        debug: DebugRequirements::disabled(),
        ..Default::default()
    }
}

#[test]
fn entry_requirements_distinguish_callables_source_artifact_and_modules() {
    let absent = EngineFeatures::default();
    requirements(EntryRequirement::ExistingCallables)
        .validate(&absent, None)
        .unwrap();
    for (entry, features) in [
        (
            EntryRequirement::Source,
            EngineFeatures {
                source: true,
                ..absent
            },
        ),
        (
            EntryRequirement::Artifact,
            EngineFeatures {
                artifacts: true,
                ..absent
            },
        ),
        (
            EntryRequirement::Module,
            EngineFeatures {
                modules: true,
                ..absent
            },
        ),
    ] {
        let required = requirements(entry);
        assert_eq!(
            required.validate(&absent, None).unwrap_err().code,
            FailureCode::Unsupported
        );
        required.validate(&features, None).unwrap();
    }
    assert!(matches!(
        EntryRequirement::default(),
        EntryRequirement::Source
    ));
}

#[test]
fn language_extensions_require_module_support_as_well_as_their_own_feature() {
    let mut required = requirements(EntryRequirement::ExistingCallables);
    required.compilation = true;
    assert_eq!(
        required
            .validate(&EngineFeatures::default(), None)
            .unwrap_err()
            .code,
        FailureCode::Unsupported
    );
    required
        .validate(
            &EngineFeatures {
                compiler: true,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    for dynamic in [false, true] {
        required.compilation = false;
        required.top_level_await = !dynamic;
        required.dynamic_import = dynamic;
        for (modules, language) in [(false, false), (true, false), (false, true), (true, true)] {
            let features = EngineFeatures {
                modules,
                top_level_await: !dynamic && language,
                dynamic_import: dynamic && language,
                ..Default::default()
            };
            assert_eq!(
                required.validate(&features, None).is_ok(),
                modules && language
            );
        }
    }
}

#[test]
fn debugger_requirements_enforce_baseline_extensions_and_disabled_mode() {
    let mut required = EngineRequirements {
        entry: EntryRequirement::ExistingCallables,
        ..Default::default()
    };
    let features = EngineFeatures::default();
    let mut descriptor = DebugDescriptor {
        api_version: 1,
        capabilities: DebugCapabilities::baseline(),
        native_requirements: Vec::new(),
        interrupt_mechanism: "fixture metadata only".into(),
    };
    assert_eq!(
        required.validate(&features, None).unwrap_err().code,
        FailureCode::Unsupported
    );
    required.validate(&features, Some(&descriptor)).unwrap();
    required.debug.additional = DebugCapabilities::empty().with(DebugCapability::AsyncStacks);
    assert_eq!(
        required
            .validate(&features, Some(&descriptor))
            .unwrap_err()
            .code,
        FailureCode::Unsupported
    );
    descriptor.capabilities = descriptor.capabilities.with(DebugCapability::AsyncStacks);
    required.validate(&features, Some(&descriptor)).unwrap();
    required.debug.mode = DebugMode::Disabled;
    assert_eq!(
        required
            .validate(&features, Some(&descriptor))
            .unwrap_err()
            .code,
        FailureCode::Unsupported
    );
    required.debug = DebugRequirements::disabled();
    required.validate(&features, None).unwrap();
}

#[test]
fn every_context_limit_must_be_positive() {
    let defaults = ContextLimits::default();
    defaults.validate().unwrap();
    for field in 0..5 {
        let mut limits = defaults;
        match field {
            0 => limits.max_persistent_roots = 0,
            1 => limits.max_host_depth = 0,
            2 => limits.export.max_depth = 0,
            3 => limits.export.max_nodes = 0,
            _ => limits.export.max_bytes = 0,
        }
        assert_eq!(limits.validate().unwrap_err().code, FailureCode::Capacity);
    }
    ContextLimits {
        max_persistent_roots: 1,
        max_host_depth: 1,
        export: ExportLimits {
            max_depth: 1,
            max_nodes: 1,
            max_bytes: 1,
        },
    }
    .validate()
    .unwrap();
}

#[test]
fn boxed_engine_delegates_descriptor_lifecycle_scope_and_errors() {
    for engine in support::engines() {
        let descriptor = JsEngine::descriptor(&engine);
        assert!(descriptor.adapter_id.ends_with("-fixture"));
        assert_eq!(descriptor.owner, std::thread::current().id());
        let context = JsEngine::create_context(&engine, ContextLimits::default()).unwrap();
        let backend = JsEngine::debug_backend(&engine, context).unwrap().unwrap();
        assert_eq!(
            backend.service().unwrap().state,
            DebugExecutionState::Detached
        );
        let called = Cell::new(false);
        let mut call = |scope: &dyn JsScope| {
            called.set(true);
            assert_eq!(scope.context(), context);
            Err(FailureReport::fault(FailureCode::HostPanic, "body failure"))
        };
        assert_eq!(
            JsEngine::with_scope(&engine, context, &support::NoLoop, &mut call)
                .unwrap_err()
                .code,
            FailureCode::HostPanic
        );
        assert!(called.get());
        JsEngine::close_context(&engine, context).unwrap();
        assert_eq!(
            JsEngine::debug_backend(&engine, context)
                .err()
                .unwrap()
                .code,
            FailureCode::ClosedContext
        );
        assert_eq!(
            JsEngine::close_context(&engine, context).unwrap_err().code,
            FailureCode::ClosedContext
        );
    }
}

#[test]
fn js_strings_expose_exact_owned_units_and_diagnostic_rendering() {
    let mut units = Vec::with_capacity(16);
    units.extend([0xd800, 0, 0xdc00]);
    let value = JsString::from_units(units);
    assert!(value.to_utf8().is_err());
    assert_eq!(format!("{value:?}"), "UTF16([55296, 0, 56320])");
    assert!(value.capacity_bytes() >= 32);
    assert_eq!(value.into_units(), [0xd800, 0, 0xdc00]);
    let valid = JsString::from_utf8("line\n😀");
    assert_eq!(format!("{valid:?}"), format!("{:?}", "line\n😀"));
    assert_eq!(
        valid.clone().into_units(),
        "line\n😀".encode_utf16().collect::<Vec<_>>()
    );
}

#[test]
fn owned_record_footprint_counts_key_value_and_container_capacities_once() {
    let mut key_units = Vec::with_capacity(11);
    key_units.push(65);
    let key = JsString::from_units(key_units);
    let mut text_units = Vec::with_capacity(13);
    text_units.push(66);
    let text = JsString::from_units(text_units);
    let mut bytes = Vec::with_capacity(17);
    bytes.push(1);
    let mut array = Vec::with_capacity(3);
    array.push(JsData::Bytes(bytes));
    let mut entries = Vec::with_capacity(5);
    entries.push((key, JsData::String(text)));
    entries.push((JsString::default(), JsData::Array(array)));
    let expected = size_of::<JsData>()
        + entries.capacity() * size_of::<(JsString, JsData)>()
        + entries[0].0.capacity_bytes()
        + match &entries[0].1 {
            JsData::String(s) => s.capacity_bytes(),
            _ => unreachable!(),
        }
        + match &entries[1].1 {
            JsData::Array(v) => {
                v.capacity() * size_of::<JsData>()
                    + match &v[0] {
                        JsData::Bytes(b) => b.capacity(),
                        _ => unreachable!(),
                    }
            }
            _ => unreachable!(),
        };
    let data = JsData::Record(entries);
    let limits = ExportLimits {
        max_depth: 3,
        max_nodes: 4,
        max_bytes: expected,
    };
    assert_eq!(data.footprint(limits).unwrap(), expected);
    assert_eq!(
        data.footprint(ExportLimits {
            max_bytes: expected - 1,
            ..limits
        })
        .unwrap_err()
        .code,
        FailureCode::Capacity
    );
    assert_eq!(
        data.footprint(ExportLimits {
            max_depth: 2,
            ..limits
        })
        .unwrap_err()
        .code,
        FailureCode::Capacity
    );
    assert_eq!(
        data.footprint(ExportLimits {
            max_nodes: 2,
            ..limits
        })
        .unwrap_err()
        .code,
        FailureCode::Capacity
    );
}

#[test]
fn empty_and_scalar_data_still_consume_a_node_and_base_allocation() {
    for data in [
        JsData::Undefined,
        JsData::Null,
        JsData::Bool(true),
        JsData::Number(f64::NAN),
        JsData::Record(Vec::new()),
        JsData::Array(Vec::new()),
    ] {
        let limits = ExportLimits {
            max_depth: 1,
            max_nodes: 1,
            max_bytes: size_of::<JsData>(),
        };
        assert_eq!(data.footprint(limits).unwrap(), size_of::<JsData>());
        assert_eq!(
            data.footprint(ExportLimits {
                max_nodes: 0,
                ..limits
            })
            .unwrap_err()
            .code,
            FailureCode::Capacity
        );
        assert_eq!(
            data.footprint(ExportLimits {
                max_bytes: size_of::<JsData>() - 1,
                ..limits
            })
            .unwrap_err()
            .code,
            FailureCode::Capacity
        );
    }
}

#[test]
fn deeply_nested_record_and_array_values_drop_iteratively() {
    let mut value = JsData::Null;
    for depth in 0..10_000 {
        value = if depth % 2 == 0 {
            JsData::Array(vec![value])
        } else {
            JsData::Record(vec![(JsString::default(), value)])
        };
    }
    assert_eq!(
        value.footprint(ExportLimits::default()).unwrap_err().code,
        FailureCode::Capacity
    );
    drop(value);
}

#[test]
fn wide_array_rejects_its_node_budget_before_expanding_the_work_stack() {
    let value = JsData::Array(vec![JsData::Null, JsData::Bool(true)]);
    let limits = ExportLimits {
        max_nodes: 2,
        ..Default::default()
    };
    assert_eq!(
        value.footprint(limits).unwrap_err().code,
        FailureCode::Capacity
    );
    assert!(
        value
            .footprint(ExportLimits {
                max_nodes: 3,
                ..limits
            })
            .is_ok()
    );
    assert!(matches!(&value, JsData::Array(values) if values.len() == 2));
}

fn unbounded_report() -> FailureReport {
    FailureReport {
        name: None,
        class: FailureClass::Script,
        code: FailureCode::ScriptException,
        message: None,
        thrown: None,
        stack: Vec::new(),
        truncated: false,
    }
}

#[test]
fn failure_bounding_preserves_utf16_and_truncates_origins_on_utf8_boundaries() {
    let mut report = unbounded_report();
    report.message = Some(JsString::from_units(vec![0xd800; 5000]));
    report.stack = vec![StackFrameReport {
        function: Some(JsString::from_units(vec![0xdc00; 300])),
        location: Some(SourceLocation {
            origin: format!("{}😀tail", "a".repeat(1023)),
            line: Some(12),
            column: Some(7),
        }),
    }];
    let bounded = report.bounded();
    assert!(bounded.truncated);
    assert_eq!(
        bounded.message.as_ref().unwrap().as_units(),
        vec![0xd800; 4096]
    );
    assert_eq!(bounded.message.as_ref().unwrap().capacity_bytes(), 8192);
    assert_eq!(
        bounded.stack[0].function.as_ref().unwrap().as_units(),
        vec![0xdc00; 256]
    );
    let location = bounded.stack[0].location.as_ref().unwrap();
    assert_eq!(location.origin, "a".repeat(1023));
    assert_eq!((location.line, location.column), (Some(12), Some(7)));
}

#[test]
fn failure_stack_has_both_a_frame_count_limit_and_total_byte_budget() {
    let mut report = unbounded_report();
    report.stack = vec![
        StackFrameReport {
            function: None,
            location: None
        };
        65
    ];
    let bounded = report.bounded();
    assert_eq!(bounded.stack.len(), 64);
    assert!(bounded.truncated);
    let mut report = unbounded_report();
    report.stack = vec![
        StackFrameReport {
            function: Some(JsString::from_utf8(&"f".repeat(256))),
            location: Some(SourceLocation {
                origin: "o".repeat(1024),
                line: None,
                column: None
            }),
        };
        64
    ];
    let bounded = report.bounded();
    assert!(bounded.truncated);
    assert!(!bounded.stack.is_empty());
    assert!(
        bounded.stack.len() < 64,
        "the byte budget must apply before the frame cap"
    );
    let frame_payload: usize = bounded
        .stack
        .iter()
        .map(|frame| {
            frame.function.as_ref().unwrap().capacity_bytes()
                + frame.location.as_ref().unwrap().origin.capacity()
        })
        .sum();
    assert!(
        size_of::<FailureReport>()
            + bounded.stack.capacity() * size_of::<StackFrameReport>()
            + frame_payload
            <= 65_536
    );
}

#[test]
fn failure_bounding_discards_oversized_thrown_data_and_keeps_small_values() {
    let mut report = unbounded_report();
    report.thrown = Some(JsData::Bytes(Vec::with_capacity(65_536)));
    let bounded = report.bounded();
    assert!(bounded.truncated);
    assert_eq!(bounded.thrown, None);
    let mut report = unbounded_report();
    report.thrown = Some(JsData::Number(73.0));
    report.stack.push(StackFrameReport {
        function: Some(JsString::from_utf8("name")),
        location: Some(SourceLocation {
            origin: String::with_capacity(4096),
            line: Some(1),
            column: None,
        }),
    });
    let bounded = report.bounded();
    assert!(!bounded.truncated);
    assert_eq!(bounded.thrown, Some(JsData::Number(73.0)));
    assert!(
        bounded.stack[0]
            .location
            .as_ref()
            .unwrap()
            .origin
            .capacity()
            < 4096
    );
}

#[test]
fn failure_display_and_checkpoint_reporting_keep_the_original_failure_class() {
    let failure = FailureReport::fault(FailureCode::Capacity, "bounded");
    assert_eq!(format!("{failure}"), "Capacity: Some(\"bounded\")");
    let mut checkpoint = CheckpointReport::empty();
    assert_eq!(checkpoint.state, CheckpointState::Empty);
    let original = FailureReport::new(
        FailureClass::Engine,
        FailureCode::EngineFailure,
        &"x".repeat(5000),
    );
    let rejected = checkpoint.push_script_failure(original).unwrap_err();
    assert_eq!(rejected.class, FailureClass::Engine);
    assert_eq!(rejected.code, FailureCode::EngineFailure);
    assert!(rejected.truncated);
    assert!(checkpoint.failures.is_empty());
    for _ in 0..66 {
        checkpoint
            .push_script_failure(FailureReport::new(
                FailureClass::Script,
                FailureCode::ScriptException,
                "exception",
            ))
            .unwrap();
    }
    assert_eq!(checkpoint.failures.len(), 64);
    assert_eq!(checkpoint.suppressed_failures, 2);
    checkpoint.suppressed_failures = u64::MAX;
    checkpoint.push_script_failure(unbounded_report()).unwrap();
    assert_eq!(checkpoint.suppressed_failures, u64::MAX);
}

#[test]
fn deferred_roots_preserve_context_and_participate_in_root_capacity() {
    let identity = EngineIdentity::new().unwrap();
    let store = ValueStore::new(
        &identity,
        ContextLimits {
            max_persistent_roots: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let deferred = store
        .with_scope(|scope| {
            let local = scope.insert(73)?;
            let deferred = scope.retain_deferred(local)?;
            assert_eq!(
                scope.retain_deferred(local).unwrap_err().code,
                FailureCode::Capacity
            );
            Ok(deferred)
        })
        .unwrap();
    assert_eq!(deferred.context(), store.context());
    assert!(format!("{deferred:?}").starts_with("RootedDeferred(RootedValue"));
    store
        .with_scope(|scope| {
            assert_eq!(*scope.read(scope.local_deferred(&deferred)?)?, 73);
            Ok(())
        })
        .unwrap();
    let other = ValueStore::<i32>::new(&identity, ContextLimits::default()).unwrap();
    other
        .with_scope(|scope| {
            assert_eq!(
                scope.local_deferred(&deferred).unwrap_err().code,
                FailureCode::WrongContext
            );
            Ok(())
        })
        .unwrap();
    drop(deferred);
    assert_eq!(store.live_roots(), 1);
    store.flush_releases().unwrap();
    assert_eq!(store.live_roots(), 0);
}

#[test]
fn foreign_scoped_values_are_rejected_before_native_access() {
    let identity = EngineIdentity::new().unwrap();
    let first = ValueStore::<i32>::new(&identity, ContextLimits::default()).unwrap();
    let second = ValueStore::<i32>::new(&identity, ContextLimits::default()).unwrap();
    first
        .with_scope(|outer| {
            let value = outer.insert(7)?;
            second.with_scope(|inner| {
                assert_eq!(
                    inner.read(value).unwrap_err().code,
                    FailureCode::WrongContext
                );
                assert_eq!(
                    inner.retain(value).unwrap_err().code,
                    FailureCode::WrongContext
                );
                Ok(())
            })
        })
        .unwrap();
}

#[test]
fn host_depth_limit_rejects_nested_entry_and_recovers_after_scope_unwind() {
    let identity = EngineIdentity::new().unwrap();
    let store = ValueStore::new(
        &identity,
        ContextLimits {
            max_host_depth: 1,
            ..Default::default()
        },
    )
    .unwrap();
    store
        .with_scope(|scope| {
            let value = scope.insert(42)?;
            assert_eq!(
                store
                    .with_scope::<()>(|_| panic!("depth-rejected body must not run"))
                    .unwrap_err()
                    .code,
                FailureCode::CallbackDepth
            );
            assert_eq!(*scope.read(value)?, 42);
            Ok(())
        })
        .unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = store.with_scope::<()>(|_| panic!("scope body panic"));
    }));
    assert!(panic.is_err());
    store
        .with_scope(|scope| scope.insert(9).map(|_| ()))
        .unwrap();
    store.close().unwrap();
    store.close().unwrap();
    assert!(store.is_closed());
    assert_eq!(
        store.flush_releases().unwrap_err().code,
        FailureCode::ClosedContext
    );
}

#[test]
fn module_metadata_sequence_is_context_local_and_identity_clones_share_generations() {
    let identity = EngineIdentity::new().unwrap();
    assert_eq!(identity.owner(), std::thread::current().id());
    let first = ValueStore::<()>::new(&identity, ContextLimits::default()).unwrap();
    let second = ValueStore::<()>::new(&identity.clone(), ContextLimits::default()).unwrap();
    assert_eq!(first.context().engine_epoch(), identity.epoch());
    assert_eq!(
        second.context().generation(),
        first.context().generation() + 1
    );
    let one = first.issue_module_id().unwrap();
    let two = first.issue_module_id().unwrap();
    let other = second.issue_module_id().unwrap();
    assert_eq!((one.slot(), two.slot(), other.slot()), (1, 2, 1));
    assert_eq!(one.context(), first.context());
    assert_eq!(other.context(), second.context());
    assert_ne!(one, other);
    let features = ModuleFeatures::default();
    assert!(!features.top_level_await && !features.dynamic_import);
}

#[test]
fn external_root_drop_after_store_teardown_does_not_reenter_released_native_values() {
    struct Native(Rc<Cell<usize>>);
    impl Drop for Native {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let drops = Rc::new(Cell::new(0));
    let identity = EngineIdentity::new().unwrap();
    let store = ValueStore::new(&identity, ContextLimits::default()).unwrap();
    let root = store
        .with_scope(|scope| scope.retain(scope.insert(Native(drops.clone()))?))
        .unwrap();
    drop(store);
    assert_eq!(drops.get(), 1);
    drop(root);
    assert_eq!(drops.get(), 1);
}
