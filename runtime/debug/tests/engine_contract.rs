use coldmind_runtime::engine::{
    ContextLimits, EngineIdentity, FailureCode, JsData, JsString, RootedValue, ScopedValue,
    ValueStore,
};
use static_assertions::assert_not_impl_any;
use std::{cell::Cell, rc::Rc};
mod support;
use coldmind_runtime::{debug::*, engine::*};

assert_not_impl_any!(RootedValue: Send, Sync, Clone, Copy);
assert_not_impl_any!(ScopedValue<'static>: Send, Sync);
assert_not_impl_any!(ValueStore<String>: Send, Sync);

#[test]
fn adapter_module_ids_are_unique_context_bound_and_unavailable_after_close() {
    let owner = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&owner, ContextLimits::default()).unwrap();
    let first = store.issue_module_id().unwrap();
    let second = store.issue_module_id().unwrap();
    assert_ne!(first, second);
    assert_eq!(first.context(), store.context());
    store.close().unwrap();
    assert_eq!(
        store.issue_module_id().unwrap_err().code,
        FailureCode::ClosedContext
    );
}

#[test]
fn impossible_root_capacity_returns_typed_error_without_panicking() {
    let owner = EngineIdentity::new().unwrap();
    let limits = ContextLimits {
        max_persistent_roots: usize::MAX,
        ..ContextLimits::default()
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ValueStore::<i32>::new(&owner, limits)
    }));
    assert!(
        result.is_ok(),
        "capacity arithmetic must be checked before allocation"
    );
    assert!(matches!(
        result.unwrap(),
        Err(FailureReport {
            code: FailureCode::Capacity,
            ..
        })
    ));
}

#[test]
fn retained_value_survives_scope_and_releases_only_at_a_safe_point() {
    let owner = EngineIdentity::new().unwrap();
    let store = ValueStore::new(&owner, ContextLimits::default()).unwrap();
    let root = store
        .with_scope(|scope| scope.retain(scope.insert("first".to_owned())?))
        .unwrap();
    assert_eq!(store.live_roots(), 1);
    store
        .with_scope(|scope| {
            let value = scope.local(&root)?;
            assert_eq!(&**scope.read(value)?, "first");
            assert_eq!(store.live_roots(), 1);
            Ok(())
        })
        .unwrap();
    drop(root);
    assert_eq!(store.live_roots(), 1, "Drop must not enter native release");
    store.flush_releases().unwrap();
    assert_eq!(store.live_roots(), 0);
}

