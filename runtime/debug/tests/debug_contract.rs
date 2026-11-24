//! Pure Rust control-path fixtures. These tests do not qualify a JavaScript engine
//! or claim native breakpoint, busy-JS interruption, or lexical-inspection support.
use coldmind_runtime::{debug::*, engine::*};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use std::{
    cell::{Cell, RefCell},
    num::NonZeroU64,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
assert_impl_all!(VmDebugHandle: Send, Sync);
assert_not_impl_any!(VmDebugHandle: Clone);

#[derive(Default)]
struct Signal {
    calls: AtomicUsize,
    fail: AtomicBool,
}
impl DebugSignal for Signal {
    fn wake(&self) -> Result<(), DebugError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            Err(DebugError::WakeFailed(FailureReport::fault(
                FailureCode::EngineFailure,
                "fixture wake failed",
            )))
        } else {
            Ok(())
        }
    }
}
struct Backend {
    descriptor: DebugDescriptor,
    port: RefCell<Option<Rc<dyn DebugPort>>>,
    session: Cell<Option<DebugSessionId>>,
    signal: Arc<Signal>,
    seen: RefCell<Vec<&'static str>>,
    asynchronous: Cell<bool>,
    evaluation: Cell<Option<DebugRequestId>>,
    initialized: Cell<usize>,
}
impl Backend {
    fn new() -> Self {
        Self {
            descriptor: DebugDescriptor {
                api_version: 1,
                capabilities: DebugCapabilities::baseline(),
                native_requirements: vec!["Rust coordination fixture only".to_owned()],
                interrupt_mechanism: "atomic fixture signal".to_owned(),
            },
            port: RefCell::new(None),
            session: Cell::new(None),
            signal: Arc::new(Signal::default()),
            seen: RefCell::new(vec![]),
            asynchronous: Cell::new(false),
            evaluation: Cell::new(None),
            initialized: Cell::new(0),
        }
    }
    fn port(&self) -> Rc<dyn DebugPort> {
        self.port.borrow().as_ref().unwrap().clone()
    }
    fn pause(&self) -> PauseId {
        let port = self.port();
        let pause = port
            .identities()
            .begin_pause(self.session.get().unwrap())
            .unwrap();
        port.publish(DebugUpdate::Paused {
            sequence: port.identities().next_update_sequence().unwrap(),
            pause,
            reason: PauseReason::Requested,
            location: None,
            original: None,
            breakpoint_ids: vec![],
        })
        .unwrap();
        pause
    }
    fn frame(&self, pause: PauseId) -> FrameRef {
        match self
            .port()
            .identities()
            .register(pause, ReferenceKind::Frame, NonZeroU64::new(1).unwrap())
            .unwrap()
        {
            DebugReference::Frame(frame) => frame,
            _ => unreachable!(),
        }
    }
    fn finish_evaluation(&self) {
        let id = self.evaluation.take().unwrap();
        self.port()
            .publish(DebugUpdate::Reply {
                id,
                result: Ok(DebugReply::Evaluation {
                    value: DebugValue {
                        preview: JsString::from_utf8("fixture result"),
                        object: None,
                        accessor: false,
                        unavailable: None,
                    },
                    exception: false,
                }),
            })
            .unwrap();
    }
}
impl DebugBackend for Backend {
    fn descriptor(&self) -> &DebugDescriptor {
        &self.descriptor
    }
    fn initialize(&self, port: Rc<dyn DebugPort>) -> Result<(), DebugError> {
        self.initialized.set(self.initialized.get() + 1);
        *self.port.borrow_mut() = Some(port);
        Ok(())
    }
    fn attach(&self, options: DebugAttachOptions) -> Result<Arc<dyn DebugSignal>, DebugError> {
        self.session.set(Some(options.session));
        Ok(self.signal.clone())
    }
    fn service(&self) -> Result<DebugServiceReport, DebugError> {
        let port = self.port();
        let mut progress = false;
        while let Some(directive) = port.next_directive(false) {
            progress = true;
            match directive {
                DebugDirective::Pause => {
                    self.seen.borrow_mut().push("pause");
                    if !matches!(port.identities().state(), DebugExecutionState::Paused(_)) {
                        self.pause();
                    }
                }
                DebugDirective::Continue { pause, .. } => {
                    self.seen.borrow_mut().push("continue");
                    port.identities().end_pause(pause)?;
                    port.publish(DebugUpdate::Resumed {
                        sequence: port.identities().next_update_sequence()?,
                        previous: pause,
                    })?;
                }
                DebugDirective::Detach => {
                    self.seen.borrow_mut().push("detach");
                    self.detach()?;
                }
                DebugDirective::Stop => {
                    self.seen.borrow_mut().push("stop");
                    self.detach()?;
                }
                DebugDirective::Request(command) => match command.request {
                    DebugRequest::Evaluate { .. } if self.asynchronous.get() => {
                        self.seen.borrow_mut().push("evaluate");
                        self.evaluation.set(Some(command.id));
                    }
                    DebugRequest::Stack { .. } => {
                        self.seen.borrow_mut().push("stack");
                        port.publish(DebugUpdate::Reply {
                            id: command.id,
                            result: Ok(DebugReply::Stack {
                                frames: vec![],
                                total: 0,
                            }),
                        })?;
                    }
                    DebugRequest::SetBreakpoints {
                        selector,
                        breakpoints,
                    } => {
                        let info = breakpoints
                            .iter()
                            .map(|spec| {
                                let candidates =
                                    port.resolve_breakpoint(&selector, spec.line, spec.column)?;
                                Ok(BreakpointInfo {
                                    client_id: spec.client_id,
                                    state: if candidates.is_empty() {
                                        BreakpointState::Pending
                                    } else {
                                        BreakpointState::Verified(candidates)
                                    },
                                })
                            })
                            .collect::<Result<Vec<_>, DebugError>>()?;
                        port.publish(DebugUpdate::Reply {
                            id: command.id,
                            result: Ok(DebugReply::Breakpoints(info)),
                        })?;
                    }
                    DebugRequest::ExceptionPolicy(policy) => {
                        port.publish(DebugUpdate::Reply {
                            id: command.id,
                            result: Ok(DebugReply::ExceptionPolicy(policy)),
                        })?;
                    }
                    _ => {
                        port.publish(DebugUpdate::Reply {
                            id: command.id,
                            result: Err(DebugError::Unsupported),
                        })?;
                    }
                },
            }
        }
        Ok(DebugServiceReport {
            made_progress: progress,
            state: port.identities().state(),
        })
    }
    fn detach(&self) -> Result<(), DebugError> {
        if let Some(session) = self.session.take() {
            let port = self.port();
            if let DebugExecutionState::Paused(pause) = port.identities().state() {
                port.identities().end_pause(pause)?;
            }
            port.identities().close_session(session)?;
            port.publish(DebugUpdate::SessionClosed {
                session,
                reason: None,
            })?;
        }
        Ok(())
    }
}
fn fixture(limits: DebugLimits) -> (ValueStore<()>, Rc<Backend>, Rc<DebugCoordinator>) {
    fixture_with_capabilities(limits, DebugCapabilities::baseline())
}
fn fixture_with_capabilities(
    limits: DebugLimits,
    capabilities: DebugCapabilities,
) -> (ValueStore<()>, Rc<Backend>, Rc<DebugCoordinator>) {
    let identity = EngineIdentity::new().unwrap();
    let store = ValueStore::new(&identity, ContextLimits::default()).unwrap();
    let mut backend = Backend::new();
    backend.descriptor.capabilities = capabilities;
    let backend = Rc::new(backend);
    let coordinator = DebugCoordinator::new(
        backend.clone(),
        store.context(),
        limits,
        Arc::new(|| Ok(())),
        NonZeroU64::new(1).unwrap(),
    )
    .unwrap();
    (store, backend, coordinator)
}
fn accepted(receipt: DebugRequestReceipt) -> DebugRequestId {
    match receipt {
        DebugRequestReceipt::Accepted { id }
        | DebugRequestReceipt::AcceptedWakeFailed { id, .. } => id,
    }
}
fn read_reply(handle: &VmDebugHandle, id: DebugRequestId) -> Result<DebugReply, DebugError> {
    loop {
        if let DebugUpdate::Reply { id: actual, result } =
            handle.try_recv().expect("terminal reply")
        {
            assert_eq!(actual, id);
            return result;
        }
    }
}

