//! Native lifecycle, resource policy, diagnostics, and trusted artifact boundaries.
#![cfg(feature = "quickjs-ng")]

use coldmind_runtime::{
    debug::DebugRequirements,
    engine::*,
    engines::quickjs_ng::{NATIVE_REVISION, QuickJsNgConfig, QuickJsNgEngine},
    scheduler::ScriptErrorPolicy,
    vm::*,
};
use std::{cell::RefCell, rc::Rc};
mod support;

fn source(text: &str) -> ScriptSource {
    ScriptSource::new(
        JsString::from_utf8(text),
        "file:///native-lifecycle.js".into(),
    )
}
fn run(
    engine: &QuickJsNgEngine,
    id: ContextId,
    mut body: impl FnMut(&dyn JsScope) -> Result<(), FailureReport>,
) -> Result<(), FailureReport> {
    engine.with_scope(id, &support::NoLoop, &mut body)
}
fn evaluate(engine: &QuickJsNgEngine, id: ContextId, program: &str) -> Result<(), FailureReport> {
    run(engine, id, |scope| {
        scope
            .source()
            .unwrap()
            .evaluate(&source(program))
            .map_err(|e| scope.detach(e))?;
        Ok(())
    })
}

#[test]
fn native_stack_overflow_keeps_catchable_range_error_semantics() {
    let config = VmConfig {
        requirements: EngineRequirements {
            debug: DebugRequirements::disabled(),
            ..Default::default()
        },
        script_error_policy: ScriptErrorPolicy::Continue,
        report_sink: Some(Rc::new(|_| {})),
        ..Default::default()
    };
    let mut vm = VirtualMachine::new(
        QuickJsNgEngine::with_config(QuickJsNgConfig {
            max_stack_bytes: 128 * 1024,
            ..Default::default()
        })
        .unwrap(),
        config,
    )
    .unwrap();
    let error = vm
        .evaluate(source("(function recurse() { return recurse(); })()"))
        .unwrap_err();
    let VmError::Execution(report) = error else {
        panic!("{error:?}")
    };
    assert_eq!(report.class, FailureClass::Script);
    assert_eq!(report.code, FailureCode::ScriptException);
    assert_eq!(vm.state(), VmState::Ready);
    vm.shutdown().unwrap();
}

#[test]
fn abort_metadata_interrupts_busy_javascript_without_crossing_native_pointers() {
    let engine = QuickJsNgEngine::new().unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    let handle = engine.interrupt_handle(id).unwrap();
    let worker = std::thread::spawn(move || handle.request_abort());
    worker.join().unwrap();
    let report = evaluate(&engine, id, "while (true) {}").unwrap_err();
    assert_eq!(report.class, FailureClass::Engine);
    engine.interrupt_handle(id).unwrap().clear();
    evaluate(&engine, id, "6 * 7").unwrap();
    engine.close_context(id).unwrap();
    assert_eq!(
        engine.interrupt_handle(id).unwrap_err().code,
        FailureCode::ClosedContext
    );
}

#[test]
fn native_heap_exhaustion_is_fatal_and_context_allocation_failure_is_typed() {
    let engine = QuickJsNgEngine::with_config(QuickJsNgConfig {
        max_memory_bytes: Some(1),
        ..Default::default()
    })
    .unwrap();
    let failure = engine.create_context(ContextLimits::default()).unwrap_err();
    assert_eq!(
        (failure.class, failure.code),
        (FailureClass::Engine, FailureCode::Capacity)
    );
    let engine = QuickJsNgEngine::with_config(QuickJsNgConfig {
        max_memory_bytes: Some(2 * 1024 * 1024),
        ..Default::default()
    })
    .unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    let report = evaluate(&engine, id, "Array(1000000).fill('x')").unwrap_err();
    assert_eq!(report.class, FailureClass::Engine);
    engine.close_context(id).unwrap();
}

