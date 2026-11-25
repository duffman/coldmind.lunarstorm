//! Checked arithmetic and native resource guards without fabricated native values.
use super::super::{QuickJsNgConfig, QuickJsNgEngine};
use super::*;
use crate::engine::*;
use crate::vm::contract_fixture as fixtures;

fn live(config: QuickJsNgConfig, body: impl FnOnce(&Scope<'_, '_>)) {
    let engine = QuickJsNgEngine::with_config(config).unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    let state = engine.context(id).unwrap();
    state
        .values
        .with_scope(|values| {
            body(&Scope {
                state: &state,
                values,
                host: &fixtures::NoLoop,
            });
            Ok(())
        })
        .unwrap();
    engine.close_context(id).unwrap();
}

#[test]
fn native_array_and_byte_storage_boundaries_reject_invalid_lengths_before_access() {
    assert_eq!(array_length(0).unwrap(), 0);
    assert_eq!(
        array_length(i128::from(u32::MAX)).unwrap(),
        u32::MAX as usize
    );
    for length in [-1, i128::from(u32::MAX) + 1, i128::MAX] {
        assert_eq!(
            array_length(length).unwrap_err().code,
            FailureCode::Capacity
        );
    }
    byte_storage(0, false).unwrap();
    byte_storage(0, true).unwrap();
    byte_storage(1, true).unwrap();
    assert_eq!(
        byte_storage(1, false).unwrap_err().code,
        FailureCode::UnsupportedConversion
    );
}

#[test]
fn budgets_reject_checked_overflow_and_allocator_capacity_overflow() {
    let limits = ExportLimits {
        max_depth: usize::MAX,
        max_nodes: usize::MAX,
        max_bytes: usize::MAX,
    };
    let mut budget = Budget::new(limits).unwrap();
    assert_eq!(
        budget.nodes(usize::MAX, 1).unwrap_err().code,
        FailureCode::Capacity
    );
    assert_eq!(
        budget.add_bytes(usize::MAX).unwrap_err().code,
        FailureCode::Capacity
    );
    assert_eq!(
        budget.vector::<u64>(usize::MAX).unwrap_err().code,
        FailureCode::Capacity
    );
    // Fits usize accounting but exceeds Vec's isize::MAX allocation contract;
    // try_reserve_exact rejects it without attempting an enormous allocation.
    assert_eq!(
        budget
            .vector::<u8>(isize::MAX as usize + 1)
            .unwrap_err()
            .code,
        FailureCode::Capacity
    );
}

#[test]
fn numeric_and_array_metadata_boundaries_preserve_pending_typed_faults() {
    live(QuickJsNgConfig::default(), |scope| {
        for data in [JsData::Number(42.0), JsData::Array(Vec::new())] {
            let value = scope.import(data).unwrap();
            let expected = FailureReport::new(
                FailureClass::Host,
                FailureCode::HostPanic,
                "retained native-boundary fault",
            );
            scope.state.raw.record_fault(expected.clone());
            let Err(ScopedError::Fault(actual)) = scope.export(value, ExportLimits::default())
            else {
                panic!("typed fault must survive nonthrowing native metadata access")
            };
            assert_eq!(actual, expected);
        }
        for expected in [true, false] {
            let value = scope.import(JsData::Bool(expected)).unwrap();
            assert_eq!(
                scope.export(value, ExportLimits::default()).unwrap(),
                JsData::Bool(expected)
            );
        }
    });
}

#[test]
fn dense_array_getters_propagate_the_original_thrown_primitive() {
    live(QuickJsNgConfig::default(), |scope| {
        let value = scope
            .source()
            .unwrap()
            .evaluate(&ScriptSource::new(
                JsString::from_utf8("Object.defineProperty([1], '0', {get(){throw 73}})"),
                "file:///data-guard.js".into(),
            ))
            .unwrap();
        let Err(ScopedError::Exception(thrown)) = scope.export(value, ExportLimits::default())
        else {
            panic!("array getter must preserve its native exception")
        };
        assert_eq!(
            scope.export(thrown, ExportLimits::default()).unwrap(),
            JsData::Number(73.0)
        );
    });
}

#[test]
fn native_heap_limits_release_partial_array_record_and_key_imports() {
    for kind in 0..3 {
        live(
            QuickJsNgConfig {
                max_memory_bytes: Some(128 * 1024),
                ..Default::default()
            },
            |scope| {
                let data = match kind {
                    0 => JsData::Array((0..32_768).map(|_| JsData::Null).collect()),
                    1 => JsData::Record(
                        (0..4096)
                            .map(|index| {
                                (
                                    JsString::from_utf8(&format!("property-{index}")),
                                    JsData::Null,
                                )
                            })
                            .collect(),
                    ),
                    2 => JsData::Record(vec![(
                        JsString::from_units(vec![0xd800; 100_000]),
                        JsData::Null,
                    )]),
                    _ => unreachable!(),
                };
                native_oom(scope.import(data));
                // Fatal exhaustion ends this context; partial guards unwind
                // before live() closes it, without another native entry.
            },
        );
    }
}

