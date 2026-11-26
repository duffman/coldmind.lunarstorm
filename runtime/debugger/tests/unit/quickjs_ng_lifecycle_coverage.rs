//! Private native lifecycle guard checks.
use super::*;
use crate::vm::contract_fixture as fixtures;

#[test]
fn owner_validation_uses_metadata_without_moving_native_state_to_another_thread() {
    let other = thread::spawn(|| thread::current().id()).join().unwrap();
    assert_eq!(
        validate_owner(other).unwrap_err().code,
        FailureCode::WrongOwner
    );
    validate_owner(thread::current().id()).unwrap();
}

#[test]
fn late_fatal_fault_wins_over_script_failure_and_success_but_keeps_first_host_fault() {
    let engine = QuickJsNgEngine::new().unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    let state = engine.context(id).unwrap();
    for first in [None, Some(FailureClass::Script), Some(FailureClass::Host)] {
        let result = engine
            .with_scope(id, &fixtures::NoLoop, &mut |_: &dyn JsScope| {
                state.raw.record_fault(FailureReport::new(
                    FailureClass::Host,
                    FailureCode::HostPanic,
                    "late",
                ));
                state.raw.record_fault(FailureReport::new(
                    FailureClass::Engine,
                    FailureCode::EngineFailure,
                    "later",
                ));
                first.map_or(Ok(()), |class| {
                    Err(FailureReport::new(
                        class,
                        FailureCode::ScriptException,
                        "first",
                    ))
                })
            })
            .unwrap_err();
        assert_eq!(result.class, FailureClass::Host);
        assert_eq!(
            result.message,
            Some(JsString::from_utf8(if first == Some(FailureClass::Host) {
                "first"
            } else {
                "late"
            }))
        );
    }
    engine.close_context(id).unwrap();
}

#[test]
fn finalizer_and_closed_raw_guards_block_all_public_native_entry() {
    let engine = QuickJsNgEngine::new().unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    let state = engine.context(id).unwrap();
    state.raw.finalizing.set(1);
    assert_eq!(
        engine.close_context(id).unwrap_err().code,
        FailureCode::ActiveContext
    );
    assert_eq!(
        engine.collect_garbage(id).unwrap_err().code,
        FailureCode::ActiveContext
    );
    assert_eq!(
        engine.interrupt_handle(id).unwrap_err().code,
        FailureCode::ActiveContext
    );
    assert_eq!(
        engine.debug_backend(id).err().unwrap().code,
        FailureCode::ActiveContext
    );
    state.raw.finalizing.set(0);
    // Immediate undefined has no owned heap allocation. Keeping its wrapper
    // after close exercises the required no-native-access drop guard safely.
    // SAFETY: undefined is a context-independent immediate value.
    let value = NativeValue::new(unsafe { ffi::jbq_undefined() }, state.raw.clone());
    engine.close_context(id).unwrap();
    assert_eq!(
        state.raw.ptr().unwrap_err().code,
        FailureCode::ClosedContext
    );
    let entry = NativeEntry {
        host: &fixtures::NoLoop,
    };
    assert_eq!(
        EntryGuard::new(&state.raw, &entry).err().unwrap().code,
        FailureCode::ClosedContext
    );
    let guard = EntryGuard {
        raw: &state.raw,
        previous: std::ptr::null_mut(),
    };
    drop(guard);
    drop(value);
}