#[test]
fn identities_roots_active_close_and_gc_preserve_native_lifetimes() {
    let engine = QuickJsNgEngine::new().unwrap();
    assert_eq!(engine.descriptor().native_version, NATIVE_REVISION);
    let id = engine
        .create_context(ContextLimits {
            max_persistent_roots: 1,
            ..Default::default()
        })
        .unwrap();
    let other = engine.create_context(ContextLimits::default()).unwrap();
    let foreign = QuickJsNgEngine::new().unwrap();
    assert_eq!(
        foreign.close_context(id).unwrap_err().code,
        FailureCode::WrongContext
    );
    assert_eq!(
        foreign.collect_garbage(id).unwrap_err().code,
        FailureCode::WrongContext
    );
    let mut root = None;
    run(&engine, id, |scope| {
        assert_eq!(scope.context(), id);
        let value = scope
            .import_data(JsData::Record(vec![(
                JsString::from_utf8("answer"),
                JsData::Number(42.),
            )]))
            .map_err(|e| scope.detach(e))?;
        root = Some(scope.retain(value)?);
        assert_eq!(scope.retain(value).unwrap_err().code, FailureCode::Capacity);
        assert_eq!(
            engine.close_context(id).unwrap_err().code,
            FailureCode::ActiveContext
        );
        engine.collect_garbage(id)?;
        run(&engine, id, |nested| {
            let value = nested
                .local(root.as_ref().unwrap())
                .map_err(|e| nested.detach(e))?;
            assert_eq!(nested.kind(value)?, ValueKind::Object);
            Ok(())
        })?;
        Ok(())
    })
    .unwrap();
    run(&engine, other, |scope| {
        let error = scope.local(root.as_ref().unwrap()).unwrap_err();
        assert_eq!(scope.detach(error).code, FailureCode::WrongContext);
        Ok(())
    })
    .unwrap();
    drop(root.take());
    run(&engine, id, |scope| {
        let value = scope.global().map_err(|e| scope.detach(e))?;
        root = Some(scope.retain(value)?);
        Ok(())
    })
    .unwrap();
    assert!(
        engine
            .debug_backend(id)
            .unwrap()
            .unwrap()
            .descriptor()
            .capabilities
            .contains_all(coldmind_runtime::debug::DebugCapabilities::baseline())
    );
    engine.close_context(id).unwrap();
    engine.close_context(id).unwrap();
    assert_eq!(
        engine.collect_garbage(id).unwrap_err().code,
        FailureCode::ClosedContext
    );
    assert_eq!(
        run(&engine, id, |_| Ok(())).unwrap_err().code,
        FailureCode::ClosedContext
    );
    drop(root);
    engine.close_context(other).unwrap();
}

#[test]
fn limits_validate_before_native_entry() {
    for config in [
        QuickJsNgConfig {
            max_memory_bytes: Some(0),
            ..Default::default()
        },
        QuickJsNgConfig {
            max_stack_bytes: 0,
            ..Default::default()
        },
    ] {
        assert!(matches!(
            QuickJsNgEngine::with_config(config),
            Err(FailureReport {
                code: FailureCode::Capacity,
                ..
            })
        ));
    }
    let engine = QuickJsNgEngine::new().unwrap();
    assert_eq!(
        engine
            .create_context(ContextLimits {
                max_host_depth: 0,
                ..Default::default()
            })
            .unwrap_err()
            .code,
        FailureCode::Capacity
    );
}

