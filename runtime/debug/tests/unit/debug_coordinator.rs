//! Owner-local metadata/cleanup defenses; this fixture does not execute JavaScript.
use super::*;
use crate::engine::{ContextLimits, EngineIdentity, JsString, ScriptSource, ValueStore};
use std::panic::{AssertUnwindSafe, catch_unwind};

struct Signal;
impl DebugSignal for Signal {
    fn wake(&self) -> Result<(), DebugError> {
        Ok(())
    }
}
struct Backend {
    descriptor: DebugDescriptor,
    port: RefCell<Option<Rc<dyn DebugPort>>>,
    session: Cell<Option<DebugSessionId>>,
    attach_error: Cell<bool>,
    detach_error: Cell<bool>,
    service_error: Cell<bool>,
    wrong_state: Cell<bool>,
    after_service: RefCell<Option<Box<dyn FnMut()>>>,
}
impl Backend {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            descriptor: DebugDescriptor {
                api_version: 1,
                capabilities: DebugCapabilities::baseline(),
                native_requirements: vec![],
                interrupt_mechanism: "Rust control metadata".into(),
            },
            port: RefCell::new(None),
            session: Cell::new(None),
            attach_error: Cell::new(false),
            detach_error: Cell::new(false),
            service_error: Cell::new(false),
            wrong_state: Cell::new(false),
            after_service: RefCell::new(None),
        })
    }
    fn port(&self) -> Rc<dyn DebugPort> {
        self.port.borrow().as_ref().unwrap().clone()
    }
}
impl DebugBackend for Backend {
    fn descriptor(&self) -> &DebugDescriptor {
        &self.descriptor
    }
    fn initialize(&self, port: Rc<dyn DebugPort>) -> Result<(), DebugError> {
        *self.port.borrow_mut() = Some(port);
        Ok(())
    }
    fn attach(&self, options: DebugAttachOptions) -> Result<Arc<dyn DebugSignal>, DebugError> {
        if self.attach_error.get() {
            return Err(DebugError::Unsupported);
        }
        self.session.set(Some(options.session));
        Ok(Arc::new(Signal))
    }
    fn service(&self) -> Result<DebugServiceReport, DebugError> {
        if self.service_error.get() {
            return Err(DebugError::EngineFailure(FailureReport::fault(
                FailureCode::EngineFailure,
                "fixture service",
            )));
        }
        let port = self.port();
        while let Some(directive) = port.next_directive(false) {
            match directive {
                DebugDirective::Detach | DebugDirective::Stop => self.detach()?,
                DebugDirective::Request(command) => port.publish(DebugUpdate::Reply {
                    id: command.id,
                    result: Err(DebugError::Unsupported),
                })?,
                _ => {}
            }
        }
        if let Some(after_service) = self.after_service.borrow_mut().as_mut() {
            after_service();
        }
        Ok(DebugServiceReport {
            made_progress: false,
            state: if self.wrong_state.get() {
                DebugExecutionState::Closed
            } else {
                port.identities().state()
            },
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
        if self.detach_error.get() {
            Err(DebugError::Unsupported)
        } else {
            Ok(())
        }
    }
}
fn store() -> ValueStore<()> {
    ValueStore::new(&EngineIdentity::new().unwrap(), ContextLimits::default()).unwrap()
}
fn coordinator(backend: Rc<Backend>, context: ContextId) -> Rc<DebugCoordinator> {
    DebugCoordinator::new(
        backend,
        context,
        DebugLimits::default(),
        Arc::new(|| Ok(())),
        NonZeroU64::new(1).unwrap(),
    )
    .unwrap()
}
fn poison_active(coordinator: &DebugCoordinator) {
    let control = coordinator.port.control.clone();
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = control.active.lock().unwrap();
            panic!("poison fixture");
        }))
        .is_err()
    );
}

