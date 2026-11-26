use super::*;
use crate::engine::{ContextLimits, EngineIdentity, ValueStore};
use std::sync::{atomic::AtomicUsize, mpsc};
use std::time::Duration;
struct BlockingSignal {
    entered: mpsc::SyncSender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    alive: AtomicBool,
    calls: AtomicUsize,
}
impl DebugSignal for BlockingSignal {
    fn wake(&self) -> Result<(), DebugError> {
        assert!(self.alive.load(Ordering::Acquire));
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.send(()).unwrap();
        self.release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| {
                DebugError::WakeFailed(FailureReport::fault(
                    FailureCode::EngineFailure,
                    "signal revocation deadlocked behind transport lock",
                ))
            })?;
        assert!(self.alive.load(Ordering::Acquire));
        Ok(())
    }
}
fn session() -> Arc<Session> {
    session_with_control(ContextControl::new(Arc::new(|| Ok(()))))
}
fn session_with_control(control: Arc<ContextControl>) -> Arc<Session> {
    let identity = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&identity, ContextLimits::default()).unwrap();
    let ids =
        DebugIdentities::new(std::num::NonZeroU64::new(1).unwrap(), store.context(), 8).unwrap();
    let id = ids.open_session().unwrap();
    Session::new(
        id,
        false,
        ids.request_issuer(id).unwrap(),
        DebugDescriptor {
            api_version: 1,
            capabilities: DebugCapabilities::baseline(),
            native_requirements: vec![],
            interrupt_mechanism: "Rust signal race fixture".to_owned(),
        },
        DebugLimits::default(),
        control,
    )
    .unwrap()
}
#[test]
fn close_releases_transport_lock_and_waits_for_entered_native_signal_before_revocation() {
    let session = session();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let signal = Arc::new(BlockingSignal {
        entered: entered_tx,
        release: Mutex::new(release_rx),
        alive: AtomicBool::new(true),
        calls: AtomicUsize::new(0),
    });
    session.signal.install(signal.clone());
    let producer_session = session.clone();
    let producer = std::thread::spawn(move || {
        let handle = VmDebugHandle {
            session: producer_session,
        };
        assert!(matches!(
            handle.request_pause().unwrap(),
            DebugControlReceipt::Accepted
        ));
        handle
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let observer_session = session.clone();
    let observer_signal = signal.clone();
    let observer = std::thread::spawn(move || {
        assert!(matches!(
            observer_session.recv(true),
            Some(DebugUpdate::SessionClosed { .. })
        ));
        assert!(observer_signal.alive.load(Ordering::Acquire));
        release_tx.send(()).unwrap();
    });
    session.close(None, true);
    signal.alive.store(false, Ordering::Release);
    observer.join().unwrap();
    let handle = producer.join().unwrap();
    let calls = signal.calls.load(Ordering::SeqCst);
    assert_eq!(handle.request_pause().unwrap_err(), DebugError::Closed);
    assert_eq!(signal.calls.load(Ordering::SeqCst), calls);
}

#[test]
fn idle_pending_and_entry_checks_do_not_wait_for_the_transport_mutex() {
    let session = session();
    let guard = session.lock();
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let observer_session = session.clone();
    let observer = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        result_tx
            .send((observer_session.pending(), observer_session.entry_held()))
            .unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let observed_while_locked = result_rx.recv_timeout(Duration::from_millis(100));
    drop(guard);
    observer.join().unwrap();
    assert_eq!(
        observed_while_locked.unwrap(),
        (false, false),
        "idle checks must use atomic metadata without taking the state mutex"
    );
}

#[test]
fn published_controls_and_gate_changes_refresh_the_atomic_hints() {
    let session = session();
    assert!(!session.pending());
    assert!(!session.entry_held());
    session.claim_entry_gate().unwrap();
    assert!(session.entry_held());
    assert!(!session.pending());
    session.release_entry_gate().unwrap();
    assert!(!session.entry_held());
    session.pause().unwrap();
    assert!(session.pending());
    assert!(matches!(session.next(false), Some(DebugDirective::Pause)));
    assert!(!session.pending());
    session.detach().unwrap();
    assert!(session.pending());
    assert!(matches!(session.next(false), Some(DebugDirective::Detach)));
    assert!(!session.pending());
    session.stop().unwrap();
    assert!(session.pending());
    assert!(matches!(session.next(false), Some(DebugDirective::Stop)));
    assert!(!session.pending());
    session.close(None, true);
    assert!(!session.pending());
    assert!(!session.entry_held());
}

#[test]
fn publication_after_each_consumer_clear_is_never_hidden_by_a_false_hint() {
    let session = session();
    let producer_session = session.clone();
    let producer = std::thread::spawn(move || {
        let handle = VmDebugHandle {
            session: producer_session,
        };
        for _ in 0..128 {
            let id = match handle
                .try_request(DebugRequest::Sources {
                    page: Page { start: 0, count: 1 },
                })
                .unwrap()
            {
                DebugRequestReceipt::Accepted { id }
                | DebugRequestReceipt::AcceptedWakeFailed { id, .. } => id,
            };
            match handle.wait_update().unwrap() {
                DebugUpdate::Reply {
                    id: actual,
                    result: Ok(DebugReply::Sources { total: 0, .. }),
                } => assert_eq!(actual, id),
                _ => panic!("reserved terminal reply"),
            }
        }
        handle
    });
    for _ in 0..128 {
        // The false hint may race with publication. The authoritative next(true)
        // predicate and Condvar use the same mutex as the publishing/clear stores.
        let directive = if session.pending() {
            session.next(false)
        } else {
            session.next(true)
        };
        let DebugDirective::Request(command) = directive.unwrap() else {
            panic!("source request")
        };
        session
            .publish(DebugUpdate::Reply {
                id: command.id,
                result: Ok(DebugReply::Sources {
                    scripts: Vec::new(),
                    total: 0,
                }),
            })
            .unwrap();
    }
    let _handle = producer.join().unwrap();
    assert!(!session.pending());
    session.close(None, true);
}

fn command(session: &Session, request: DebugRequest) -> DebugRequestId {
    match session.request(request).unwrap() {
        DebugRequestReceipt::Accepted { id }
        | DebugRequestReceipt::AcceptedWakeFailed { id, .. } => id,
    }
}
fn pause_id(session: &Session) -> PauseId {
    PauseId {
        session: session.id,
        generation: 1,
    }
}
fn publish_pause(session: &Session) -> PauseId {
    let pause = pause_id(session);
    session
        .publish(DebugUpdate::Paused {
            sequence: 1,
            pause,
            reason: PauseReason::Requested,
            location: None,
            original: None,
            breakpoint_ids: vec![],
        })
        .unwrap();
    pause
}
fn empty_sources() -> DebugReply {
    DebugReply::Sources {
        scripts: vec![],
        total: 0,
    }
}
fn dispatch(session: &Session, id: DebugRequestId) {
    assert!(
        matches!(session.next(false),Some(DebugDirective::Request(DebugCommand{id:actual,..})) if actual==id)
    );
}

#[test]
fn a_reserved_reply_rejects_unknown_early_duplicate_and_foreign_native_replies() {
    let session = session();
    let id = command(
        &session,
        DebugRequest::Sources {
            page: Page { start: 0, count: 1 },
        },
    );
    assert_eq!(
        session.publish(DebugUpdate::Reply {
            id,
            result: Ok(empty_sources())
        }),
        Err(DebugError::InvalidRequest)
    );
    let mut unknown = id;
    unknown.sequence = u64::MAX;
    assert_eq!(
        session.publish(DebugUpdate::Reply {
            id: unknown,
            result: Ok(empty_sources())
        }),
        Err(DebugError::InvalidRequest)
    );
    let mut foreign = id;
    foreign.session.generation += 1;
    assert_eq!(
        session.publish(DebugUpdate::Reply {
            id: foreign,
            result: Ok(empty_sources())
        }),
        Err(DebugError::StaleSession)
    );
    dispatch(&session, id);
    session
        .publish(DebugUpdate::Reply {
            id,
            result: Ok(empty_sources()),
        })
        .unwrap();
    assert_eq!(
        session.publish(DebugUpdate::Reply {
            id,
            result: Ok(empty_sources())
        }),
        Err(DebugError::InvalidRequest)
    );
    session.close(None, true);
    assert_eq!(
        session.publish(DebugUpdate::Reply {
            id,
            result: Ok(empty_sources())
        }),
        Err(DebugError::Closed)
    );
    assert!(matches!(
        session.recv(false),
        Some(DebugUpdate::Reply { result: Ok(_), .. })
    ));
    assert!(matches!(
        session.recv(false),
        Some(DebugUpdate::SessionClosed { .. })
    ));
    assert!(session.recv(false).is_none());
}
#[test]
fn oversized_reply_becomes_a_reserved_capacity_error_and_error_reports_are_bounded() {
    use crate::engine::JsString;
    let session = session();
    let id = command(
        &session,
        DebugRequest::Sources {
            page: Page { start: 0, count: 1 },
        },
    );
    dispatch(&session, id);
    session
        .publish(DebugUpdate::Reply {
            id,
            result: Ok(DebugReply::Source {
                text: JsString::from_units(vec![1; session.limits.reply_bytes]),
                total_units: session.limits.reply_bytes,
            }),
        })
        .unwrap();
    assert!(matches!(
        session.recv(false),
        Some(DebugUpdate::Reply {
            result: Err(DebugError::Capacity),
            ..
        })
    ));
    for engine in [false, true] {
        let id = command(
            &session,
            DebugRequest::Sources {
                page: Page { start: 0, count: 1 },
            },
        );
        dispatch(&session, id);
        let mut report = FailureReport::fault(FailureCode::EngineFailure, "oversized");
        report.message = Some(JsString::from_units(vec![1; 9000]));
        let error = if engine {
            DebugError::EngineFailure(report)
        } else {
            DebugError::WakeFailed(report)
        };
        session
            .publish(DebugUpdate::Reply {
                id,
                result: Err(error),
            })
            .unwrap();
        match session.recv(false).unwrap() {
            DebugUpdate::Reply {
                result: Err(DebugError::EngineFailure(report) | DebugError::WakeFailed(report)),
                ..
            } => {
                assert!(report.truncated);
                assert_eq!(report.message.unwrap().as_units().len(), 4096)
            }
            _ => panic!("bounded terminal error"),
        }
    }
}
#[test]
fn queued_second_evaluation_and_pause_mismatches_fail_before_native_dispatch() {
    use crate::engine::JsString;
    let session = session();
    let pause = publish_pause(&session);
    let frame = FrameRef {
        pause,
        generation: 1,
        slot: 0,
    };
    let request = || DebugRequest::Evaluate {
        frame,
        expression: JsString::from_utf8("fixture expression"),
        allow_side_effects: true,
    };
    let first = command(&session, request());
    let second = command(&session, request());
    dispatch(&session, first);
    assert!(session.next(false).is_none());
    assert!(
        matches!(session.recv(false),Some(DebugUpdate::Reply{id,result:Err(DebugError::InFlight)}) if id==second)
    );
    session
        .publish(DebugUpdate::Resumed {
            sequence: 2,
            previous: pause,
        })
        .unwrap();
    assert!(
        matches!(session.recv(false),Some(DebugUpdate::Reply{id,result:Err(DebugError::StalePause)}) if id==first)
    );
    assert!(matches!(
        session.request(DebugRequest::Scopes { frame }),
        Err(Rejected {
            reason: RejectionReason::Debug(DebugError::NotPaused),
            ..
        })
    ));
    assert!(matches!(
        session.request(DebugRequest::Variables {
            parent: VariableParent::Object(ObjectRef {
                pause,
                generation: 1,
                slot: 0
            }),
            page: Page { start: 0, count: 1 }
        }),
        Err(Rejected {
            reason: RejectionReason::Debug(DebugError::NotPaused),
            ..
        })
    ));
}
#[test]
fn stale_state_sequences_and_foreign_closures_do_not_replace_authoritative_state() {
    let session = session();
    let pause = publish_pause(&session);
    assert!(matches!(
        session.pause().unwrap(),
        DebugControlReceipt::AlreadyRequested
    ));
    let mut foreign = pause;
    foreign.session.generation += 1;
    assert_eq!(
        session.publish(DebugUpdate::Paused {
            sequence: 2,
            pause: foreign,
            reason: PauseReason::Requested,
            location: None,
            original: None,
            breakpoint_ids: vec![],
        }),
        Err(DebugError::StaleSession)
    );
    assert_eq!(
        session.publish(DebugUpdate::Resumed {
            sequence: 2,
            previous: foreign
        }),
        Err(DebugError::StaleSession)
    );
    assert_eq!(
        session.publish(DebugUpdate::Paused {
            sequence: 1,
            pause,
            reason: PauseReason::Requested,
            location: None,
            original: None,
            breakpoint_ids: vec![],
        }),
        Err(DebugError::StalePause)
    );
    assert_eq!(
        session.publish(DebugUpdate::Resumed {
            sequence: 1,
            previous: pause
        }),
        Err(DebugError::StalePause)
    );
    assert_eq!(
        session.publish(DebugUpdate::SessionClosed {
            session: foreign.session,
            reason: None
        }),
        Err(DebugError::StaleSession)
    );
    let mut stale = pause;
    stale.generation += 1;
    assert_eq!(
        session
            .continue_execution(stale, DebugContinue::Resume)
            .unwrap_err(),
        DebugError::StalePause
    );
    assert!(matches!(
        session.request(DebugRequest::Stack {
            pause: stale,
            page: Page { start: 0, count: 1 }
        }),
        Err(Rejected {
            reason: RejectionReason::Debug(DebugError::StalePause),
            ..
        })
    ));
    assert_eq!(session.lock().execution, DebugExecutionState::Paused(pause));
}
#[test]
fn bounded_updates_preserve_queue_and_byte_capacity_without_affecting_urgent_control() {
    use crate::engine::{JsString, SourceDigest};
    let session = session();
    let id = ScriptId {
        issuer_epoch: session.id.issuer_epoch,
        engine_epoch: session.id.engine_epoch,
        context_generation: session.id.context_generation,
        sequence: 1,
    };
    let info = |origin: String| ScriptInfo {
        registration: ScriptRegistration {
            id,
            revision: 1,
            digest: SourceDigest::of_utf16(&JsString::from_utf8("x")),
        },
        origin,
        length_units: 1,
    };
    let mut huge = String::with_capacity(session.limits.reply_bytes * 2);
    huge.push('x');
    assert_eq!(
        session.publish(DebugUpdate::ScriptRegistered(info(huge))),
        Err(DebugError::Capacity)
    );
    for _ in 0..session.limits.updates {
        session
            .publish(DebugUpdate::ScriptRegistered(info("file:///x.js".into())))
            .unwrap();
    }
    assert_eq!(
        session.publish(DebugUpdate::ScriptRegistered(info(
            "file:///extra.js".into()
        ))),
        Err(DebugError::Capacity)
    );
    session.stop().unwrap();
    assert!(matches!(session.next(false), Some(DebugDirective::Stop)));
    session.close(None, true);
    assert_eq!(
        session.publish(DebugUpdate::ScriptRegistered(info(
            "file:///late.js".into()
        ))),
        Err(DebugError::Closed)
    );
}
#[test]
fn context_without_session_reports_wake_failure_and_poisoned_mutexes_recover() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let control = ContextControl::new(Arc::new(|| {
        Err(FailureReport::fault(
            FailureCode::EngineFailure,
            "owner wake",
        ))
    }));
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = control.active.lock().unwrap();
            panic!("fixture poison");
        }))
        .is_err()
    );
    assert!(matches!(
        control.stop().unwrap(),
        DebugControlReceipt::AcceptedWakeFailed {
            error: DebugError::WakeFailed(_)
        }
    ));
    let session = session();
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = session.state.lock().unwrap();
            panic!("fixture poison");
        }))
        .is_err()
    );
    assert!(session.next(false).is_none());
    assert!(session.recv(false).is_none());
    session.claim_entry_gate().unwrap();
    session.release_entry_gate().unwrap();
    let lease = SignalLease::new();
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = lease.signal.write().unwrap();
            panic!("fixture poison");
        }))
        .is_err()
    );
    struct Immediate;
    impl DebugSignal for Immediate {
        fn wake(&self) -> Result<(), DebugError> {
            Ok(())
        }
    }
    lease.install(Arc::new(Immediate));
    lease.wake().unwrap();
    lease.revoke();
}

