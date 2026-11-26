//! Phase 9: the public facade owns one bridge/context and uses scheduler turns.
mod support;
use coldmind_runtime::{debug::DebugRequirements, engine::*, scheduler::ScriptErrorPolicy, vm::*};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use support::{Boxed, Inline, RustFixture};
fn config() -> VmConfig {
    VmConfig {
        requirements: EngineRequirements {
            entry: EntryRequirement::ExistingCallables,
            debug: DebugRequirements::disabled(),
            ..Default::default()
        },
        ..Default::default()
    }
}
assert_not_impl_any!(VirtualMachine<RustFixture<Inline>>:Send,Sync);
assert_impl_all!(VmHandle:Send,Sync,Clone);
struct Sum;
impl HostFunction for Sum {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let sum = call
            .arguments()
            .iter()
            .map(
                |&arg| match call.export_data(arg, ExportLimits::default()).unwrap() {
                    JsData::Number(value) => value,
                    _ => 0.0,
                },
            )
            .sum();
        call.import_data(JsData::Number(sum))
    }
}
fn number<E: JsEngine>(vm: &mut VirtualMachine<E>, value: f64) -> RootedValue {
    let result = Rc::new(RefCell::new(None));
    let captured = result.clone();
    vm.execute(Box::new(move |scope: &dyn JsScope| {
        let value = scope
            .import_data(JsData::Number(value))
            .map_err(|e| scope.detach(e))?;
        *captured.borrow_mut() = Some(scope.retain(value)?);
        Ok(())
    }))
    .unwrap();
    result.borrow_mut().take().unwrap()
}
fn exported<E: JsEngine>(vm: &mut VirtualMachine<E>, root: RootedValue) -> JsData {
    let result = Rc::new(RefCell::new(None));
    let captured = result.clone();
    vm.execute(Box::new(move |scope: &dyn JsScope| {
        let value = scope.local(&root).map_err(|e| scope.detach(e))?;
        *captured.borrow_mut() = Some(
            scope
                .export_data(value, ExportLimits::default())
                .map_err(|e| scope.detach(e))?,
        );
        Ok(())
    }))
    .unwrap();
    result.borrow_mut().take().unwrap()
}
#[test]
fn generic_and_erased_engines_use_the_same_vm_binding_call_and_capture_api() {
    fn run<E: JsEngine>(engine: E) {
        let mut vm = VirtualMachine::new(engine, config()).unwrap();
        assert_eq!(vm.state(), VmState::Ready);
        let function = vm
            .bind_function(JsString::from_utf8("sum"), 2, Rc::new(Sum))
            .unwrap();
        let args = vec![number(&mut vm, 17.0), number(&mut vm, 25.0)];
        let result = vm.call(&function, None, &args).unwrap();
        assert_eq!(exported(&mut vm, result), JsData::Number(42.0));
        vm.shutdown().unwrap();
        assert_eq!(vm.state(), VmState::Stopped);
        vm.shutdown().unwrap();
    }
    run(RustFixture::<Inline>::new("inline-vm"));
    run(RustFixture::<Boxed>::new("boxed-vm"));
    for engine in support::engines() {
        run(engine);
    }
}
#[test]
fn default_development_requirements_fail_before_application_and_close_context() {
    let engine = RustFixture::<Inline>::new("default-reject");
    let probe = engine.probe();
    assert!(VirtualMachine::new(engine, VmConfig::default()).is_err());
    assert_eq!(probe.contexts_created(), 1);
    assert_eq!(probe.contexts_closed(), 1);
}
#[test]
fn unavailable_source_and_compile_paths_reject_without_faulting_healthy_vm() {
    let mut vm = VirtualMachine::new(RustFixture::<Inline>::new("optional"), config()).unwrap();
    assert_eq!(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8("1"),
            "test.js".into()
        ))
        .unwrap_err(),
        VmError::Unsupported
    );
    let source = ScriptSource::new(JsString::from_utf8("1"), "test.js".into());
    let rejected = vm.submit(source).unwrap_err();
    assert_eq!(rejected.value.text.as_units(), &[49]);
    assert_eq!(rejected.reason, RejectionReason::Unsupported);
    assert_eq!(vm.state(), VmState::Ready);
    assert_eq!(
        vm.compile(
            ScriptSource::new(JsString::from_utf8("1"), "test.js".into()),
            CompileOptions {
                mode: CodeMode::Script,
                debug_info: DebugInfo::Required,
                fingerprint: "test".into()
            }
        )
        .unwrap_err(),
        VmError::Unsupported
    );
}
#[test]
fn stop_control_closes_before_returning_any_new_successful_capture() {
    let mut vm = VirtualMachine::new(RustFixture::<Inline>::new("stop-vm"), config()).unwrap();
    let handle = vm.handle();
    std::thread::spawn(move || handle.request_stop().unwrap())
        .join()
        .unwrap();
    assert_eq!(vm.state(), VmState::Stopping);
    assert_eq!(
        vm.execute(Box::new(|_: &dyn JsScope| panic!("must not enter")))
            .unwrap_err(),
        VmError::Stopped
    );
    assert_eq!(vm.state(), VmState::Stopped);
}
#[test]
fn synchronous_script_errors_return_once_and_continue_keeps_context_live() {
    let reports = Rc::new(Cell::new(0));
    let calls = reports.clone();
    let mut cfg = config();
    cfg.script_error_policy = ScriptErrorPolicy::Continue;
    cfg.report_sink = Some(Rc::new(move |_| calls.set(calls.get() + 1)));
    let mut vm = VirtualMachine::new(RustFixture::<Inline>::new("continue-vm"), cfg).unwrap();
    let report = FailureReport::new(FailureClass::Script, FailureCode::ScriptException, "script");
    let error = report.clone();
    assert_eq!(
        vm.execute(Box::new(move |_: &dyn JsScope| Err(error.clone())))
            .unwrap_err(),
        VmError::Execution(report)
    );
    assert_eq!(reports.get(), 0);
    assert_eq!(vm.state(), VmState::Ready);
    let _ = number(&mut vm, 1.0);
}
#[test]
fn foreign_root_rejection_precedes_entry_and_preserves_both_vms() {
    let mut first = VirtualMachine::new(RustFixture::<Inline>::new("first"), config()).unwrap();
    let mut second = VirtualMachine::new(RustFixture::<Inline>::new("second"), config()).unwrap();
    let function = first
        .bind_function(JsString::from_utf8("sum"), 0, Rc::new(Sum))
        .unwrap();
    assert_eq!(
        second.call(&function, None, &[]).unwrap_err(),
        VmError::InvalidHandle
    );
    assert_eq!(first.state(), VmState::Ready);
    assert_eq!(second.state(), VmState::Ready);
}