#[test]
fn attach_failure_rolls_back_ids_and_a_retry_gets_a_fresh_session() {
    let store = store();
    let backend = Backend::new();
    let coordinator = coordinator(backend.clone(), store.context());
    poison_active(&coordinator);
    backend.attach_error.set(true);
    assert!(matches!(
        coordinator.attach(DebugSessionOptions::default()),
        Err(DebugError::Unsupported)
    ));
    assert_eq!(coordinator.state(), DebugExecutionState::Detached);
    backend.attach_error.set(false);
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    assert_eq!(handle.state(), DebugExecutionState::Running);
    coordinator.shutdown().unwrap();
    assert_eq!(coordinator.state(), DebugExecutionState::Closed);
    assert!(matches!(
        coordinator.attach(DebugSessionOptions::default()),
        Err(DebugError::Closed)
    ));
}
#[test]
fn shutdown_finishes_metadata_cleanup_even_if_native_detach_fails() {
    let store = store();
    let backend = Backend::new();
    let coordinator = coordinator(backend.clone(), store.context());
    let _handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    poison_active(&coordinator);
    backend.detach_error.set(true);
    assert_eq!(coordinator.shutdown(), Err(DebugError::Unsupported));
    assert_eq!(
        coordinator.port.identities.next_script(),
        Err(DebugError::Closed)
    );
    assert_eq!(coordinator.port.session().err(), Some(DebugError::Closed));
    assert!(coordinator.port.next_directive(false).is_none());
    coordinator.shutdown().unwrap();
    coordinator.service().unwrap();
}
#[test]
fn native_report_must_match_issued_metadata_and_service_errors_remain_detached() {
    let store = store();
    let backend = Backend::new();
    let coordinator = coordinator(backend.clone(), store.context());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    backend.wrong_state.set(true);
    handle.request_pause().unwrap();
    assert!(matches!(
        coordinator.service(),
        Err(DebugError::EngineFailure(_))
    ));
    backend.wrong_state.set(false);
    backend.service_error.set(true);
    handle.request_pause().unwrap();
    assert!(matches!(
        coordinator.service(),
        Err(DebugError::EngineFailure(_))
    ));
    backend.service_error.set(false);
    coordinator.shutdown().unwrap();
}
#[test]
fn invalid_static_debug_budgets_fail_before_backend_initialization() {
    let store = store();
    for limits in [
        DebugLimits {
            requests: 0,
            ..DebugLimits::default()
        },
        DebugLimits {
            dap_output_bytes: 1,
            ..DebugLimits::default()
        },
        DebugLimits {
            source_bytes: usize::MAX,
            ..DebugLimits::default()
        },
    ] {
        let backend = Backend::new();
        assert!(matches!(
            DebugCoordinator::new(
                backend.clone(),
                store.context(),
                limits,
                Arc::new(|| Ok(())),
                NonZeroU64::new(1).unwrap()
            ),
            Err(DebugError::Capacity)
        ));
        assert!(backend.port.borrow().is_none());
    }
}
#[test]
fn unattached_stop_and_closed_port_queries_never_touch_native_state() {
    let store = store();
    let backend = Backend::new();
    let coordinator = coordinator(backend.clone(), store.context());
    assert_eq!(
        coordinator.port.session().err(),
        Some(DebugError::NotAttached)
    );
    coordinator.service().unwrap();
    coordinator.before_entry().unwrap();
    assert!(matches!(
        coordinator.stop_signal()().unwrap(),
        DebugControlReceipt::Accepted
    ));
    assert_eq!(coordinator.before_entry(), Err(DebugError::Closed));
    coordinator.shutdown().unwrap();
    assert!(matches!(
        backend.port().resolve_breakpoint(
            &ScriptSelector {
                origin: "file:///a.js".into(),
                expected_digest: None
            },
            0,
            None
        ),
        Err(DebugError::Closed)
    ));
    let source = NativeScriptMetadata {
        key: NativeScriptKey::new(store.context(), NonZeroU64::new(1).unwrap()),
        source: ScriptSource::new(JsString::from_utf8("x"), "file:///a.js".into()),
        kind: ScriptKind::Script,
    };
    assert_eq!(
        backend.port().register_script(source),
        Err(DebugError::Closed)
    );
}
#[test]
fn source_requests_with_stale_frame_metadata_end_in_terminal_errors_before_dispatch() {
    let store = store();
    let backend = Backend::new();
    let coordinator = coordinator(backend.clone(), store.context());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let session = coordinator.port.identities.state();
    assert_eq!(session, DebugExecutionState::Running);
    let session_id = handle.session.id;
    let pause = coordinator.port.identities.begin_pause(session_id).unwrap();
    coordinator
        .port
        .publish(DebugUpdate::Paused {
            sequence: coordinator.port.identities.next_update_sequence().unwrap(),
            pause,
            reason: PauseReason::Requested,
            location: None,
            original: None,
            breakpoint_ids: vec![],
        })
        .unwrap();
    let frame = match coordinator
        .port
        .identities
        .register(pause, ReferenceKind::Frame, NonZeroU64::new(1).unwrap())
        .unwrap()
    {
        DebugReference::Frame(frame) => frame,
        _ => unreachable!(),
    };
    handle.try_request(DebugRequest::Scopes { frame }).unwrap();
    coordinator.port.identities.end_pause(pause).unwrap();
    assert!(coordinator.port.next_directive(false).is_none());
    assert!(matches!(
        handle.try_recv(),
        Some(DebugUpdate::Reply {
            result: Err(DebugError::StalePause),
            ..
        })
    ));
}