#[test]
fn detached_diagnostics_preserve_primitives_bound_strings_and_never_run_user_code() {
    let engine = QuickJsNgEngine::new().unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    for (program, thrown) in [
        ("throw undefined", JsData::Undefined),
        ("throw null", JsData::Null),
        ("throw true", JsData::Bool(true)),
        ("throw 42", JsData::Number(42.)),
    ] {
        let error = evaluate(&engine, id, program).unwrap_err();
        assert_eq!(error.class, FailureClass::Script);
        assert_eq!(error.thrown, Some(thrown));
    }
    let error = evaluate(&engine, id, "throw 'x'.repeat(5000)").unwrap_err();
    assert!(error.truncated);
    assert_eq!(error.message.unwrap().as_units().len(), 4096);
    for program in [
        "globalThis.touched = false; const e = new Error('bad'); Object.defineProperty(e, 'message', {get() { touched=true; throw 3; }}); Object.defineProperty(e, 'stack', {get() { touched=true; throw 4; }}); throw e;",
        "throw new Proxy({}, {get(){ touched=true; throw 3; }, getOwnPropertyDescriptor(){ touched=true; throw 4; }})",
        "throw {toString(){ touched=true; throw 5; }}",
    ] {
        let error = evaluate(&engine, id, program).unwrap_err();
        assert_eq!(error.class, FailureClass::Script);
        assert!(error.message.is_none());
        assert!(error.stack.is_empty());
    }
    run(&engine, id, |scope| {
        let value = scope
            .source()
            .unwrap()
            .evaluate(&source("touched"))
            .map_err(|e| scope.detach(e))?;
        assert_eq!(
            scope
                .export_data(value, ExportLimits::default())
                .map_err(|e| scope.detach(e))?,
            JsData::Bool(false)
        );
        Ok(())
    })
    .unwrap();
    let error = evaluate(
        &engine,
        id,
        "function failure(){throw new Error('message')}; failure()",
    )
    .unwrap_err();
    assert_eq!(error.message, Some(JsString::from_utf8("message")));
    assert!(!error.stack.is_empty());
    assert_eq!(
        error.stack[0].location.as_ref().unwrap().origin,
        "file:///native-lifecycle.js"
    );
    assert_eq!(error.stack[0].location.as_ref().unwrap().line, Some(0));
    engine.close_context(id).unwrap();
}

#[test]
fn source_and_compiler_reject_invalid_metadata_and_unsupported_policy_before_execution() {
    let engine = QuickJsNgEngine::new().unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    run(&engine, id, |scope| {
        assert!(scope.modules().is_some());
        {
            let origin = "file:///invalid\0.js";
            let error = scope
                .source()
                .unwrap()
                .evaluate(&ScriptSource::new(
                    JsString::from_utf8("throw 'should not execute'"),
                    origin.into(),
                ))
                .unwrap_err();
            assert_eq!(scope.detach(error).code, FailureCode::InvalidArtifact);
        }
        let mut options = QuickJsNgEngine::script_compile_options();
        options.fingerprint = "unsupported".into();
        assert_eq!(
            scope
                .compiler()
                .unwrap()
                .compile(&source("throw 42"), options)
                .unwrap_err()
                .code,
            FailureCode::Unsupported
        );
        let report = scope
            .compiler()
            .unwrap()
            .compile(
                &source("function {"),
                QuickJsNgEngine::script_compile_options(),
            )
            .unwrap_err();
        assert_eq!(report.class, FailureClass::Script);
        Ok(())
    })
    .unwrap();
    engine.close_context(id).unwrap();
}

#[test]
fn lossless_source_handles_astral_identifier_lone_surrogate_and_embedded_nul() {
    let engine = QuickJsNgEngine::new().unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    run(&engine, id, |scope| {
        let text = JsString::from_units(
            "const 𐐀 = '"
                .encode_utf16()
                .chain([0xd800, 0])
                .chain("'; 𐐀".encode_utf16())
                .collect(),
        );
        let value = scope
            .source()
            .unwrap()
            .evaluate(&ScriptSource::new(text, "file:///utf16.js".into()))
            .map_err(|e| scope.detach(e))?;
        assert_eq!(
            scope
                .export_data(value, ExportLimits::default())
                .map_err(|e| scope.detach(e))?,
            JsData::String(JsString::from_units(vec![0xd800, 0]))
        );
        Ok(())
    })
    .unwrap();
    engine.close_context(id).unwrap();
}

#[test]
fn engine_drop_invalidates_external_roots_without_running_pending_jobs() {
    let events = Rc::new(RefCell::new(None));
    let captured = events.clone();
    let engine = QuickJsNgEngine::new().unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    run(&engine, id, |scope| {
        let global = scope.global().map_err(|e| scope.detach(e))?;
        *captured.borrow_mut() = Some(scope.retain(global)?);
        scope
            .source()
            .unwrap()
            .evaluate(&source(
                "Promise.resolve().then(() => { throw 'must not run'; })",
            ))
            .map_err(|e| scope.detach(e))?;
        Ok(())
    })
    .unwrap();
    drop(engine);
    drop(events.borrow_mut().take());
}