type RustBody = dyn Fn(&dyn HostCall) -> Result<JsData, FailureReport>;
struct RustCallback(Box<RustBody>);
impl HostFunction for RustCallback {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        call.import_data((self.0)(call)?)
    }
}
fn post_rust(call: &dyn HostCall, function: Rc<dyn HostFunction>) -> Result<TaskId, FailureReport> {
    let local = call
        .host_function(&JsString::from_utf8("queued-rust"), 0, function)
        .map_err(|error| call.detach(error))?;
    call.host()
        .post(CallbackJob {
            function: call.retain(local)?,
            receiver: None,
            arguments: Vec::new(),
        })
        .map_err(|_| FailureReport::fault(FailureCode::Capacity, "test callback admission failed"))
}
fn source(name: &str) -> ScriptSource {
    ScriptSource::new(JsString::from_utf8(name), format!("rust-fixture://{name}"))
}
fn options() -> CompileOptions {
    CompileOptions {
        mode: CodeMode::Script,
        debug_info: DebugInfo::Required,
        fingerprint: "rust-program-v1".into(),
    }
}

#[test]
fn invalid_vm_configuration_never_creates_a_context() {
    for field in 0..14 {
        let engine = RustFixture::<Inline>::new("invalid-config");
        let probe = engine.probe();
        let mut cfg = config();
        match field {
            0 => cfg.scheduler.max_jobs = 0,
            1 => cfg.context.max_host_depth = 0,
            2 => cfg.debug_limits.requests = 0,
            3 => cfg.debug_limits.updates = 0,
            4 => cfg.debug_limits.request_bytes = 0,
            5 => cfg.debug_limits.reply_bytes = 0,
            6 => cfg.debug_limits.inspection_objects = 0,
            7 => cfg.debug_limits.scripts = 0,
            8 => cfg.debug_limits.source_bytes = 0,
            9 => cfg.debug_limits.breakpoints = 0,
            10 => cfg.debug_limits.condition_bytes = 0,
            11 => cfg.debug_limits.expression_units = 0,
            12 => cfg.debug_limits.dap_output_bytes = 0,
            _ => cfg.script_error_policy = ScriptErrorPolicy::Continue,
        }
        assert!(matches!(
            VirtualMachine::new(engine, cfg),
            Err(VmError::Configuration(_))
        ));
        assert_eq!(probe.contexts_created(), 0);
        assert_eq!(probe.contexts_closed(), 0);
    }
}