#[test]
fn stale_stack_and_both_variable_parents_are_rejected_before_backend_dispatch() {
    let store = store();
    let backend = Backend::new();
    let coordinator = coordinator(backend, store.context());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = coordinator
        .port
        .identities
        .begin_pause(handle.session.id)
        .unwrap();
    coordinator
        .port
        .publish(DebugUpdate::Paused {
            sequence: 1,
            pause,
            reason: PauseReason::Requested,
            location: None,
            original: None,
            breakpoint_ids: vec![],
        })
        .unwrap();
    let scope = match coordinator
        .port
        .identities
        .register(pause, ReferenceKind::Scope, NonZeroU64::new(1).unwrap())
        .unwrap()
    {
        DebugReference::Scope(scope) => scope,
        _ => unreachable!(),
    };
    let object = match coordinator
        .port
        .identities
        .register(pause, ReferenceKind::Object, NonZeroU64::new(2).unwrap())
        .unwrap()
    {
        DebugReference::Object(object) => object,
        _ => unreachable!(),
    };
    for request in [
        DebugRequest::Stack {
            pause,
            page: Page { start: 0, count: 1 },
        },
        DebugRequest::Variables {
            parent: VariableParent::Scope(scope),
            page: Page { start: 0, count: 1 },
        },
        DebugRequest::Variables {
            parent: VariableParent::Object(object),
            page: Page { start: 0, count: 1 },
        },
    ] {
        handle.try_request(request).unwrap();
    }
    // The transport still has the published pause, but the native metadata has
    // already invalidated it. The permanent port must perform its own check.
    coordinator.port.identities.end_pause(pause).unwrap();
    assert!(coordinator.port.next_directive(false).is_none());
    for _ in 0..3 {
        assert!(matches!(
            handle.try_recv(),
            Some(DebugUpdate::Reply {
                result: Err(DebugError::StalePause),
                ..
            })
        ));
    }
    coordinator.shutdown().unwrap();
}

