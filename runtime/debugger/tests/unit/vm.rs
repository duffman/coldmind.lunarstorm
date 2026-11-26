//! Unit tests of jetbunny's owner-thread VM facade: owner guards, state transitions,
//! turn cancellation, binding, source and artifact failures, and debugger attach.

use super::*;
use contract_fixture::{FixtureProbe, Inline, with_watchdog};
use faults::{AttachFault, FaultEngine, ScopeFault};

type TestVm = VirtualMachine<FaultEngine>;
fn configuration() -> VmConfig {
    VmConfig {
        requirements: EngineRequirements {
            entry: EntryRequirement::ExistingCallables,
            debug: crate::debug::DebugRequirements::disabled(),
            ..Default::default()
        },
        ..Default::default()
    }
}

struct CountWork(Rc<Cell<usize>>);
impl ScopeCall for CountWork {
    fn run(&mut self, _: &dyn JsScope) -> Result<(), FailureReport> {
        self.0.set(self.0.get() + 1);
        Ok(())
    }
}

fn vm() -> (TestVm, FixtureProbe<Inline>) {
    let (engine, _, probe) = FaultEngine::new(false);
    (VirtualMachine::new(engine, configuration()).unwrap(), probe)
}

#[test]
fn foreign_owner_guards_precede_admission_and_leave_real_context_unchanged() {
    let (mut vm, probe) = vm();
    let owner = vm.owner;
    // Only a scalar ThreadId crosses threads; the VM and its Rc state stay here.
    vm.owner = thread::spawn(|| thread::current().id()).join().unwrap();
    let entered = Rc::new(Cell::new(0));
    assert_eq!(
        vm.execute(Box::new(CountWork(entered.clone())))
            .unwrap_err(),
        VmError::WrongOwner
    );
    assert_eq!(vm.tick(false).unwrap_err(), VmError::WrongOwner);
    assert_eq!(vm.run().unwrap_err(), VmError::WrongOwner);
    assert_eq!(vm.shutdown().unwrap_err(), VmError::WrongOwner);
    let input = ScriptSource::new(JsString::from_utf8("unentered"), "rust://guard".into());
    assert_eq!(
        vm.submit(input).unwrap_err().reason,
        RejectionReason::WrongOwner
    );
    assert_eq!(entered.get(), 0);
    assert_eq!(probe.checkpoint_calls(), 0);
    assert_eq!(probe.contexts_closed(), 0);
    vm.owner = owner;
    vm.execute(Box::new(CountWork(entered.clone()))).unwrap();
    assert_eq!(entered.get(), 1);
    assert_eq!(vm.state(), VmState::Ready);
}

#[test]
fn active_and_terminal_state_guards_do_not_mutate_the_queue_or_enter_engine() {
    let (mut vm, probe) = vm();
    vm.active.set(true);
    assert_eq!(vm.tick(false).unwrap_err(), VmError::ReentrantEntry);
    assert_eq!(vm.shutdown().unwrap_err(), VmError::ReentrantEntry);
    let input = ScriptSource::new(JsString::from_utf8("owned"), "rust://guard".into());
    assert_eq!(
        vm.submit(input).unwrap_err().reason,
        RejectionReason::Closed
    );
    vm.active.set(false);
    for (state, error) in [
        (VmState::Running, VmError::ReentrantEntry),
        (VmState::Stopped, VmError::Stopped),
        (VmState::Faulted, VmError::Faulted),
    ] {
        vm.state.set(state);
        assert_eq!(vm.run().unwrap_err(), error);
    }
    assert_eq!(probe.checkpoint_calls(), 0);
    assert_eq!(vm.runner.stats().live_jobs, 0);
    vm.state.set(VmState::Ready);
    vm.execute(Box::new(|_: &dyn JsScope| Ok(()))).unwrap();
    assert_eq!(vm.state(), VmState::Ready);
}