#[test]
fn synchronous_call_keeps_fifo_and_finishes_pending_checkpoint_before_newer_work() {
    let engine = RustFixture::<Inline>::new("fifo-capture");
    let probe = engine.probe();
    let mut cfg = config();
    cfg.scheduler.microtask_hint = std::num::NonZeroUsize::new(1).unwrap();
    let mut vm = VirtualMachine::new(engine, cfg).unwrap();
    let trace = Rc::new(RefCell::new(Vec::new()));
    let events = trace.clone();
    let checkpoints = probe.clone();
    let target = vm
        .bind_function(
            JsString::from_utf8("target"),
            0,
            Rc::new(RustCallback(Box::new(move |call| {
                events.borrow_mut().push("target");
                let newer = events.clone();
                post_rust(
                    call,
                    Rc::new(RustCallback(Box::new(move |_| {
                        newer.borrow_mut().push("newer");
                        Ok(JsData::Undefined)
                    }))),
                )?;
                for label in ["checkpoint-1", "checkpoint-2"] {
                    let events = events.clone();
                    checkpoints.queue_checkpoint(move |_| {
                        events.borrow_mut().push(label);
                        Ok(())
                    })?;
                }
                Ok(JsData::Number(42.0))
            }))),
        )
        .unwrap();
    let events = trace.clone();
    let seed = vm
        .bind_function(
            JsString::from_utf8("seed"),
            0,
            Rc::new(RustCallback(Box::new(move |call| {
                let events = events.clone();
                post_rust(
                    call,
                    Rc::new(RustCallback(Box::new(move |_| {
                        events.borrow_mut().push("older");
                        Ok(JsData::Undefined)
                    }))),
                )?;
                Ok(JsData::Undefined)
            }))),
        )
        .unwrap();
    drop(vm.call(&seed, None, &[]).unwrap());
    assert!(trace.borrow().is_empty());
    let result = vm.call(&target, None, &[]).unwrap();
    assert_eq!(
        *trace.borrow(),
        ["older", "target", "checkpoint-1", "checkpoint-2"]
    );
    assert_eq!(vm.state(), VmState::Ready);
    vm.tick(false).unwrap();
    assert_eq!(
        *trace.borrow(),
        ["older", "target", "checkpoint-1", "checkpoint-2", "newer"]
    );
    assert_eq!(exported(&mut vm, result), JsData::Number(42.0));
}

#[test]
fn named_rust_source_compile_and_artifact_commands_share_fifo_and_real_root_lifetimes() {
    // Exact named Rust programs exercise orchestration, not JS or foreign bytecode.
    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    let program: Rc<dyn HostFunction> = Rc::new(RustCallback(Box::new(move |_| {
        observed.set(observed.get() + 1);
        Ok(JsData::Number(observed.get() as f64))
    })));
    let engine = RustFixture::<Inline>::new("named-program-vm")
        .with_programs(vec![(source("counter"), program)]);
    let probe = engine.probe();
    let mut vm = VirtualMachine::new(engine, config()).unwrap();
    let first = vm.evaluate(source("counter")).unwrap();
    assert_eq!(exported(&mut vm, first), JsData::Number(1.0));
    vm.submit(source("counter")).unwrap();
    let artifact = vm.compile(source("counter"), options()).unwrap();
    assert_eq!(
        calls.get(),
        2,
        "older submitted program executes before compilation capture"
    );
    assert_eq!(probe.compile_calls(), 1);
    assert_eq!(
        artifact.metadata().source_digest,
        SourceDigest::of_utf16(&source("counter").text)
    );
    let result = vm.execute_artifact(artifact).unwrap();
    assert_eq!(exported(&mut vm, result), JsData::Number(3.0));
    let queued = vm.submit(source("counter")).unwrap();
    vm.handle().request_cancel(queued).unwrap();
    vm.execute(Box::new(|_: &dyn JsScope| Ok(()))).unwrap();
    assert_eq!(
        calls.get(),
        3,
        "canceled source never enters its Rust program"
    );
    assert_eq!(vm.state(), VmState::Ready);
}