#[test]
fn port_rejects_stale_native_state_publications_and_detach_invalidates_a_live_pause() {
    let store = store();
    let backend = Backend::new();
    let coordinator = coordinator(backend, store.context());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let pause = coordinator
        .port
        .identities
        .begin_pause(handle.session.id)
        .unwrap();
    let stale = PauseId {
        generation: pause.generation + 1,
        ..pause
    };
    assert_eq!(
        coordinator.port.publish(DebugUpdate::Paused {
            sequence: 1,
            pause: stale,
            reason: PauseReason::Requested,
            location: None,
            original: None,
            breakpoint_ids: vec![],
        }),
        Err(DebugError::StalePause)
    );
    assert_eq!(
        coordinator.port.publish(DebugUpdate::Resumed {
            sequence: 2,
            previous: pause
        }),
        Err(DebugError::InFlight)
    );
    let mut foreign = pause;
    foreign.session.generation += 1;
    assert_eq!(
        coordinator.port.publish(DebugUpdate::Resumed {
            sequence: 2,
            previous: foreign
        }),
        Err(DebugError::StaleSession)
    );
    assert_eq!(
        coordinator.port.publish(DebugUpdate::SessionClosed {
            session: foreign.session,
            reason: None
        }),
        Err(DebugError::StaleSession)
    );
    coordinator
        .port
        .finish_detach(&handle.session, None, false)
        .unwrap();
    assert_eq!(coordinator.state(), DebugExecutionState::Detached);
    assert_eq!(handle.state(), DebugExecutionState::Detached);
    assert!(matches!(
        handle.try_recv(),
        Some(DebugUpdate::SessionClosed { .. })
    ));
}

#[test]
fn failed_session_allocation_rolls_back_issued_metadata_before_retry() {
    let store = store();
    let backend = Backend::new();
    let coordinator = coordinator(backend.clone(), store.context());
    // The session's open Arc and copied descriptor precede the first fallible
    // transport allocation, which reserves its request slots.
    let (result, fired) = crate::vm::test_allocator::fail_nth(2, || {
        coordinator.attach(DebugSessionOptions::default())
    });
    assert!(fired);
    assert!(matches!(result, Err(DebugError::Capacity)));
    assert_eq!(coordinator.state(), DebugExecutionState::Detached);
    assert!(backend.session.get().is_none());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    assert_eq!(handle.session.id.generation, 2);
    coordinator.shutdown().unwrap();
}

#[test]
fn entry_guard_rechecks_stop_and_gate_release_after_each_native_service_boundary() {
    for stop in [false, true] {
        let store = store();
        let backend = Backend::new();
        let coordinator = coordinator(backend.clone(), store.context());
        let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
        handle.session.claim_entry_gate().unwrap();
        let session = handle.session.clone();
        let requests = VmDebugHandle {
            session: session.clone(),
        };
        let mut calls = 0;
        *backend.after_service.borrow_mut() = Some(Box::new(move || {
            calls += 1;
            if calls == 1 {
                if stop {
                    session.stop().unwrap();
                } else {
                    requests.request_pause().unwrap();
                }
            } else if !stop {
                session.release_entry_gate().unwrap();
            }
        }));
        handle.request_pause().unwrap();
        assert_eq!(
            coordinator.before_entry(),
            if stop {
                Err(DebugError::Closed)
            } else {
                Ok(())
            }
        );
        *backend.after_service.borrow_mut() = None;
        coordinator.shutdown().unwrap();
    }
    let store = store();
    let backend = Backend::new();
    let coordinator = coordinator(backend, store.context());
    let _handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    coordinator.stop_signal()().unwrap();
    assert_eq!(coordinator.before_entry(), Err(DebugError::Closed));
}

#[test]
fn shutdown_catches_signal_disposal_panics_and_keeps_the_port_permanently_closed() {
    struct PanicOnDrop;
    impl DebugSignal for PanicOnDrop {
        fn wake(&self) -> Result<(), DebugError> {
            Ok(())
        }
    }
    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("signal destructor failure");
        }
    }
    let store = store();
    let backend = Backend::new();
    let coordinator = coordinator(backend, store.context());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    handle.session.signal.install(Arc::new(PanicOnDrop));
    // The native session has already unwound, leaving its transferable signal
    // lease for the coordinator's final session-disposal step.
    coordinator
        .port
        .identities
        .close_session(handle.session.id)
        .unwrap();
    match coordinator.shutdown() {
        Err(DebugError::EngineFailure(report)) => {
            assert_eq!(report.code, FailureCode::HostPanic);
            assert_eq!(
                report.message.unwrap().to_utf8().unwrap(),
                "debug session disposal panicked"
            );
        }
        _ => panic!("signal disposal panic must become a detached host failure"),
    }
    assert_eq!(coordinator.state(), DebugExecutionState::Closed);
    assert_eq!(coordinator.port.sources.borrow().retained_bytes(), 0);
    assert_eq!(coordinator.port.session().err(), Some(DebugError::Closed));
    coordinator.shutdown().unwrap();
}