#[test]
fn terminal_reply_permit_is_held_until_the_controller_reads_it() {
    let (_store, _backend, coordinator) = fixture(DebugLimits {
        requests: 1,
        ..DebugLimits::default()
    });
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let id = accepted(
        handle
            .try_request(DebugRequest::Sources {
                page: Page { start: 0, count: 1 },
            })
            .unwrap(),
    );
    coordinator.service().unwrap();
    assert!(matches!(
        handle.try_request(DebugRequest::Sources {
            page: Page { start: 0, count: 1 }
        }),
        Err(Rejected {
            reason: RejectionReason::Debug(DebugError::Capacity),
            ..
        })
    ));
    assert!(matches!(
        read_reply(&handle, id),
        Ok(DebugReply::Sources { total: 0, .. })
    ));
    assert!(
        handle
            .try_request(DebugRequest::Sources {
                page: Page { start: 0, count: 1 }
            })
            .is_ok()
    );
}
#[test]
fn first_continue_decision_wins_even_when_normal_admission_is_full() {
    let (_store, backend, coordinator) = fixture(DebugLimits {
        requests: 1,
        updates: 1,
        ..DebugLimits::default()
    });
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    let id = accepted(
        handle
            .try_request(DebugRequest::Stack {
                pause,
                page: Page { start: 0, count: 1 },
            })
            .unwrap(),
    );
    handle
        .continue_execution(pause, DebugContinue::StepOver)
        .unwrap();
    assert_eq!(
        handle
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap_err(),
        DebugError::InFlight
    );
    coordinator.service().unwrap();
    assert_eq!(read_reply(&handle, id).unwrap_err(), DebugError::StalePause);
    assert_eq!(&*backend.seen.borrow(), &["continue"]);
    assert_eq!(handle.state(), DebugExecutionState::Running);
}
#[test]
fn restricted_wait_is_woken_by_cross_thread_control_without_an_application_pump() {
    let (_store, backend, coordinator) = fixture(DebugLimits::default());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    let producer = std::thread::spawn(move || {
        handle
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        handle
    });
    assert!(
        matches!(backend.port().next_directive(true), Some(DebugDirective::Continue { pause: actual, .. }) if actual == pause)
    );
    let _handle = producer.join().unwrap();
    assert!(backend.seen.borrow().is_empty());
}
#[test]
fn asynchronous_evaluation_blocks_inspection_and_continue_but_not_stop() {
    let (_store, backend, coordinator) = fixture(DebugLimits::default());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    let frame = backend.frame(pause);
    backend.asynchronous.set(true);
    let id = accepted(
        handle
            .try_request(DebugRequest::Evaluate {
                frame,
                expression: JsString::from_utf8("1+1"),
                allow_side_effects: true,
            })
            .unwrap(),
    );
    coordinator.service().unwrap();
    assert_eq!(
        handle
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap_err(),
        DebugError::InFlight
    );
    assert!(matches!(
        handle.try_request(DebugRequest::Stack {
            pause,
            page: Page { start: 0, count: 1 }
        }),
        Err(Rejected {
            reason: RejectionReason::Debug(DebugError::InFlight),
            ..
        })
    ));
    backend.finish_evaluation();
    assert!(matches!(
        read_reply(&handle, id),
        Ok(DebugReply::Evaluation {
            exception: false,
            ..
        })
    ));
    coordinator.stop_signal()().unwrap();
    coordinator.service().unwrap();
    assert!(coordinator.is_stopping());
    assert_eq!(backend.seen.borrow().last(), Some(&"stop"));
}
#[test]
fn accepted_wake_failure_preserves_request_identity_and_ownership() {
    let (_store, backend, coordinator) = fixture(DebugLimits::default());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    backend.signal.fail.store(true, Ordering::SeqCst);
    match handle
        .try_request(DebugRequest::Sources {
            page: Page { start: 0, count: 1 },
        })
        .unwrap()
    {
        DebugRequestReceipt::AcceptedWakeFailed {
            id,
            error: DebugError::WakeFailed(_),
        } => {
            assert!(format!("{id:?}").contains("sequence"));
        }
        receipt => panic!("accepted signaling failure expected: {receipt:?}"),
    }
    assert!(coordinator.is_stopping());
}
#[test]
fn dropping_controller_detaches_a_paused_owner_despite_full_request_capacity() {
    let (_store, backend, coordinator) = fixture(DebugLimits {
        requests: 1,
        ..DebugLimits::default()
    });
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    handle
        .try_request(DebugRequest::Stack {
            pause,
            page: Page { start: 0, count: 1 },
        })
        .unwrap();
    drop(handle);
    coordinator.service().unwrap();
    assert_eq!(coordinator.state(), DebugExecutionState::Detached);
    assert_eq!(backend.seen.borrow().last(), Some(&"detach"));
}
#[test]
fn stop_has_precedence_over_already_admitted_continue_and_detach() {
    let (_store, backend, coordinator) = fixture(DebugLimits::default());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    handle
        .continue_execution(pause, DebugContinue::Resume)
        .unwrap();
    handle.detach().unwrap();
    coordinator.stop_signal()().unwrap();
    coordinator.service().unwrap();
    assert_eq!(&*backend.seen.borrow(), &["stop"]);
}
#[test]
fn source_registration_is_synchronous_and_available_without_an_attached_controller() {
    let (store, backend, _coordinator) = fixture(DebugLimits {
        scripts: 1,
        ..DebugLimits::default()
    });
    let make = |key, text| NativeScriptMetadata {
        key: NativeScriptKey::new(store.context(), NonZeroU64::new(key).unwrap()),
        source: ScriptSource::new(JsString::from_utf8(text), "file:///app.js".to_owned()),
        kind: ScriptKind::Eval,
    };
    let first = backend.port().register_script(make(1, "x")).unwrap();
    assert_eq!(backend.port().register_script(make(2, "x")).unwrap(), first);
    assert_eq!(
        backend.port().register_script(make(3, "y")),
        Err(DebugError::Capacity)
    );
}
#[test]
fn missing_baseline_is_rejected_before_native_initialization() {
    let identity = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&identity, ContextLimits::default()).unwrap();
    let mut backend = Backend::new();
    backend.descriptor.capabilities = DebugCapabilities::empty();
    let backend = Rc::new(backend);
    assert!(matches!(
        DebugCoordinator::new(
            backend.clone(),
            store.context(),
            DebugLimits::default(),
            Arc::new(|| Ok(())),
            NonZeroU64::new(1).unwrap()
        ),
        Err(DebugError::Unsupported)
    ));
    assert_eq!(backend.initialized.get(), 0);
}