#[test]
fn failed_owner_wake_keeps_the_accepted_request_and_also_attempts_native_signal() {
    struct CountingSignal(Arc<AtomicUsize>);
    impl DebugSignal for CountingSignal {
        fn wake(&self) -> Result<(), DebugError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let session = session_with_control(ContextControl::new(Arc::new(|| {
        Err(FailureReport::fault(
            FailureCode::EngineFailure,
            "owner no longer available",
        ))
    })));
    let calls = Arc::new(AtomicUsize::new(0));
    session
        .signal
        .install(Arc::new(CountingSignal(calls.clone())));
    let id = match session
        .request(DebugRequest::Sources {
            page: Page { start: 0, count: 1 },
        })
        .unwrap()
    {
        DebugRequestReceipt::AcceptedWakeFailed {
            id,
            error: DebugError::WakeFailed(_),
        } => id,
        _ => panic!("accepted wake failure"),
    };
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(session.control.stopping.load(Ordering::Acquire));
    assert!(matches!(session.next(false), Some(DebugDirective::Stop)));
    session.close(None, true);
    assert!(
        matches!(session.recv(false),Some(DebugUpdate::Reply{id:actual,result:Err(DebugError::Closed)}) if actual==id)
    );
}

#[test]
fn entry_gate_yields_for_a_continue_decision_and_cannot_reopen_after_close() {
    let session = session();
    session.claim_entry_gate().unwrap();
    assert_eq!(session.claim_entry_gate(), Err(DebugError::InFlight));
    let pause = publish_pause(&session);
    session
        .continue_execution(pause, DebugContinue::Resume)
        .unwrap();
    assert!(matches!(
        session.continue_execution(pause, DebugContinue::StepOver),
        Err(DebugError::InFlight)
    ));
    session.wait_entry_work();
    assert!(matches!(
        session.next(false),
        Some(DebugDirective::Continue { .. })
    ));
    session.close(None, true);
    assert_eq!(session.release_entry_gate(), Err(DebugError::Closed));
    assert_eq!(session.claim_entry_gate(), Err(DebugError::Closed));
    session.wait_entry_work();
}

#[test]
fn poisoned_waiters_recheck_authoritative_entry_directive_and_reply_state() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    #[derive(Clone, Copy)]
    enum Wait {
        Entry,
        Directive,
        Reply,
    }
    for kind in [Wait::Entry, Wait::Directive, Wait::Reply] {
        let session = session();
        if matches!(kind, Wait::Entry) {
            session.claim_entry_gate().unwrap();
        }
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        let reader = session.clone();
        let waiter = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            match kind {
                Wait::Entry => {
                    reader.wait_entry_work();
                    assert!(!reader.entry_held());
                }
                Wait::Directive => {
                    assert!(matches!(reader.next(true), Some(DebugDirective::Pause)))
                }
                Wait::Reply => assert!(matches!(
                    reader.recv(true),
                    Some(DebugUpdate::Diagnostic { dropped: 0, .. })
                )),
            }
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // Keep the predicate unchanged while the known-empty waiter parks. This
        // exercises poison recovery on waking, rather than just its initial lock.
        std::thread::sleep(Duration::from_millis(10));
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                let _guard = session.state.lock().unwrap();
                panic!("fault injection while waiter is parked");
            }))
            .is_err()
        );
        match kind {
            Wait::Entry => session.release_entry_gate().unwrap(),
            Wait::Directive => {
                session.pause().unwrap();
            }
            Wait::Reply => session
                .publish(DebugUpdate::Diagnostic {
                    report: FailureReport::fault(FailureCode::Capacity, "wake reader"),
                    dropped: 0,
                })
                .unwrap(),
        }
        waiter.join().unwrap();
    }
}