#[test]
fn accepted_wake_fault_is_reported_after_owner_service_finishes_native_detach() {
    let store = store();
    let backend = Backend::new();
    let coordinator = DebugCoordinator::new(
        backend,
        store.context(),
        DebugLimits::default(),
        Arc::new(|| {
            Err(FailureReport::fault(
                FailureCode::EngineFailure,
                "owner wake failed",
            ))
        }),
        NonZeroU64::new(1).unwrap(),
    )
    .unwrap();
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    assert!(matches!(
        handle.request_pause().unwrap(),
        DebugControlReceipt::AcceptedWakeFailed {
            error: DebugError::WakeFailed(_)
        }
    ));
    assert!(matches!(
        coordinator.service(),
        Err(DebugError::WakeFailed(_))
    ));
    assert_eq!(coordinator.state(), DebugExecutionState::Detached);
    assert!(matches!(
        handle.try_recv(),
        Some(DebugUpdate::SessionClosed {
            reason: Some(_),
            ..
        })
    ));
}

#[test]
fn unsupported_backend_fails_before_permanent_hooks_are_initialized() {
    let store = store();
    let mut backend = Backend::new();
    Rc::get_mut(&mut backend).unwrap().descriptor.capabilities = DebugCapabilities::empty();
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
    assert!(backend.port.borrow().is_none());
}

#[test]
fn detached_or_closed_execution_metadata_never_reenters_the_native_dispatcher() {
    let store = store();
    let backend = Backend::new();
    let coordinator = coordinator(backend.clone(), store.context());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    *backend.after_service.borrow_mut() = Some(Box::new(|| {
        panic!("detached native execution must not be serviced")
    }));
    for terminal in [DebugExecutionState::Detached, DebugExecutionState::Closed] {
        // Model terminal execution metadata arriving before the owner's next
        // mailbox observation. Pending controls must not reopen native service.
        handle.session.lock().execution = terminal;
        handle.request_pause().unwrap();
        coordinator.service().unwrap();
    }
    *backend.after_service.borrow_mut() = None;
    handle.session.lock().execution = DebugExecutionState::Running;
    coordinator.shutdown().unwrap();
}

#[test]
fn entry_guard_waits_for_configuration_release_and_rejects_permanent_closure() {
    use std::sync::mpsc;
    use std::time::Duration;
    let store = store();
    let backend = Backend::new();
    let coordinator = coordinator(backend.clone(), store.context());
    let handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
    coordinator.before_entry().unwrap();
    handle.session.claim_entry_gate().unwrap();
    let (serviced_tx, serviced_rx) = mpsc::sync_channel(1);
    *backend.after_service.borrow_mut() = Some(Box::new(move || serviced_tx.send(()).unwrap()));
    let session = handle.session.clone();
    let release = std::thread::spawn(move || {
        serviced_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // Keep the configuration predicate held while the owner reaches its
        // native-control-only Condvar wait, then release it from the consumer.
        std::thread::sleep(Duration::from_millis(20));
        session.release_entry_gate().unwrap();
    });
    handle.request_pause().unwrap();
    coordinator.before_entry().unwrap();
    release.join().unwrap();
    *backend.after_service.borrow_mut() = None;
    assert!(!handle.session.entry_held());
    coordinator.shutdown().unwrap();
    assert_eq!(coordinator.before_entry(), Err(DebugError::Closed));
}