#[test]
fn stopping_state_performs_actual_teardown_before_rejecting_entry() {
    let (mut vm, probe) = vm();
    vm.state.set(VmState::Stopping);
    assert_eq!(vm.run().unwrap_err(), VmError::Stopped);
    assert_eq!(vm.state(), VmState::Stopped);
    assert_eq!(probe.contexts_closed(), 1);
    assert_eq!(probe.checkpoint_calls(), 0);
}

#[test]
fn actual_older_turn_can_cancel_or_stop_the_private_capture_without_entering_it() {
    for stop in [false, true] {
        let (mut vm, probe) = vm();
        let host = vm.runner.host();
        let control = host.clone();
        let older = Rc::new(Cell::new(0));
        let old_calls = older.clone();
        host.post(EngineJob::Work(Box::new(move |_: &dyn JsScope| {
            old_calls.set(old_calls.get() + 1);
            if stop {
                control.request_stop();
            } else {
                let id = {
                    let state = control.state.borrow();
                    let (key, _) = state
                        .capture
                        .expect("UntilTurn has admitted its actual target");
                    TaskId {
                        epoch: state.epoch,
                        slot: key.slot,
                        generation: key.generation,
                    }
                };
                assert_eq!(control.cancel(id), CancelOutcome::Canceled);
            }
            Ok(())
        })))
        .unwrap();
        let entered = Rc::new(Cell::new(0));
        assert_eq!(
            vm.execute(Box::new(CountWork(entered.clone())))
                .unwrap_err(),
            if stop {
                VmError::Stopped
            } else {
                VmError::Canceled
            }
        );
        assert_eq!(older.get(), 1);
        assert_eq!(entered.get(), 0);
        assert_eq!(vm.runner.stats().live_jobs, 0);
        assert_eq!(probe.contexts_closed(), usize::from(stop));
        assert_eq!(
            vm.state(),
            if stop {
                VmState::Stopped
            } else {
                VmState::Ready
            }
        );
    }
}

#[test]
fn missing_private_capture_is_a_stopping_fault_after_real_command_execution() {
    let (mut vm, probe) = vm();
    // Deliberately corrupt only private capture metadata at the checkpoint
    // boundary, after the real typed command has entered and returned Unit.
    let capture = vm.capture.clone();
    probe
        .queue_checkpoint(move |_| {
            *capture.borrow_mut() = Captured::Empty;
            Ok(())
        })
        .unwrap();
    // Consume startup first so the next queued task is a turn checkpoint.
    vm.tick(false).unwrap();
    let capture = vm.capture.clone();
    probe
        .queue_checkpoint(move |_| {
            *capture.borrow_mut() = Captured::Empty;
            Ok(())
        })
        .unwrap();
    let entered = Rc::new(Cell::new(0));
    let calls = entered.clone();
    let error = vm
        .execute(Box::new(move |_: &dyn JsScope| {
            calls.set(calls.get() + 1);
            Ok(())
        }))
        .unwrap_err();
    assert!(matches!(
        error,
        VmError::Execution(FailureReport {
            code: FailureCode::EngineFailure,
            ..
        })
    ));
    assert_eq!(entered.get(), 1);
    assert_eq!(vm.state(), VmState::Faulted);
    assert_eq!(probe.contexts_closed(), 1);
}

#[test]
fn value_transaction_rejects_a_real_unit_command_without_returning_a_fake_value() {
    let (mut vm, _) = vm();
    let entered = Rc::new(Cell::new(0));
    assert_eq!(
        vm.value(Kind::Work(Box::new(CountWork(entered.clone()))))
            .unwrap_err(),
        VmError::InvalidState
    );
    assert_eq!(entered.get(), 1);
    assert_eq!(vm.state(), VmState::Ready);
}