struct FillMessages {
    calls: Rc<Cell<usize>>,
    count: usize,
}
impl HostMessageHandler for FillMessages {
    fn deliver<'s>(
        &self,
        scope: &'s dyn JsScope,
        host: &dyn HostServices,
        _: JsData,
    ) -> JsResult<'s, ()> {
        for _ in 0..self.count {
            let calls = self.calls.clone();
            let local = scope.host_function(
                &JsString::from_utf8("filler"),
                0,
                Rc::new(RustCallback(Box::new(move |_| {
                    calls.set(calls.get() + 1);
                    Ok(JsData::Undefined)
                }))),
            )?;
            host.post(CallbackJob {
                function: scope.retain(local)?,
                receiver: None,
                arguments: Vec::new(),
            })
            .map_err(|_| FailureReport::fault(FailureCode::Capacity, "filler admission failed"))?;
        }
        Ok(())
    }
}

#[test]
fn full_command_admission_preserves_ready_state_and_submit_buffer_ownership() {
    let calls = Rc::new(Cell::new(0));
    let mut cfg = config();
    cfg.scheduler.max_jobs = 2;
    cfg.scheduler.task_batch = 1;
    cfg.message_handler = Some(Rc::new(FillMessages {
        calls: calls.clone(),
        count: 2,
    }));
    let program: Rc<dyn HostFunction> =
        Rc::new(RustCallback(Box::new(|_| Ok(JsData::Number(42.0)))));
    let mut vm = VirtualMachine::new(
        RustFixture::<Inline>::new("full-vm").with_programs(vec![(source("constant"), program)]),
        cfg,
    )
    .unwrap();
    vm.handle().try_send(JsData::Null).unwrap();
    vm.tick(false).unwrap();
    assert_eq!(calls.get(), 0);
    assert_eq!(
        vm.execute(Box::new(|_: &dyn JsScope| panic!(
            "rejected command must not run"
        )))
        .unwrap_err(),
        VmError::Admission(RejectionReason::Full)
    );
    let input = source("constant");
    let pointer = input.text.as_units().as_ptr();
    let rejected = vm.submit(input).unwrap_err();
    assert_eq!(rejected.reason, RejectionReason::Full);
    assert_eq!(rejected.value.text.as_units().as_ptr(), pointer);
    assert_eq!(vm.state(), VmState::Ready);
    assert!(vm.last_failure().is_none());
    vm.tick(false).unwrap();
    assert_eq!(calls.get(), 1);
    vm.execute(Box::new(|_: &dyn JsScope| Ok(()))).unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(vm.state(), VmState::Ready);
}

#[test]
fn private_call_leases_do_not_consume_extra_persistent_root_permits() {
    let engine = RustFixture::<Inline>::new("private-root-leases");
    let probe = engine.probe();
    let mut cfg = config();
    cfg.context.max_persistent_roots = 3;
    let mut vm = VirtualMachine::new(engine, cfg).unwrap();
    let function = vm
        .bind_function(JsString::from_utf8("sum"), 1, Rc::new(Sum))
        .unwrap();
    let argument = number(&mut vm, 42.0);
    let arguments = [argument];
    let result = vm.call(&function, None, &arguments).unwrap();
    assert_eq!(probe.live_roots(), Some(3));
    assert_eq!(exported(&mut vm, result), JsData::Number(42.0));
    assert_eq!(probe.live_roots(), Some(2));
}

struct StartPromise(Rc<RefCell<Option<CompletionTicket>>>);
impl HostFunction for StartPromise {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let deferred = call.deferred()?;
        let reservation = call.host().reserve_operation(4096)?;
        *self.0.borrow_mut() = Some(call.host().bind_deferred(
            reservation,
            deferred.resolver,
            None,
        )?);
        Ok(deferred.promise)
    }
}