#[test]
fn latest_execution_state_and_reserved_closure_survive_a_zero_update_queue() {
    let (_store, backend, coordinator) = fixture(DebugLimits {
        updates: 0,
        ..DebugLimits::default()
    });
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let first = backend.pause();
    handle
        .continue_execution(first, DebugContinue::Resume)
        .unwrap();
    coordinator.service().unwrap();
    let second = backend.pause();
    match handle.try_recv().unwrap() {
        DebugUpdate::Paused { pause, .. } => assert_eq!(pause, second),
        _ => panic!("latest state must be retained"),
    }
    handle.detach().unwrap();
    coordinator.service().unwrap();
    assert!(matches!(
        handle.try_recv(),
        Some(DebugUpdate::SessionClosed { .. })
    ));
    let replacement = coordinator.attach(DebugSessionOptions::default()).unwrap();
    assert_eq!(handle.request_pause().unwrap_err(), DebugError::Closed);
    assert_eq!(replacement.state(), DebugExecutionState::Running);
}
#[test]
fn unsafe_evaluation_disconnect_reports_why_it_stops_instead_of_promising_resume() {
    let (_store, backend, coordinator) = fixture(DebugLimits::default());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    let frame = backend.frame(pause);
    backend.asynchronous.set(true);
    let id = accepted(
        handle
            .try_request(DebugRequest::Evaluate {
                frame,
                expression: JsString::from_utf8("fixture"),
                allow_side_effects: true,
            })
            .unwrap(),
    );
    coordinator.service().unwrap();
    handle.detach().unwrap();
    assert!(matches!(
        coordinator.service(),
        Err(DebugError::EngineFailure(_))
    ));
    assert_eq!(read_reply(&handle, id).unwrap_err(), DebugError::Closed);
    loop {
        if let DebugUpdate::SessionClosed { reason, .. } =
            handle.try_recv().expect("reserved close reason")
        {
            assert!(reason.is_some());
            break;
        }
    }
    assert_eq!(backend.seen.borrow().last(), Some(&"stop"));
}
#[test]
fn retained_breakpoint_sets_share_the_session_count_and_condition_budgets() {
    let (_store, _backend, coordinator) = fixture(DebugLimits {
        breakpoints: 1,
        condition_bytes: 12,
        ..DebugLimits::default()
    });
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let request = |origin: &str| DebugRequest::SetBreakpoints {
        selector: ScriptSelector {
            origin: origin.to_owned(),
            expected_digest: None,
        },
        breakpoints: vec![BreakpointSpec {
            client_id: 1,
            line: 0,
            column: None,
            condition: Some(JsString::from_utf8("true")),
            enabled: true,
            hit_condition: None,
            log_message: None,
        }],
    };
    let id = accepted(handle.try_request(request("file:///a.js")).unwrap());
    coordinator.service().unwrap();
    read_reply(&handle, id).unwrap();
    assert!(matches!(
        handle.try_request(request("file:///b.js")),
        Err(Rejected {
            reason: RejectionReason::Debug(DebugError::Capacity),
            ..
        })
    ));
    assert!(
        handle.try_request(request("file:///a.js")).is_ok(),
        "replacement must remain possible at the count limit"
    );
}
#[test]
fn request_accounting_uses_owned_expression_capacity_and_preserves_rejected_input() {
    let (_store, backend, coordinator) = fixture(DebugLimits {
        request_bytes: 1024,
        ..DebugLimits::default()
    });
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    let frame = backend.frame(pause);
    let mut units = Vec::with_capacity(4096);
    units.push(b'x' as u16);
    let error = handle
        .try_request(DebugRequest::Evaluate {
            frame,
            expression: JsString::from_units(units),
            allow_side_effects: true,
        })
        .unwrap_err();
    assert_eq!(error.reason, RejectionReason::Debug(DebugError::Capacity));
    match error.value {
        DebugRequest::Evaluate { expression, .. } => {
            assert_eq!(expression.as_units(), &[b'x' as u16])
        }
        _ => panic!(),
    }
}
#[test]
fn diagnostic_overflow_coalesces_with_a_truthful_dropped_count() {
    let (_store, backend, coordinator) = fixture(DebugLimits {
        updates: 0,
        ..DebugLimits::default()
    });
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    for value in 0..5 {
        backend
            .port()
            .publish(DebugUpdate::Diagnostic {
                report: FailureReport::fault(FailureCode::Capacity, &format!("diagnostic {value}")),
                dropped: 0,
            })
            .unwrap();
    }
    match handle.try_recv().unwrap() {
        DebugUpdate::Diagnostic { report, dropped } => {
            assert_eq!(dropped, 4);
            assert_eq!(report.message.unwrap().to_utf8().unwrap(), "diagnostic 4")
        }
        _ => panic!(),
    }
}