struct Function {
    calls: Rc<Cell<usize>>,
    drops: Rc<Cell<usize>>,
}
impl HostFunction for Function {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.calls.set(self.calls.get() + 1);
        call.import_data(JsData::Number(42.0))
    }
}
impl Drop for Function {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

#[test]
fn scoped_binding_failures_detach_real_thrown_values_and_release_uninstalled_callbacks() {
    for (fault, message) in [
        (ScopeFault::HostFunction, "host registration failed"),
        (ScopeFault::Global, "global access failed"),
        (ScopeFault::Set, "global assignment failed"),
    ] {
        let (engine, control, probe) = FaultEngine::new(false);
        let reports = Rc::new(Cell::new(0));
        let seen = reports.clone();
        let mut config = configuration();
        config.script_error_policy = ScriptErrorPolicy::Continue;
        config.report_sink = Some(Rc::new(move |_| seen.set(seen.get() + 1)));
        let mut vm = VirtualMachine::new(engine, config).unwrap();
        let calls = Rc::new(Cell::new(0));
        let drops = Rc::new(Cell::new(0));
        control.scope.set(fault);
        let error = vm
            .bind_function(
                JsString::from_utf8("fault_target"),
                0,
                Rc::new(Function {
                    calls: calls.clone(),
                    drops: drops.clone(),
                }),
            )
            .unwrap_err();
        assert!(
            matches!(error, VmError::Execution(FailureReport { class: FailureClass::Script, thrown: Some(JsData::String(ref text)), .. }) if text.to_utf8().unwrap() == message)
        );
        assert_eq!((calls.get(), drops.get(), reports.get()), (0, 1, 0));
        assert_eq!(probe.live_roots(), Some(0));
        assert_eq!(vm.state(), VmState::Ready);
        control.scope.set(ScopeFault::None);
        let function = vm
            .bind_function(
                JsString::from_utf8("fault_target"),
                0,
                Rc::new(Function {
                    calls: calls.clone(),
                    drops: drops.clone(),
                }),
            )
            .unwrap();
        drop(vm.call(&function, None, &[]).unwrap());
        assert_eq!(calls.get(), 1);
        vm.shutdown().unwrap();
        assert_eq!(drops.get(), 2);
        assert_eq!(probe.contexts_closed(), 1);
    }
}

#[test]
fn source_and_artifact_failures_preserve_original_scoped_exception_without_retry() {
    for fault in [ScopeFault::Evaluate, ScopeFault::Artifact] {
        let (engine, control, probe) = FaultEngine::new(true);
        let mut vm = VirtualMachine::new(engine, configuration()).unwrap();
        let artifact = vm
            .compile(faults::source(), faults::options(DebugInfo::Required))
            .unwrap();
        control.scope.set(fault);
        let error = if fault == ScopeFault::Evaluate {
            vm.evaluate(faults::source())
        } else {
            vm.execute_artifact(artifact)
        }
        .unwrap_err();
        assert!(matches!(
            error,
            VmError::Execution(FailureReport {
                class: FailureClass::Script,
                thrown: Some(JsData::String(_)),
                ..
            })
        ));
        assert_eq!(
            control.program_calls.get(),
            0,
            "entry failure must not execute the registered Rust program"
        );
        assert_eq!(vm.state(), VmState::Faulted);
        assert_eq!(probe.contexts_closed(), 1);
        assert!(vm.last_failure().is_some());
    }
}

#[test]
fn debugger_attach_rejections_and_panic_use_independent_vm_error_channels() {
    let (engine, control, probe) = FaultEngine::new(false);
    let mut config = configuration();
    config.requirements.debug = crate::debug::DebugRequirements::default();
    let mut vm = VirtualMachine::new(engine, config).unwrap();
    control.attach.set(AttachFault::Unsupported);
    assert!(matches!(
        vm.attach_debugger(DebugSessionOptions::default()),
        Err(VmError::Unsupported)
    ));
    assert_eq!(vm.state(), VmState::Ready);
    control.attach.set(AttachFault::Capacity);
    assert!(matches!(
        vm.attach_debugger(DebugSessionOptions::default()),
        Err(VmError::InvalidState)
    ));
    assert_eq!(probe.contexts_closed(), 0);
    control.attach.set(AttachFault::Panic);
    assert!(matches!(
        vm.attach_debugger(DebugSessionOptions::default()),
        Err(VmError::Execution(FailureReport {
            code: FailureCode::HostPanic,
            ..
        }))
    ));
    assert_eq!(control.attach_calls.get(), 3);
    assert_eq!(vm.state(), VmState::Faulted);
    assert_eq!(probe.contexts_closed(), 1);
    vm.shutdown().unwrap();
    assert_eq!(
        vm.state(),
        VmState::Faulted,
        "cached attachment panic survives successful native cleanup"
    );
    assert_eq!(vm.last_failure().unwrap().code, FailureCode::HostPanic);
}

#[test]
fn disabled_debugger_attach_rejects_before_invoking_the_dependency() {
    let (engine, control, probe) = FaultEngine::new(false);
    let mut vm = VirtualMachine::new(engine, configuration()).unwrap();
    assert!(matches!(
        vm.attach_debugger(DebugSessionOptions::default()),
        Err(VmError::Unsupported)
    ));
    assert_eq!(control.attach_calls.get(), 0);
    assert_eq!(probe.checkpoint_calls(), 0);
    assert_eq!(vm.state(), VmState::Ready);
}

#[test]
fn unexpected_native_context_close_faults_the_next_owner_entry_without_returning_a_value() {
    let (engine, control, probe) = FaultEngine::new(false);
    let mut vm = VirtualMachine::new(engine, configuration()).unwrap();
    control.close_native();
    let calls = Rc::new(Cell::new(0));
    let error = vm.execute(Box::new(CountWork(calls.clone()))).unwrap_err();
    assert!(matches!(
        error,
        VmError::Execution(FailureReport {
            code: FailureCode::ClosedContext,
            ..
        })
    ));
    assert_eq!(calls.get(), 0);
    assert_eq!(probe.contexts_closed(), 1);
    assert_eq!(vm.state(), VmState::Faulted);
}

#[test]
fn continuous_vm_run_uses_the_same_shutdown_and_fault_refresh_as_synchronous_commands() {
    for fail in [false, true] {
        let (mut vm, probe) = vm();
        let host = vm.runner.host();
        let control = host.clone();
        let entered = Rc::new(Cell::new(0));
        let calls = entered.clone();
        host.post(EngineJob::Work(Box::new(move |_: &dyn JsScope| {
            calls.set(calls.get() + 1);
            if fail {
                Err(FailureReport::fault(
                    FailureCode::EngineFailure,
                    "queued owner fault",
                ))
            } else {
                control.request_stop();
                Ok(())
            }
        })))
        .unwrap();
        let result = vm.run();
        if fail {
            assert!(matches!(
                result,
                Err(VmError::Execution(FailureReport {
                    code: FailureCode::EngineFailure,
                    ..
                }))
            ));
        } else {
            result.unwrap();
        }
        assert_eq!(entered.get(), 1);
        assert_eq!(
            vm.state(),
            if fail {
                VmState::Faulted
            } else {
                VmState::Stopped
            }
        );
        assert_eq!(probe.contexts_closed(), 1);
    }
}

#[test]
fn constructor_owner_and_initialization_failures_return_configuration_and_close_created_contexts() {
    let owner = thread::spawn(|| thread::current().id()).join().unwrap();
    let (engine, _, probe) = FaultEngine::with_owner(false, Some(owner));
    assert!(matches!(
        VirtualMachine::new(engine, configuration()),
        Err(VmError::Configuration(FailureReport {
            code: FailureCode::WrongOwner,
            ..
        }))
    ));
    assert_eq!(probe.contexts_created(), 0);
    let (engine, _, probe) = FaultEngine::new(false);
    let mut config = configuration();
    config.requirements.entry = EntryRequirement::Source;
    assert!(matches!(
        VirtualMachine::new(engine, config),
        Err(VmError::Unsupported)
    ));
    assert_eq!(probe.contexts_created(), 1);
    assert_eq!(probe.contexts_closed(), 1);
    let (engine, _, probe) = FaultEngine::new(false);
    let mut config = configuration();
    config.requirements.debug = crate::debug::DebugRequirements::default();
    config.debug_limits.source_bytes = usize::MAX;
    let error = VirtualMachine::new(engine, config).err().unwrap();
    assert!(
        matches!(error, VmError::Configuration(FailureReport { class: FailureClass::Host, code: FailureCode::EngineFailure, ref message, .. }) if message.as_ref().unwrap().to_utf8().unwrap().contains("Capacity"))
    );
    assert_eq!(probe.contexts_created(), 1);
    assert_eq!(probe.contexts_closed(), 1);
}

#[test]
fn invalid_source_is_rejected_before_scope_entry_and_preserves_submitted_buffers() {
    let (engine, control, probe) = FaultEngine::new(true);
    let mut config = configuration();
    config.debug_limits.source_bytes = 1;
    let mut vm = VirtualMachine::new(engine, config).unwrap();
    let input = faults::source();
    let pointer = input.text.as_units().as_ptr();
    let rejected = vm.submit(input).unwrap_err();
    assert!(matches!(
        rejected.reason,
        RejectionReason::InvalidSource(FailureReport {
            code: FailureCode::InvalidArtifact,
            ..
        })
    ));
    assert_eq!(rejected.value.text.as_units().as_ptr(), pointer);
    assert!(matches!(
        vm.evaluate(faults::source()),
        Err(VmError::Execution(FailureReport {
            code: FailureCode::InvalidArtifact,
            ..
        }))
    ));
    assert!(matches!(
        vm.compile(faults::source(), faults::options(DebugInfo::Required)),
        Err(VmError::Execution(FailureReport {
            code: FailureCode::InvalidArtifact,
            ..
        }))
    ));
    assert_eq!(control.program_calls.get(), 0);
    assert_eq!(probe.compile_calls(), 0);
    assert_eq!(probe.checkpoint_calls(), 0);
    assert!(vm.last_failure().is_none());
    assert_eq!(vm.state(), VmState::Ready);
}

#[test]
fn missing_optional_interfaces_reject_owned_artifacts_without_entering_a_native_loader() {
    let (engine, _, _) = FaultEngine::new(true);
    let mut producer = VirtualMachine::new(engine, configuration()).unwrap();
    let artifact = producer
        .compile(faults::source(), faults::options(DebugInfo::Required))
        .unwrap();
    let (mut vm, probe) = vm();
    assert!(matches!(
        vm.execute_artifact(artifact),
        Err(VmError::Unsupported)
    ));
    assert!(matches!(
        vm.compile(faults::source(), faults::options(DebugInfo::Required)),
        Err(VmError::Unsupported)
    ));
    assert!(matches!(
        vm.evaluate(faults::source()),
        Err(VmError::Unsupported)
    ));
    let rejected = vm.submit(faults::source()).unwrap_err();
    assert_eq!(rejected.reason, RejectionReason::Unsupported);
    assert_eq!(probe.checkpoint_calls(), 0);
    assert_eq!(vm.state(), VmState::Ready);
}

#[test]
fn artifact_build_and_profile_guards_reject_before_program_entry_without_faulting_owner() {
    for field in 0..5 {
        let (engine, control, probe) = FaultEngine::new(true);
        let mut vm = VirtualMachine::new(engine, configuration()).unwrap();
        let info = if field == 4 {
            DebugInfo::Strip
        } else {
            DebugInfo::Required
        };
        let artifact = vm.compile(faults::source(), faults::options(info)).unwrap();
        // Model an incompatible producer/build through expected metadata only;
        // the artifact remains actual unmodified output of the Rust registry.
        match field {
            0 => vm.descriptor.adapter_id.push_str("-other"),
            1 => vm.descriptor.native_version.push_str("-other"),
            2 => vm.descriptor.target_fingerprint.push_str("-other"),
            3 => vm.descriptor.build_fingerprint.push_str("-other"),
            _ => {
                // Guard-only profile metadata, not a qualified debugger session.
                vm.development = true;
                assert!(matches!(
                    vm.compile(faults::source(), faults::options(DebugInfo::Strip)),
                    Err(VmError::Unsupported)
                ));
            }
        }
        let before = probe.checkpoint_calls();
        assert!(matches!(
            vm.execute_artifact(artifact),
            Err(VmError::Execution(FailureReport {
                code: FailureCode::InvalidArtifact,
                ..
            }))
        ));
        assert_eq!(control.program_calls.get(), 0);
        assert_eq!(probe.compile_calls(), 1);
        assert_eq!(probe.checkpoint_calls(), before);
        assert_eq!(vm.state(), VmState::Ready);
        assert!(vm.last_failure().is_none());
    }
    let (engine, control, _) = FaultEngine::new(true);
    let mut vm = VirtualMachine::new(engine, configuration()).unwrap();
    let artifact = vm
        .compile(faults::source(), faults::options(DebugInfo::Required))
        .unwrap();
    drop(vm.execute_artifact(artifact).unwrap());
    assert_eq!(
        control.program_calls.get(),
        1,
        "matching real metadata permits the program once"
    );
}

#[test]
fn typed_capture_shape_mismatches_do_not_expose_the_wrong_owned_result() {
    let (engine, control, probe) = FaultEngine::new(true);
    let mut vm = VirtualMachine::new(engine, configuration()).unwrap();
    vm.tick(false).unwrap();
    let capture = vm.capture.clone();
    probe
        .queue_checkpoint(move |_| {
            *capture.borrow_mut() = Captured::Unit;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        vm.compile(faults::source(), faults::options(DebugInfo::Required)),
        Err(VmError::InvalidState)
    ));
    assert_eq!(probe.compile_calls(), 1);
    assert_eq!(control.program_calls.get(), 0);
    let output = Rc::new(RefCell::new(None));
    let result = output.clone();
    vm.execute(Box::new(move |scope: &dyn JsScope| {
        let local = scope
            .import_data(JsData::Number(9.0))
            .map_err(|error| scope.detach(error))?;
        *result.borrow_mut() = Some(scope.retain(local)?);
        Ok(())
    }))
    .unwrap();
    let root = output.borrow_mut().take().unwrap();
    let capture = vm.capture.clone();
    probe
        .queue_checkpoint(move |_| {
            *capture.borrow_mut() = Captured::Value(root);
            Ok(())
        })
        .unwrap();
    let calls = Rc::new(Cell::new(0));
    assert_eq!(
        vm.execute(Box::new(CountWork(calls.clone()))).unwrap_err(),
        VmError::InvalidState
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(vm.state(), VmState::Ready);
    vm.execute(Box::new(CountWork(calls.clone()))).unwrap();
    assert_eq!(
        probe.live_roots(),
        Some(0),
        "discarded wrong-shape result releases its real ValueStore root"
    );
}

#[test]
fn wrapper_configuration_failures_precede_creation_and_valid_configuration_still_enters() {
    for invalid in 0..4 {
        let (engine, _, probe) = FaultEngine::new(false);
        let mut config = configuration();
        match invalid {
            0 => config.scheduler.max_jobs = 0,
            1 => config.context.max_persistent_roots = 0,
            2 => config.debug_limits.requests = 0,
            _ => config.script_error_policy = ScriptErrorPolicy::Continue,
        }
        assert!(matches!(
            VirtualMachine::new(engine, config),
            Err(VmError::Configuration(_))
        ));
        assert_eq!(probe.contexts_created(), 0);
    }
    let (mut vm, probe) = vm();
    let entered = Rc::new(Cell::new(0));
    vm.execute(Box::new(CountWork(entered.clone()))).unwrap();
    assert_eq!(entered.get(), 1);
    assert_eq!(probe.contexts_created(), 1);
}

#[test]
fn source_submission_owns_a_real_queue_permit_and_cancel_preserves_later_progress() {
    let (engine, control, probe) = FaultEngine::new(true);
    let mut config = configuration();
    config.scheduler.max_jobs = 1;
    let mut vm = VirtualMachine::new(engine, config).unwrap();
    let entered = Rc::new(Cell::new(0));
    vm.runner
        .host()
        .post(EngineJob::Work(Box::new(CountWork(entered.clone()))))
        .unwrap();
    let input = faults::source();
    let pointer = input.text.as_units().as_ptr();
    let rejected = vm.submit(input).unwrap_err();
    assert_eq!(rejected.reason, RejectionReason::Full);
    assert_eq!(rejected.value.text.as_units().as_ptr(), pointer);
    assert_eq!(probe.checkpoint_calls(), 0);
    assert_eq!(vm.state(), VmState::Ready);
    vm.tick(false).unwrap();
    assert_eq!(entered.get(), 1);
    let task = vm.submit(rejected.value).unwrap();
    vm.handle().request_cancel(task).unwrap();
    vm.tick(false).unwrap();
    assert_eq!(control.program_calls.get(), 0);
    vm.submit(faults::source()).unwrap();
    vm.tick(false).unwrap();
    assert_eq!(control.program_calls.get(), 1);
    assert_eq!(vm.runner.stats().live_jobs, 0);
    assert_eq!(vm.state(), VmState::Ready);
}

#[test]
fn every_value_command_detaches_retention_failure_after_its_real_body_without_retry() {
    for command in 0..4 {
        let (engine, control, probe) = FaultEngine::new(true);
        let mut config = configuration();
        config.context.max_persistent_roots = 1;
        let mut vm = VirtualMachine::new(engine, config).unwrap();
        let calls = Rc::new(Cell::new(0));
        let drops = Rc::new(Cell::new(0));
        let function = vm
            .bind_function(
                JsString::from_utf8("retention-target"),
                0,
                Rc::new(Function {
                    calls: calls.clone(),
                    drops: drops.clone(),
                }),
            )
            .unwrap();
        let artifact = vm
            .compile(faults::source(), faults::options(DebugInfo::Required))
            .unwrap();
        assert_eq!(probe.live_roots(), Some(1), "the real root table is full");
        let error = match command {
            0 => vm.bind_function(
                JsString::from_utf8("retention-bind"),
                0,
                Rc::new(Function {
                    calls: calls.clone(),
                    drops: drops.clone(),
                }),
            ),
            1 => vm.evaluate(faults::source()),
            2 => vm.call(&function, None, &[]),
            _ => vm.execute_artifact(artifact),
        }
        .unwrap_err();
        assert!(matches!(
            error,
            VmError::Execution(FailureReport {
                code: FailureCode::Capacity,
                ..
            })
        ));
        assert_eq!(calls.get(), usize::from(command == 2));
        assert_eq!(
            control.program_calls.get(),
            usize::from(command == 1 || command == 3)
        );
        assert_eq!(probe.compile_calls(), 1);
        assert_eq!(probe.contexts_closed(), 1);
        assert_eq!(vm.state(), VmState::Faulted);
        assert_eq!(vm.tick(false).unwrap_err(), VmError::Faulted);
        assert_eq!(probe.contexts_closed(), 1);
        drop(function);
        assert_eq!(drops.get(), if command == 0 { 2 } else { 1 });
    }
}

#[test]
fn compiler_failure_returns_its_detached_report_and_never_runs_the_program() {
    let (engine, control, probe) = FaultEngine::new(true);
    let mut vm = VirtualMachine::new(engine, configuration()).unwrap();
    control.scope.set(ScopeFault::Compile);
    assert!(matches!(
        vm.compile(faults::source(), faults::options(DebugInfo::Required)),
        Err(VmError::Execution(FailureReport {
            code: FailureCode::InvalidArtifact,
            ..
        }))
    ));
    assert_eq!(probe.compile_calls(), 0);
    assert_eq!(control.program_calls.get(), 0);
    assert_eq!(probe.contexts_closed(), 1);
    assert_eq!(vm.state(), VmState::Faulted);
}

#[test]
fn stop_published_during_run_until_idle_completes_it_normally() {
    let (mut vm, probe) = vm();
    let host = vm.runner.host();
    let control = host.clone();
    host.post(EngineJob::Work(Box::new(move |_: &dyn JsScope| {
        control.request_stop();
        Ok(())
    })))
    .unwrap();
    host.post(EngineJob::Work(Box::new(|_: &dyn JsScope| Ok(()))))
        .unwrap();
    vm.run_until_idle().unwrap();
    assert_eq!(vm.state(), VmState::Stopped);
    assert_eq!(probe.contexts_closed(), 1);
    assert_eq!(vm.run_until_idle().unwrap_err(), VmError::Stopped);
}

fn batched_vm(task_batch: usize) -> (TestVm, FixtureProbe<Inline>) {
    let (engine, _, probe) = FaultEngine::new(false);
    let mut config = configuration();
    config.scheduler.task_batch = task_batch;
    (VirtualMachine::new(engine, config).unwrap(), probe)
}

#[test]
fn stop_from_a_job_completes_run_until_idle_normally_with_a_batch_of_one() {
    let (mut vm, probe) = batched_vm(1);
    let host = vm.runner.host();
    let control = host.clone();
    host.post(EngineJob::Work(Box::new(move |_: &dyn JsScope| {
        control.request_stop();
        Ok(())
    })))
    .unwrap();
    host.post(EngineJob::Work(Box::new(|_: &dyn JsScope| Ok(()))))
        .unwrap();
    assert_eq!(vm.run_until_idle(), Ok(()));
    assert_eq!(vm.state(), VmState::Stopped);
    assert_eq!(probe.contexts_closed(), 1);
}

#[test]
fn run_until_idle_rejects_a_stop_published_before_it_was_entered() {
    let (mut vm, probe) = vm();
    vm.handle().request_stop().unwrap();
    assert_eq!(vm.run_until_idle().unwrap_err(), VmError::Stopped);
    assert_eq!(vm.state(), VmState::Stopped);
    assert_eq!(probe.contexts_closed(), 1);
}

#[test]
fn run_until_idle_returns_at_once_without_tracked_work_and_leaves_the_vm_ready() {
    let (mut vm, _) = vm();
    let rescue = vm.handle();
    // The first call runs the startup checkpoint; the second finds nothing tracked.
    let first = with_watchdog(move || drop(rescue.request_stop()), || vm.run_until_idle());
    assert_eq!(first, Ok(()));
    assert!(!vm.has_tracked_work());
    assert_eq!(vm.run_until_idle(), Ok(()));
    assert_eq!(vm.state(), VmState::Ready);
}

#[test]
fn run_until_idle_pumps_across_ticks_until_all_work_ran() {
    let (mut vm, _) = batched_vm(1);
    let count = Rc::new(Cell::new(0));
    let host = vm.runner.host();
    for _ in 0..3 {
        host.post(EngineJob::Work(Box::new(CountWork(count.clone()))))
            .unwrap();
    }
    assert_eq!(vm.run_until_idle(), Ok(()));
    assert_eq!(count.get(), 3);
    assert_eq!(vm.state(), VmState::Ready);
}

#[test]
fn invalid_module_limits_are_a_configuration_error_before_any_context() {
    let (engine, _, probe) = FaultEngine::new(false);
    let mut config = configuration();
    config.module_limits.nodes = 0;
    assert!(matches!(
        VirtualMachine::new(engine, config),
        Err(VmError::Configuration(_))
    ));
    assert_eq!(probe.contexts_created(), 0);
}

#[test]
fn module_query_on_an_engine_without_modules_fails_without_faulting_the_vm() {
    let (mut vm, _) = vm();
    let id = ModuleId {
        context: vm.context,
        slot: 1,
    };
    let Err(VmError::Execution(report)) = vm.module_state(id) else {
        panic!("expected a detached failure");
    };
    assert_eq!(report.code, FailureCode::Unsupported);
    assert_eq!(vm.state(), VmState::Ready);
}

#[test]
fn a_blocking_tick_on_a_fresh_vm_runs_startup_and_returns() {
    let (mut vm, _) = vm();
    let rescue = vm.handle();
    let tick = with_watchdog(move || drop(rescue.request_stop()), || vm.tick(true)).unwrap();
    assert_eq!(tick.state, VmState::Ready);
    assert!(!vm.has_tracked_work());
}