#[test]
fn returned_promise_is_not_implicitly_awaited_and_settles_only_after_later_pump() {
    let engine = RustFixture::<Inline>::new("unawaited-promise");
    let probe = engine.probe();
    let mut vm = VirtualMachine::new(engine, config()).unwrap();
    let ticket = Rc::new(RefCell::new(None));
    let function = vm
        .bind_function(
            JsString::from_utf8("start"),
            0,
            Rc::new(StartPromise(ticket.clone())),
        )
        .unwrap();
    let promise = vm.call(&function, None, &[]).unwrap();
    assert_eq!(probe.promise(&promise).unwrap(), None);
    let ticket = ticket.borrow_mut().take().unwrap();
    assert!(matches!(
            std::thread::spawn(
                move || ticket.complete(CompletionPayload::Success(JsData::Number(7.0)))
            )
            .join()
            .unwrap(),
            PublishOutcome::Published
        ));
    assert_eq!(probe.promise(&promise).unwrap(), None);
    vm.tick(false).unwrap();
    assert_eq!(
        probe.promise(&promise).unwrap(),
        Some((false, JsData::Number(7.0)))
    );
}

#[test]
fn captured_checkpoint_failure_returns_once_after_pending_work_finishes() {
    let engine = RustFixture::<Inline>::new("checkpoint-capture");
    let probe = engine.probe();
    let reports = Rc::new(Cell::new(0));
    let seen = reports.clone();
    let mut cfg = config();
    cfg.scheduler.microtask_hint = std::num::NonZeroUsize::new(1).unwrap();
    cfg.script_error_policy = ScriptErrorPolicy::Continue;
    cfg.report_sink = Some(Rc::new(move |_| seen.set(seen.get() + 1)));
    let mut vm = VirtualMachine::new(engine, cfg).unwrap();
    let completed = Rc::new(Cell::new(false));
    let checkpoint_done = completed.clone();
    let checkpoint = probe.clone();
    let function = vm
        .bind_function(
            JsString::from_utf8("checkpoint"),
            0,
            Rc::new(RustCallback(Box::new(move |_| {
                checkpoint.queue_checkpoint(|_| {
                    Err(FailureReport::new(
                        FailureClass::Script,
                        FailureCode::ScriptException,
                        "checkpoint failure",
                    ))
                })?;
                let completed = checkpoint_done.clone();
                checkpoint.queue_checkpoint(move |_| {
                    completed.set(true);
                    Ok(())
                })?;
                Ok(JsData::Number(42.0))
            }))),
        )
        .unwrap();
    let error = vm.call(&function, None, &[]).unwrap_err();
    assert!(matches!(
        error,
        VmError::Execution(FailureReport {
            class: FailureClass::Script,
            ..
        })
    ));
    assert!(completed.get());
    assert_eq!(reports.get(), 0);
    assert_eq!(vm.state(), VmState::Ready);
    vm.execute(Box::new(|_: &dyn JsScope| Ok(()))).unwrap();
    assert_eq!(
        probe.live_roots(),
        Some(1),
        "discarded successful capture releases its private result"
    );
}

#[test]
fn stop_during_a_pending_capture_discards_the_result_and_later_checkpoint_work() {
    let engine = RustFixture::<Inline>::new("checkpoint-stop");
    let probe = engine.probe();
    let mut cfg = config();
    cfg.scheduler.microtask_hint = std::num::NonZeroUsize::new(1).unwrap();
    let mut vm = VirtualMachine::new(engine, cfg).unwrap();
    let later = Rc::new(Cell::new(false));
    let after = later.clone();
    let checkpoint = probe.clone();
    let function = vm
        .bind_function(
            JsString::from_utf8("stop"),
            0,
            Rc::new(RustCallback(Box::new(move |_| {
                checkpoint.queue_checkpoint(|host| host.request_stop())?;
                let later = after.clone();
                checkpoint.queue_checkpoint(move |_| {
                    later.set(true);
                    Ok(())
                })?;
                Ok(JsData::Number(42.0))
            }))),
        )
        .unwrap();
    assert_eq!(vm.call(&function, None, &[]).unwrap_err(), VmError::Stopped);
    assert!(!later.get());
    assert_eq!(vm.state(), VmState::Stopped);
    assert_eq!(probe.contexts_closed(), 1);
}