#[test]
fn repeated_service_after_shutdown_does_not_refault_or_reenter_the_backend() {
    let (_store, backend, coordinator) = fixture(DebugLimits::default());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    coordinator.shutdown().unwrap();
    let seen = backend.seen.borrow().len();
    for _ in 0..3 {
        coordinator.service().unwrap();
    }
    assert_eq!(backend.seen.borrow().len(), seen);
    assert!(!coordinator.pending());
    assert_eq!(coordinator.before_entry(), Err(DebugError::Closed));
    assert_eq!(handle.request_pause().unwrap_err(), DebugError::Closed);
}

struct PanickingDetach(Rc<Backend>);
impl DebugBackend for PanickingDetach {
    fn descriptor(&self) -> &DebugDescriptor {
        self.0.descriptor()
    }
    fn initialize(&self, port: Rc<dyn DebugPort>) -> Result<(), DebugError> {
        self.0.initialize(port)
    }
    fn attach(&self, options: DebugAttachOptions) -> Result<Arc<dyn DebugSignal>, DebugError> {
        self.0.attach(options)
    }
    fn service(&self) -> Result<DebugServiceReport, DebugError> {
        self.0.service()
    }
    fn detach(&self) -> Result<(), DebugError> {
        panic!("native detach cleanup panic");
    }
}
#[test]
fn detach_panic_still_revokes_signals_completes_requests_and_closes_identities() {
    let identity = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&identity, ContextLimits::default()).unwrap();
    let backend = Rc::new(Backend::new());
    let coordinator = DebugCoordinator::new(
        Rc::new(PanickingDetach(backend.clone())),
        store.context(),
        DebugLimits::default(),
        Arc::new(|| Ok(())),
        NonZeroU64::new(1).unwrap(),
    )
    .unwrap();
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let request_id = accepted(
        handle
            .try_request(DebugRequest::ExceptionPolicy(ExceptionPolicy::All))
            .unwrap(),
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| coordinator.shutdown()));
    assert!(
        matches!(result,Ok(Err(DebugError::EngineFailure(ref r))) if r.code==FailureCode::HostPanic)
    );
    assert_eq!(coordinator.state(), DebugExecutionState::Closed);
    assert_eq!(
        backend.port().identities().state(),
        DebugExecutionState::Closed
    );
    assert_eq!(handle.state(), DebugExecutionState::Closed);
    assert_eq!(
        Arc::strong_count(&backend.signal),
        1,
        "the native signal lease is revoked despite detach panic"
    );
    assert!(matches!(
        read_reply(&handle, request_id),
        Err(DebugError::Closed)
    ));
    let calls = backend.signal.calls.load(Ordering::SeqCst);
    assert_eq!(handle.request_pause().unwrap_err(), DebugError::Closed);
    assert_eq!(backend.signal.calls.load(Ordering::SeqCst), calls);
    coordinator.shutdown().unwrap();
}