fn native_oom(result: JsResult<'_, ScopedValue<'_>>) {
    let Err(ScopedError::Fault(error)) = result else {
        panic!("expected native heap exhaustion: {result:?}")
    };
    assert_eq!(error.class, FailureClass::Engine);
    assert_eq!(error.code, FailureCode::EngineFailure);
}

#[test]
fn native_root_constructors_and_own_key_snapshots_fail_under_an_exhausted_budget() {
    for kind in 0..5 {
        live(QuickJsNgConfig::default(), |scope| {
            let record = if kind == 4 {
                Some(
                    scope
                        .import(JsData::Record(vec![(
                            JsString::from_utf8("x"),
                            JsData::Null,
                        )]))
                        .unwrap(),
                )
            } else {
                None
            };
            // SAFETY: This owner-local scope keeps its native context live. The
            // intrinsic memory policy is changed before the operation under test.
            unsafe { ffi::jbq_set_memory_limit(scope.ptr(), 1) };
            if let Some(value) = record {
                let Err(ScopedError::Fault(error)) = scope.export(value, ExportLimits::default())
                else {
                    panic!("snapshot allocation must fail")
                };
                assert_eq!(
                    (error.class, error.code),
                    (FailureClass::Engine, FailureCode::EngineFailure)
                );
            } else {
                let input = match kind {
                    0 => JsData::Array(Vec::new()),
                    1 => JsData::Record(Vec::new()),
                    2 => JsData::String(JsString::from_utf8("new native string")),
                    3 => JsData::Bytes(vec![1]),
                    _ => unreachable!(),
                };
                native_oom(scope.import(input));
            }
            // Fatal exhaustion ends this context without another engine entry.
        });
    }
}

#[test]
fn record_property_definition_propagates_native_allocation_failures() {
    for offset in (0..4096).step_by(64) {
        live(
            QuickJsNgConfig {
                max_memory_bytes: Some(128 * 1024 + offset),
                ..Default::default()
            },
            |scope| {
                let input = JsData::Record(
                    (0..4096)
                        .map(|index| (JsString::from_utf8(&index.to_string()), JsData::Null))
                        .collect(),
                );
                native_oom(scope.import(input));
            },
        );
    }
}

#[test]
fn native_property_and_numeric_snapshot_key_guards_propagate_allocation_failure() {
    live(QuickJsNgConfig::default(), |scope| {
        let parent = scope
            .import(JsData::Record(
                (0..19)
                    .map(|index| {
                        (
                            JsString::from_utf8(&format!("existing-{index}")),
                            JsData::Null,
                        )
                    })
                    .collect(),
            ))
            .unwrap();
        let key = scope
            .import(JsData::String(JsString::from_utf8("new-property")))
            .unwrap();
        let value = scope.import(JsData::Null).unwrap();
        let parent = scope.read(parent).unwrap();
        let key = scope.read(key).unwrap();
        let value = scope.read(value).unwrap();
        // SAFETY: All three actual native values were created before changing
        // this live owner context's memory policy.
        unsafe { ffi::jbq_set_memory_limit(scope.ptr(), 1) };
        let Err(ScopedError::Fault(error)) = scope.define_import_key(&parent, &key, &value) else {
            panic!("native property definition must exhaust the heap")
        };
        assert_eq!(
            (error.class, error.code),
            (FailureClass::Engine, FailureCode::EngineFailure)
        );
    });
    live(QuickJsNgConfig::default(), |scope| {
        let value = scope
            .import(JsData::Record(vec![(
                JsString::from_utf8("123"),
                JsData::Null,
            )]))
            .unwrap();
        let value = scope.read(value).unwrap();
        let mut length = 0;
        // SAFETY: The protected native record is ordinary and this context is
        // live; immediately guard the real native snapshot under normal memory.
        let raw = unsafe { ffi::jbq_own_keys(scope.ptr(), value.value, &mut length) };
        assert!(!raw.is_null());
        let keys = Keys {
            context: scope.ptr(),
            raw,
            length,
        };
        assert_eq!(keys.length, 1);
        // SAFETY: The snapshot was allocated before this limit. Converting its
        // tagged numeric atom now requires a fresh native primitive string.
        unsafe { ffi::jbq_set_memory_limit(scope.ptr(), 1) };
        let Err(ScopedError::Fault(error)) = scope.snapshot_key(&keys, 0) else {
            panic!("numeric atom conversion must exhaust the heap")
        };
        assert_eq!(
            (error.class, error.code),
            (FailureClass::Engine, FailureCode::EngineFailure)
        );
    });
}