#[test]
fn foreign_receiver_and_arguments_are_rejected_without_a_checkpoint_or_side_effect() {
    let engine = RustFixture::<Inline>::new("root-owner");
    let probe = engine.probe();
    let mut vm = VirtualMachine::new(engine, config()).unwrap();
    let mut other =
        VirtualMachine::new(RustFixture::<Inline>::new("foreign-owner"), config()).unwrap();
    let function = vm
        .bind_function(JsString::from_utf8("sum"), 1, Rc::new(Sum))
        .unwrap();
    let foreign = number(&mut other, 9.0);
    let before = probe.checkpoint_calls();
    assert_eq!(
        vm.call(&function, Some(&foreign), &[]).unwrap_err(),
        VmError::InvalidHandle
    );
    assert_eq!(
        vm.call(&function, None, &[foreign]).unwrap_err(),
        VmError::InvalidHandle
    );
    assert_eq!(probe.checkpoint_calls(), before);
    assert_eq!(
        (vm.state(), other.state()),
        (VmState::Ready, VmState::Ready)
    );
}

#[test]
fn scoped_body_panic_is_detached_and_vm_does_not_remain_running() {
    let engine = RustFixture::<Inline>::new("panic-owner");
    let probe = engine.probe();
    let mut vm = VirtualMachine::new(engine, config()).unwrap();
    let error = vm
        .execute(Box::new(|_: &dyn JsScope| panic!("scoped Rust body panic")))
        .unwrap_err();
    assert!(matches!(
        error,
        VmError::Execution(FailureReport {
            code: FailureCode::HostPanic,
            ..
        })
    ));
    assert_eq!(vm.state(), VmState::Faulted);
    assert_eq!(probe.contexts_closed(), 1);
    assert_eq!(
        vm.execute(Box::new(|_: &dyn JsScope| Ok(()))).unwrap_err(),
        VmError::Faulted
    );
    assert_eq!(vm.last_failure().unwrap().code, FailureCode::HostPanic);
    assert!(
        error.to_string().contains("HostPanic"),
        "the detached user diagnostic survives context teardown"
    );
    vm.shutdown().unwrap();
    assert_eq!(probe.contexts_closed(), 1);
}

struct CollectMessages(Rc<RefCell<Vec<JsData>>>);
impl HostMessageHandler for CollectMessages {
    fn deliver<'s>(
        &self,
        _: &'s dyn JsScope,
        _: &dyn HostServices,
        payload: JsData,
    ) -> JsResult<'s, ()> {
        self.0.borrow_mut().push(payload);
        Ok(())
    }
}

#[test]
fn producer_bounds_full_and_closed_rejections_preserve_payloads_without_handler_entry() {
    let observed = Rc::new(RefCell::new(Vec::new()));
    let mut cfg = config();
    cfg.scheduler.max_messages = 1;
    cfg.context.export.max_bytes = 128;
    cfg.message_handler = Some(Rc::new(CollectMessages(observed.clone())));
    let mut vm = VirtualMachine::new(RustFixture::<Inline>::new("producer-vm"), cfg).unwrap();
    let mut oversized = Vec::with_capacity(4096);
    oversized.push(5);
    let pointer = oversized.as_ptr();
    let rejected = vm.handle().try_send(JsData::Bytes(oversized)).unwrap_err();
    assert!(matches!(
        rejected.reason,
        RejectionReason::InvalidData(FailureReport {
            code: FailureCode::Capacity,
            ..
        })
    ));
    assert!(
        matches!(&rejected.value, JsData::Bytes(bytes) if bytes.as_ptr() == pointer && bytes == &[5])
    );
    let handle = vm.handle();
    std::thread::spawn(move || {
        handle.try_send(JsData::Number(1.0)).unwrap();
    })
    .join()
    .unwrap();
    assert!(observed.borrow().is_empty());
    let bytes = vec![7, 8];
    let pointer = bytes.as_ptr();
    let rejected = vm.handle().try_send(JsData::Bytes(bytes)).unwrap_err();
    assert_eq!(rejected.reason, RejectionReason::Full);
    assert!(
        matches!(&rejected.value, JsData::Bytes(bytes) if bytes.as_ptr() == pointer && bytes == &[7, 8])
    );
    vm.tick(false).unwrap();
    assert_eq!(*observed.borrow(), [JsData::Number(1.0)]);
    vm.shutdown().unwrap();
    assert_eq!(
        vm.handle().try_send(JsData::Null).unwrap_err().reason,
        RejectionReason::Closed
    );
    let engine = RustFixture::<Inline>::new("no-handler-vm");
    let probe = engine.probe();
    let vm = VirtualMachine::new(engine, config()).unwrap();
    let rejected = vm.handle().try_send(JsData::Number(9.0)).unwrap_err();
    assert_eq!(rejected.reason, RejectionReason::Unsupported);
    assert_eq!(rejected.value, JsData::Number(9.0));
    assert_eq!(probe.checkpoint_calls(), 0);
}