#[test]
fn inspection_accounting_reserves_complete_reply() {
    let (_store, backend, coordinator) = fixture_with_capabilities(
        DebugLimits {
            reply_bytes: 1024,
            ..DebugLimits::default()
        },
        DebugCapabilities::baseline().with(DebugCapability::LazyInspection),
    );
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    let scope = match backend
        .port()
        .identities()
        .register(pause, ReferenceKind::Scope, NonZeroU64::new(2).unwrap())
        .unwrap()
    {
        DebugReference::Scope(scope) => scope,
        _ => unreachable!(),
    };
    let request = DebugRequest::Inspect(InspectionRequest {
        parent: VariableParent::Scope(scope),
        page: Page {
            start: 0,
            count: usize::MAX,
        },
        cursor: None,
    });
    assert!(matches!(
        handle.try_request(request),
        Err(Rejected {
            reason: RejectionReason::Debug(DebugError::Capacity),
            ..
        })
    ));
    assert!(backend.seen.borrow().is_empty());
}

#[test]
fn admitted_write_target_remains_resolvable_until_native_capture() {
    let (_store, backend, coordinator) = fixture_with_capabilities(
        DebugLimits::default(),
        DebugCapabilities::baseline().with(DebugCapability::VariableMutation),
    );
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    let frame = backend.frame(pause);
    let port = backend.port();
    let key = NonZeroU64::new(7).unwrap();
    let binding = match port
        .identities()
        .register(pause, ReferenceKind::Binding, key)
        .unwrap()
    {
        DebugReference::Binding(binding) => binding,
        _ => unreachable!(),
    };
    let id = accepted(
        handle
            .try_request(DebugRequest::Write {
                target: WriteTarget::Binding(binding),
                frame,
                expression: JsString::from_utf8("1"),
            })
            .unwrap(),
    );
    assert!(matches!(
        port.next_directive(false),
        Some(DebugDirective::Request(DebugCommand {
            request: DebugRequest::Write { .. },
            ..
        }))
    ));
    assert_eq!(
        port.identities().resolve(DebugReference::Binding(binding)),
        Ok(key)
    );
    port.publish(DebugUpdate::Reply {
        id,
        result: Err(DebugError::Unsupported),
    })
    .unwrap();
    assert_eq!(
        read_reply(&handle, id).unwrap_err(),
        DebugError::Unsupported
    );
}