#[test]
fn close_invalidates_external_roots_without_retaining_native_values() {
    struct DropValue(Rc<Cell<usize>>);
    impl Drop for DropValue {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let drops = Rc::new(Cell::new(0));
    let owner = EngineIdentity::new().unwrap();
    let store = ValueStore::new(&owner, ContextLimits::default()).unwrap();
    let root = store
        .with_scope(|scope| scope.retain(scope.insert(DropValue(drops.clone()))?))
        .unwrap();
    store.close().unwrap();
    assert_eq!(drops.get(), 1);
    assert_eq!(
        store
            .with_scope(|scope| scope.local(&root).map(|_| ()))
            .unwrap_err()
            .code,
        FailureCode::ClosedContext
    );
    drop(store);
    drop(root);
    assert_eq!(drops.get(), 1);
}

#[test]
fn wrong_context_and_active_close_are_rejected_without_mutation() {
    let owner = EngineIdentity::new().unwrap();
    let first = ValueStore::new(&owner, ContextLimits::default()).unwrap();
    let second = ValueStore::new(&owner, ContextLimits::default()).unwrap();
    let root = first
        .with_scope(|scope| scope.retain(scope.insert(7)?))
        .unwrap();
    second
        .with_scope(|scope| {
            assert_eq!(
                scope.local(&root).unwrap_err().code,
                FailureCode::WrongContext
            );
            assert_eq!(second.close().unwrap_err().code, FailureCode::ActiveContext);
            assert_eq!(*scope.read(scope.insert(9)?)?, 9);
            Ok(())
        })
        .unwrap();
}

#[test]
fn root_limit_is_reusable_after_deferred_release() {
    let owner = EngineIdentity::new().unwrap();
    let limits = ContextLimits {
        max_persistent_roots: 1,
        ..ContextLimits::default()
    };
    let store = ValueStore::new(&owner, limits).unwrap();
    let root = store
        .with_scope(|scope| {
            let value = scope.insert(3)?;
            let root = scope.retain(value)?;
            assert_eq!(scope.retain(value).unwrap_err().code, FailureCode::Capacity);
            Ok(root)
        })
        .unwrap();
    drop(root);
    store.flush_releases().unwrap();
    let next = store
        .with_scope(|scope| scope.retain(scope.insert(4)?))
        .unwrap();
    store
        .with_scope(|scope| {
            assert_eq!(*scope.read(scope.local(&next)?)?, 4);
            Ok(())
        })
        .unwrap();
}

#[test]
fn nested_scope_preserves_outer_arguments_and_queues_root_drops() {
    let owner = EngineIdentity::new().unwrap();
    let store = ValueStore::new(&owner, ContextLimits::default()).unwrap();
    let root = store
        .with_scope(|scope| scope.retain(scope.insert(41)?))
        .unwrap();
    store
        .with_scope(|outer| {
            let argument = outer.local(&root)?;
            store.with_scope(|inner| {
                assert_eq!(
                    inner.read(argument).unwrap_err().code,
                    FailureCode::InvalidHandle
                );
                drop(root);
                assert_eq!(
                    store.flush_releases().unwrap_err().code,
                    FailureCode::ActiveContext
                );
                assert_eq!(*inner.read(inner.insert(99)?)?, 99);
                Ok(())
            })?;
            assert_eq!(*outer.read(argument)?, 41);
            Ok(())
        })
        .unwrap();
    store.flush_releases().unwrap();
    assert_eq!(store.live_roots(), 0);
}

#[test]
fn strings_round_trip_utf16_without_lossy_conversion() {
    let units = vec![0, 0xd800, 0x0061, 0xdc00, 0xd83d, 0xde00];
    let value = JsString::from_units(units.clone());
    assert_eq!(value.as_units(), units);
    assert!(value.to_utf8().is_err());
    assert_eq!(JsString::from_utf8("a😀").to_utf8().unwrap(), "a😀");
}

#[test]
fn owned_data_limits_reject_deep_or_overallocated_payloads() {
    let limits = coldmind_runtime::engine::ExportLimits {
        max_depth: 2,
        max_nodes: 8,
        max_bytes: 512,
    };
    let deep = JsData::Array(vec![JsData::Array(vec![JsData::Array(vec![JsData::Null])])]);
    assert_eq!(
        deep.footprint(limits).unwrap_err().code,
        FailureCode::Capacity
    );
    let huge_capacity = JsData::Bytes(Vec::with_capacity(4096));
    assert_eq!(
        huge_capacity.footprint(limits).unwrap_err().code,
        FailureCode::Capacity
    );
    assert!(JsData::Number(-0.0).footprint(limits).is_ok());
}

#[test]
fn same_host_registration_reenters_three_times_without_overwriting_arguments() {
    struct Recursive {
        depth: Cell<usize>,
        returns: Rc<std::cell::RefCell<Vec<usize>>>,
    }
    impl HostFunction for Recursive {
        fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
            let depth = self.depth.get();
            let before = call.export_data(call.arguments()[0], ExportLimits::default())?;
            if depth < 3 {
                self.depth.set(depth + 1);
                let key = JsString::from_utf8("recur");
                let function = call.get(call.global()?, PropertyKey::String(&key))?;
                let argument = call.import_data(JsData::Number((depth + 1) as f64))?;
                call.call(function, None, &[argument])?;
                self.depth.set(depth);
            }
            assert_eq!(
                call.export_data(call.arguments()[0], ExportLimits::default())?,
                before
            );
            assert_eq!(call.kind(call.receiver())?, ValueKind::Undefined);
            self.returns.borrow_mut().push(depth);
            call.import_data(JsData::Number(depth as f64))
        }
    }
    for engine in support::engines() {
        let context = engine.create_context(ContextLimits::default()).unwrap();
        let returns = Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut body = |scope: &dyn JsScope| {
            let run = || -> JsResult<'_, ()> {
                let key = JsString::from_utf8("recur");
                let function = scope.host_function(
                    &key,
                    1,
                    Rc::new(Recursive {
                        depth: Cell::new(0),
                        returns: returns.clone(),
                    }),
                )?;
                scope.set(scope.global()?, PropertyKey::String(&key), function)?;
                let arg = scope.import_data(JsData::Number(-0.0))?;
                scope.call(function, None, &[arg])?;
                Ok(())
            };
            run().map_err(|e| scope.detach(e))
        };
        engine
            .with_scope(context, &support::NoLoop, &mut body)
            .unwrap();
        assert_eq!(*returns.borrow(), vec![3, 2, 1, 0]);
        engine.close_context(context).unwrap();
    }
}