struct AliasReceiver;
impl HostFunction for AliasReceiver {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let key = JsString::from_units(vec![0xd800]);
        call.set(
            call.receiver(),
            PropertyKey::String(&key),
            call.import_data(JsData::Number(42.0))?,
        )?;
        Ok(call.arguments()[0])
    }
}

#[test]
fn borrowed_receiver_argument_and_returned_root_keep_native_object_identity() {
    let mut vm = VirtualMachine::new(RustFixture::<Inline>::new("identity-vm"), config()).unwrap();
    let function = vm
        .bind_function(JsString::from_utf8("alias"), 1, Rc::new(AliasReceiver))
        .unwrap();
    let root = Rc::new(RefCell::new(None));
    let output = root.clone();
    vm.execute(Box::new(move |scope: &dyn JsScope| {
        let object = scope
            .import_data(JsData::Record(Vec::new()))
            .map_err(|error| scope.detach(error))?;
        *output.borrow_mut() = Some(scope.retain(object)?);
        Ok(())
    }))
    .unwrap();
    let mut arguments = vec![root.borrow_mut().take().unwrap()];
    let result = vm.call(&function, Some(&arguments[0]), &arguments).unwrap();
    vm.execute(Box::new(move |scope: &dyn JsScope| {
        let alias = scope.local(&result).map_err(|error| scope.detach(error))?;
        let key = JsString::from_units(vec![0xd800]);
        let value = scope
            .import_data(JsData::Number(99.0))
            .map_err(|error| scope.detach(error))?;
        scope
            .set(alias, PropertyKey::String(&key), value)
            .map_err(|error| scope.detach(error))?;
        Ok(())
    }))
    .unwrap();
    assert_eq!(
        exported(&mut vm, arguments.pop().unwrap()),
        JsData::Record(vec![(
            JsString::from_units(vec![0xd800]),
            JsData::Number(99.0)
        )])
    );
}

#[test]
fn result_root_capacity_failure_keeps_side_effects_and_never_retries_the_call() {
    let engine = RustFixture::<Inline>::new("retention-failure");
    let probe = engine.probe();
    let mut cfg = config();
    cfg.context.max_persistent_roots = 2;
    let mut vm = VirtualMachine::new(engine, cfg).unwrap();
    let entered = Rc::new(Cell::new(0));
    let calls = entered.clone();
    let function = vm
        .bind_function(
            JsString::from_utf8("effect"),
            1,
            Rc::new(RustCallback(Box::new(move |_| {
                calls.set(calls.get() + 1);
                Ok(JsData::Number(42.0))
            }))),
        )
        .unwrap();
    let argument = number(&mut vm, 1.0);
    let error = vm.call(&function, None, &[argument]).unwrap_err();
    assert!(matches!(
        error,
        VmError::Execution(FailureReport {
            code: FailureCode::Capacity,
            ..
        })
    ));
    assert_eq!(entered.get(), 1);
    assert_eq!(vm.state(), VmState::Faulted);
    assert_eq!(probe.contexts_closed(), 1);
}

#[test]
fn cached_vm_metadata_remains_available_after_context_teardown_without_engine_entry() {
    let engine = RustFixture::<Inline>::new("cached-vm-metadata");
    let probe = engine.probe();
    let mut vm = VirtualMachine::new(engine, config()).unwrap();
    let descriptor = vm.descriptor().clone();
    assert!(!vm.features().source && !vm.features().compiler);
    assert!(vm.debug_descriptor().unwrap().capabilities.is_empty());
    vm.shutdown().unwrap();
    let checkpoints = probe.checkpoint_calls();
    assert_eq!(probe.contexts_closed(), 1);
    assert_eq!(vm.descriptor().adapter_id, descriptor.adapter_id);
    assert_eq!(vm.descriptor().owner, std::thread::current().id());
    assert!(!vm.features().modules);
    assert!(vm.debug_descriptor().unwrap().capabilities.is_empty());
    assert_eq!(
        vm.debug_state(),
        coldmind_runtime::debug::DebugExecutionState::Closed
    );
    assert_eq!(probe.checkpoint_calls(), checkpoints);
}