#[test]
fn inspection_reply_accounts_reserved_vector_and_nested_text_capacity() {
    let (_store, backend, coordinator) = fixture_with_capabilities(
        DebugLimits {
            reply_bytes: 1024,
            ..DebugLimits::default()
        },
        DebugCapabilities::baseline().with(DebugCapability::LazyInspection),
    );
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    let port = backend.port();
    let scope = match port
        .identities()
        .register(pause, ReferenceKind::Scope, NonZeroU64::new(2).unwrap())
        .unwrap()
    {
        DebugReference::Scope(scope) => scope,
        _ => unreachable!(),
    };
    for payload in 0..4 {
        let id = accepted(
            handle
                .try_request(DebugRequest::Inspect(InspectionRequest {
                    parent: VariableParent::Scope(scope),
                    page: Page { start: 0, count: 1 },
                    cursor: None,
                }))
                .unwrap(),
        );
        assert!(matches!(
            port.next_directive(false),
            Some(DebugDirective::Request(_))
        ));
        let mut page = InspectionPage {
            items: Vec::new(),
            next: None,
            complete: true,
            scanned_slots: 1,
            total: Some(1),
        };
        if payload == 0 {
            page.items = Vec::with_capacity(128);
        } else {
            let mut units = Vec::with_capacity(2048);
            units.push(b'x' as u16);
            let mut entry = InspectionEntry {
                variable: DebugVariable {
                    name: JsString::from_utf8("x"),
                    value: DebugValue {
                        preview: JsString::from_utf8("1"),
                        object: None,
                        accessor: false,
                        unavailable: None,
                    },
                },
                type_name: JsString::from_utf8("number"),
                write: None,
                write_unavailable: None,
            };
            match payload {
                1 => entry.type_name = JsString::from_units(units),
                2 => entry.variable.name = JsString::from_units(units),
                _ => entry.variable.value.preview = JsString::from_units(units),
            }
            page.items.push(entry);
        }
        port.publish(DebugUpdate::Reply {
            id,
            result: Ok(DebugReply::Inspection(page)),
        })
        .unwrap();
        assert_eq!(read_reply(&handle, id).unwrap_err(), DebugError::Capacity);
    }
}