#[test]
fn throwing_callbacks_preserve_primitive_exceptions_and_object_identity() {
    struct Throw;
    impl HostFunction for Throw {
        fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
            Err(ScopedError::Exception(
                call.import_data(JsData::Number(73.0))?,
            ))
        }
    }
    for engine in support::engines() {
        let context = engine.create_context(ContextLimits::default()).unwrap();
        let mut body = |scope: &dyn JsScope| {
            let run = || -> JsResult<'_, ()> {
                let function =
                    scope.host_function(&JsString::from_utf8("throw"), 0, Rc::new(Throw))?;
                let error = scope.call(function, None, &[]).unwrap_err();
                let report = scope.detach(error);
                assert_eq!(report.class, FailureClass::Script);
                assert_eq!(report.thrown, Some(JsData::Number(73.0)));
                let object = scope.import_data(JsData::Record(Vec::new()))?;
                let root = scope.retain(object)?;
                let alias = scope.local(&root)?;
                let key = JsString::from_units(vec![0xd800, 0]);
                let number = scope.import_data(JsData::Number(f64::NEG_INFINITY))?;
                scope.set(alias, PropertyKey::String(&key), number)?;
                let found = scope.get(object, PropertyKey::String(&key))?;
                assert_eq!(
                    scope.export_data(found, ExportLimits::default())?,
                    JsData::Number(f64::NEG_INFINITY)
                );
                Ok(())
            };
            run().map_err(|e| scope.detach(e))
        };
        engine
            .with_scope(context, &support::NoLoop, &mut body)
            .unwrap();
        engine.close_context(context).unwrap();
    }
}

#[test]
fn debugger_backend_metadata_outlives_context_without_retaining_engine() {
    for engine in support::engines() {
        let context = engine.create_context(ContextLimits::default()).unwrap();
        let backend = engine.debug_backend(context).unwrap().unwrap();
        assert_eq!(
            backend.service().unwrap().state,
            DebugExecutionState::Detached
        );
        engine.close_context(context).unwrap();
        drop(engine);
        assert!(backend.descriptor().capabilities.is_empty());
        assert!(matches!(backend.service(), Err(DebugError::Closed)));
    }
}

#[test]
fn nested_engine_entry_keeps_outer_scope_alive_and_defers_release() {
    for engine in support::engines() {
        let context = engine.create_context(ContextLimits::default()).unwrap();
        let mut outer = |scope: &dyn JsScope| {
            let value = scope
                .import_data(JsData::Number(37.0))
                .map_err(|e| scope.detach(e))?;
            let mut root = Some(scope.retain(value)?);
            let alias = scope
                .local(root.as_ref().unwrap())
                .map_err(|e| scope.detach(e))?;
            let mut inner = |_: &dyn JsScope| {
                drop(root.take());
                Ok(())
            };
            engine.with_scope(context, &support::NoLoop, &mut inner)?;
            assert_eq!(
                scope
                    .export_data(alias, ExportLimits::default())
                    .map_err(|e| scope.detach(e))?,
                JsData::Number(37.0)
            );
            Ok(())
        };
        engine
            .with_scope(context, &support::NoLoop, &mut outer)
            .unwrap();
        engine.close_context(context).unwrap();
    }
}

#[test]
fn fixture_object_cycles_are_rejected_and_disposed_on_close() {
    for engine in support::engines() {
        let context = engine.create_context(ContextLimits::default()).unwrap();
        let mut body = |scope: &dyn JsScope| {
            let object = scope
                .import_data(JsData::Record(Vec::new()))
                .map_err(|e| scope.detach(e))?;
            let key = JsString::from_utf8("self");
            scope
                .set(object, PropertyKey::String(&key), object)
                .map_err(|e| scope.detach(e))?;
            let error = scope
                .export_data(object, ExportLimits::default())
                .unwrap_err();
            assert_eq!(scope.detach(error).code, FailureCode::UnsupportedConversion);
            Ok(())
        };
        engine
            .with_scope(context, &support::NoLoop, &mut body)
            .unwrap();
        engine.close_context(context).unwrap();
    }
}