#[test]
fn pending_breakpoint_reservations_charge_other_selectors_and_release_on_failure() {
    use crate::engine::JsString;
    let mut session = session();
    let limits = &mut Arc::get_mut(&mut session).unwrap().limits;
    limits.breakpoints = 2;
    limits.condition_bytes = 12;
    let request = |origin: &str| DebugRequest::SetBreakpoints {
        selector: ScriptSelector {
            origin: origin.into(),
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
    let first = command(&session, request("file:///a.js"));
    assert!(
        matches!(
            session.request(request("file:///b.js")),
            Err(Rejected {
                reason: RejectionReason::Debug(DebugError::Capacity),
                ..
            })
        ),
        "pending condition allowance belongs to the admitted first selector"
    );
    dispatch(&session, first);
    session
        .publish(DebugUpdate::Reply {
            id: first,
            result: Err(DebugError::InvalidRequest),
        })
        .unwrap();
    session.recv(false).unwrap();
    assert!(
        session.request(request("file:///b.js")).is_ok(),
        "terminal failure rolls the provisional budget back"
    );
}

#[test]
fn request_budget_and_checked_arithmetic_errors_need_no_oversized_allocations() {
    use crate::engine::JsString;
    let session = session();
    let descriptor = &session.descriptor;
    let pause = pause_id(&session);
    let frame = FrameRef {
        pause,
        generation: 1,
        slot: 0,
    };
    let evaluate = |text: JsString, allow_side_effects| DebugRequest::Evaluate {
        frame,
        expression: text,
        allow_side_effects,
    };
    assert_eq!(
        request_footprint(
            &evaluate(JsString::from_utf8("two"), true),
            &DebugLimits {
                expression_units: 2,
                ..DebugLimits::default()
            },
            descriptor
        ),
        Err(DebugError::Capacity)
    );
    assert_eq!(
        request_footprint(
            &evaluate(JsString::from_utf8("one"), false),
            &DebugLimits::default(),
            descriptor
        ),
        Err(DebugError::Unsupported)
    );
    let breakpoint = |column, condition| DebugRequest::SetBreakpoints {
        selector: ScriptSelector {
            origin: "file:///a.js".into(),
            expected_digest: None,
        },
        breakpoints: vec![BreakpointSpec {
            client_id: 1,
            line: 0,
            column,
            condition,
            enabled: true,
            hit_condition: None,
            log_message: None,
        }],
    };
    assert_eq!(
        request_footprint(
            &breakpoint(Some(0), None),
            &DebugLimits::default(),
            descriptor
        ),
        Err(DebugError::Unsupported)
    );
    assert_eq!(
        request_footprint(
            &breakpoint(None, None),
            &DebugLimits {
                breakpoints: 0,
                ..DebugLimits::default()
            },
            descriptor
        ),
        Err(DebugError::Capacity)
    );
    assert_eq!(
        request_footprint(
            &breakpoint(None, Some(JsString::from_utf8("long"))),
            &DebugLimits {
                condition_bytes: 2,
                ..DebugLimits::default()
            },
            descriptor
        ),
        Err(DebugError::Capacity)
    );
    assert_eq!(
        request_footprint(
            &DebugRequest::Sources {
                page: Page { start: 0, count: 1 }
            },
            &DebugLimits {
                request_bytes: 1,
                ..DebugLimits::default()
            },
            descriptor
        ),
        Err(DebugError::Capacity)
    );
    assert_eq!(checked_add(usize::MAX, 1), Err(DebugError::Capacity));
    assert_eq!(
        vector_bytes::<DebugFrame>(usize::MAX),
        Err(DebugError::Capacity)
    );
    let request = breakpoint(None, None);
    let mut state = session.lock();
    state.breakpoint_budgets[0] = Some(BreakpointBudget {
        origin: "file:///other.js".into(),
        digest: None,
        count: usize::MAX,
        bytes: 0,
        pending: None,
    });
    state.breakpoint_budgets[1] = Some(BreakpointBudget {
        origin: "file:///another.js".into(),
        digest: None,
        count: 1,
        bytes: 0,
        pending: None,
    });
    assert_eq!(
        breakpoint_budget(&request, &state, &DebugLimits::default()),
        Err(DebugError::Capacity)
    );
    state.breakpoint_budgets[0].as_mut().unwrap().count = 0;
    state.breakpoint_budgets[0].as_mut().unwrap().bytes = usize::MAX;
    state.breakpoint_budgets[1].as_mut().unwrap().count = 0;
    state.breakpoint_budgets[1].as_mut().unwrap().bytes = 1;
    assert_eq!(
        breakpoint_budget(&request, &state, &DebugLimits::default()),
        Err(DebugError::Capacity)
    );
}

#[test]
fn session_reservations_fail_transactionally_at_each_fallible_allocation() {
    let identity = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&identity, ContextLimits::default()).unwrap();
    let ids =
        DebugIdentities::new(std::num::NonZeroU64::new(1).unwrap(), store.context(), 8).unwrap();
    let id = ids.open_session().unwrap();
    let descriptor = || DebugDescriptor {
        api_version: 1,
        capabilities: DebugCapabilities::baseline(),
        native_requirements: vec![],
        interrupt_mechanism: String::new(),
    };
    for allocation in 0..5 {
        let issuer = ids.request_issuer(id).unwrap();
        let control = ContextControl::new(Arc::new(|| Ok(())));
        let (result, fired) = crate::vm::test_allocator::fail_nth(allocation, || {
            Session::new(
                id,
                false,
                issuer,
                descriptor(),
                DebugLimits::default(),
                control,
            )
        });
        assert!(fired, "allocation {allocation}");
        assert!(matches!(result, Err(DebugError::Capacity)));
    }
    for limits in [
        DebugLimits {
            requests: 0,
            ..DebugLimits::default()
        },
        DebugLimits {
            breakpoints: usize::MAX,
            ..DebugLimits::default()
        },
    ] {
        assert!(matches!(
            Session::new(
                id,
                false,
                ids.request_issuer(id).unwrap(),
                descriptor(),
                limits,
                ContextControl::new(Arc::new(|| Ok(())))
            ),
            Err(DebugError::Capacity)
        ));
    }
    let session = Session::new(
        id,
        false,
        ids.request_issuer(id).unwrap(),
        descriptor(),
        DebugLimits::default(),
        ContextControl::new(Arc::new(|| Ok(()))),
    )
    .unwrap();
    ids.close_session(id).unwrap();
    let rejected = session
        .request(DebugRequest::Sources {
            page: Page { start: 0, count: 1 },
        })
        .unwrap_err();
    assert!(matches!(
        rejected.reason,
        RejectionReason::Debug(DebugError::Closed)
    ));
    assert!(session.lock().slots.iter().all(Option::is_none));
    assert!(!session.pending());
}

#[test]
fn repeated_context_stop_and_pending_selector_replacements_are_coalesced() {
    let control = ContextControl::new(Arc::new(|| Ok(())));
    assert!(matches!(
        control.stop().unwrap(),
        DebugControlReceipt::Accepted
    ));
    assert!(matches!(
        control.stop().unwrap(),
        DebugControlReceipt::AlreadyRequested
    ));
    let session = session();
    let request = || DebugRequest::SetBreakpoints {
        selector: ScriptSelector {
            origin: "file:///a.js".into(),
            expected_digest: None,
        },
        breakpoints: vec![BreakpointSpec {
            client_id: 1,
            line: 0,
            column: None,
            condition: None,
            enabled: true,
            hit_condition: None,
            log_message: None,
        }],
    };
    let id = command(&session, request());
    assert!(matches!(
        session.request(request()),
        Err(Rejected {
            reason: RejectionReason::Debug(DebugError::InFlight),
            ..
        })
    ));
    dispatch(&session, id);
    session
        .publish(DebugUpdate::Reply {
            id,
            result: Err(DebugError::InvalidRequest),
        })
        .unwrap();
    session.recv(false).unwrap();
    assert!(session.request(request()).is_ok());
    session
        .publish(DebugUpdate::SessionClosed {
            session: session.id,
            reason: None,
        })
        .unwrap();
    assert_eq!(session.lock().execution, DebugExecutionState::Detached);
}

#[test]
fn stale_queue_slots_and_repeated_terminal_completion_cannot_duplicate_replies() {
    let session = session();
    let id = command(
        &session,
        DebugRequest::Sources {
            page: Page { start: 0, count: 1 },
        },
    );
    let mut state = session.lock();
    let slot = state.queued[0];
    finish(&mut state, slot, Ok(empty_sources()));
    finish(&mut state, slot, Err(DebugError::Closed));
    assert_eq!(state.replies.len(), 1);
    // Queue traversal tolerates both a stale empty slot and a completed slot;
    // neither may be dispatched again if stale bookkeeping reaches the pump.
    state.queued.push_back(slot + 1);
    state.queued.push_back(slot);
    drop(state);
    assert!(session.next(false).is_none());
    assert!(
        matches!(session.recv(false), Some(DebugUpdate::Reply { id: actual, result: Ok(_) }) if actual == id)
    );
    assert!(session.recv(false).is_none());
}

#[test]
fn update_footprints_charge_only_owned_payload_capacity_beyond_the_envelope() {
    let report = FailureReport::fault(
        FailureCode::EngineFailure,
        "diagnostic has a separate allowance",
    );
    assert_eq!(
        update_footprint(&DebugUpdate::Diagnostic { report, dropped: 0 }),
        Ok(size_of::<DebugUpdate>())
    );
}

#[test]
fn diagnostics_coalesce_with_saturating_drop_counts_and_latest_report() {
    let session = session();
    for (message, dropped) in [("first", u64::MAX), ("latest", 1)] {
        session
            .publish(DebugUpdate::Diagnostic {
                report: FailureReport::fault(FailureCode::EngineFailure, message),
                dropped,
            })
            .unwrap();
    }
    match session.recv(false).unwrap() {
        DebugUpdate::Diagnostic { report, dropped } => {
            assert_eq!(dropped, u64::MAX);
            assert_eq!(report.message.unwrap().to_utf8().unwrap(), "latest");
        }
        _ => panic!("one coalesced diagnostic expected"),
    }
    assert!(session.recv(false).is_none());
}

#[test]
fn successful_breakpoint_replacement_reuses_the_selector_budget_and_clear_releases_it() {
    let session = session();
    let request = |breakpoints| DebugRequest::SetBreakpoints {
        selector: ScriptSelector {
            origin: "file:///replace.js".into(),
            expected_digest: None,
        },
        breakpoints,
    };
    for count in [1, 1, 0] {
        let breakpoints = (0..count)
            .map(|client_id| BreakpointSpec {
                client_id,
                line: 0,
                column: None,
                condition: None,
                enabled: true,
                hit_condition: None,
                log_message: None,
            })
            .collect();
        let id = command(&session, request(breakpoints));
        dispatch(&session, id);
        session
            .publish(DebugUpdate::Reply {
                id,
                result: Ok(DebugReply::Breakpoints(vec![])),
            })
            .unwrap();
        assert!(matches!(
            session.recv(false),
            Some(DebugUpdate::Reply {
                result: Ok(DebugReply::Breakpoints(_)),
                ..
            })
        ));
        let state = session.lock();
        assert_eq!(
            state.breakpoint_budgets.iter().flatten().count(),
            usize::from(count != 0)
        );
    }
}