#[test]
fn legacy_side_effectful_evaluation_revokes_cursors_but_retains_objects() {
    let (_store, backend, coordinator) = fixture(DebugLimits::default());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    let frame = backend.frame(pause);
    let port = backend.port();
    let key = NonZeroU64::new(8).unwrap();
    let cursor = port
        .identities()
        .register(pause, ReferenceKind::Cursor, key)
        .unwrap();
    let object = port
        .identities()
        .register(pause, ReferenceKind::Object, key)
        .unwrap();
    let id = accepted(
        handle
            .try_request(DebugRequest::Evaluate {
                frame,
                expression: JsString::from_utf8("1"),
                allow_side_effects: true,
            })
            .unwrap(),
    );
    backend.asynchronous.set(true);
    coordinator.service().unwrap();
    backend.finish_evaluation();
    assert!(matches!(
        read_reply(&handle, id),
        Ok(DebugReply::Evaluation { .. })
    ));
    assert_eq!(
        port.identities().resolve(cursor),
        Err(DebugError::StaleInspection)
    );
    assert_eq!(port.identities().resolve(object), Ok(key));
}

#[test]
fn new_optional_requests_remain_unsupported_on_baseline_backend() {
    let (_store, backend, coordinator) = fixture(DebugLimits::default());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    let frame = backend.frame(pause);
    for request in [
        DebugRequest::EvaluateWithOptions {
            frame,
            expression: JsString::from_utf8("1"),
            options: EvaluationOptions::default(),
        },
        DebugRequest::LookupPath {
            frame,
            path: JsString::from_utf8("x"),
        },
    ] {
        assert!(matches!(
            handle.try_request(request),
            Err(Rejected {
                reason: RejectionReason::Debug(DebugError::Unsupported),
                ..
            })
        ));
    }
    assert!(backend.seen.borrow().is_empty());
}

#[test]
fn stale_inspection_requests_fail_before_backend_access() {
    let caps = DebugCapabilities::baseline()
        .with(DebugCapability::LazyInspection)
        .with(DebugCapability::VariableMutation);
    let (_store, backend, coordinator) = fixture_with_capabilities(DebugLimits::default(), caps);
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    let frame = backend.frame(pause);
    let port = backend.port();
    let key = NonZeroU64::new(12).unwrap();
    let scope = match port
        .identities()
        .register(pause, ReferenceKind::Scope, key)
        .unwrap()
    {
        DebugReference::Scope(value) => value,
        _ => unreachable!(),
    };
    let cursor = match port
        .identities()
        .register(pause, ReferenceKind::Cursor, key)
        .unwrap()
    {
        DebugReference::Cursor(value) => value,
        _ => unreachable!(),
    };
    let binding = match port
        .identities()
        .register(pause, ReferenceKind::Binding, key)
        .unwrap()
    {
        DebugReference::Binding(value) => value,
        _ => unreachable!(),
    };
    port.identities().invalidate_inspection(pause).unwrap();
    for request in [
        DebugRequest::Inspect(InspectionRequest {
            parent: VariableParent::Scope(scope),
            page: Page { start: 0, count: 1 },
            cursor: Some(cursor),
        }),
        DebugRequest::Write {
            target: WriteTarget::Binding(binding),
            frame,
            expression: JsString::from_utf8("1"),
        },
        DebugRequest::ReleaseCursor(cursor),
    ] {
        let id = accepted(handle.try_request(request).unwrap());
        coordinator.service().unwrap();
        assert_eq!(
            read_reply(&handle, id).unwrap_err(),
            DebugError::StaleInspection
        );
    }
    assert!(backend.seen.borrow().is_empty());
}

#[test]
fn new_request_text_uses_backing_capacity_before_admission() {
    let caps = DebugCapabilities::baseline()
        .with(DebugCapability::SafeLookup)
        .with(DebugCapability::CancelEvaluation)
        .with(DebugCapability::VariableMutation);
    let (_store, backend, coordinator) = fixture_with_capabilities(
        DebugLimits {
            request_bytes: 1024,
            ..DebugLimits::default()
        },
        caps,
    );
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = backend.pause();
    let frame = backend.frame(pause);
    let binding = match backend
        .port()
        .identities()
        .register(pause, ReferenceKind::Binding, NonZeroU64::new(4).unwrap())
        .unwrap()
    {
        DebugReference::Binding(value) => value,
        _ => unreachable!(),
    };
    for kind in 0..3 {
        let mut units = Vec::with_capacity(4096);
        units.push(b'x' as u16);
        let text = JsString::from_units(units);
        let request = match kind {
            0 => DebugRequest::LookupPath { frame, path: text },
            1 => DebugRequest::Write {
                target: WriteTarget::Binding(binding),
                frame,
                expression: text,
            },
            _ => DebugRequest::EvaluateWithOptions {
                frame,
                expression: text,
                options: EvaluationOptions::default(),
            },
        };
        let rejected = handle.try_request(request).unwrap_err();
        assert_eq!(
            rejected.reason,
            RejectionReason::Debug(DebugError::Capacity)
        );
        let text = match rejected.value {
            DebugRequest::LookupPath { path, .. } => path,
            DebugRequest::Write { expression, .. }
            | DebugRequest::EvaluateWithOptions { expression, .. } => expression,
            _ => unreachable!(),
        };
        assert_eq!(text.as_units(), &[b'x' as u16]);
    }
    assert!(backend.seen.borrow().is_empty());
}
