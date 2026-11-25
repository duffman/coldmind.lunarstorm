//! Deterministic checks of the actual native transport around its OS-wake seam.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
struct TestWake {
    fail: AtomicBool,
    calls: AtomicUsize,
}
impl Wake for TestWake {
    fn wake(&self) -> io::Result<()> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.fail.swap(false, Ordering::SeqCst) {
            Err(io::Error::other("injected wake failure"))
        } else {
            Ok(())
        }
    }
}
fn setup() -> (Transport<usize>, Arc<TestWake>) {
    let wake = Arc::new(TestWake {
        fail: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    });
    let config = SchedulerConfig {
        max_messages: 2,
        max_operations: 2,
        max_controls: 2,
        ..Default::default()
    };
    (Transport::new(7, &config, wake.clone()).unwrap(), wake)
}
#[test]
fn publication_before_arm_and_partial_drain_prevent_sleep() {
    let (transport, wake) = setup();
    transport.handle().try_send(11).unwrap();
    transport.handle().try_send(12).unwrap();
    assert!(!transport.arm());
    let (mut notifications, mut controls) = (Vec::new(), Vec::new());
    transport.import(1, &mut notifications, &mut controls);
    assert!(!transport.arm());
    let Delivery::Message(key) = notifications.pop().unwrap() else {
        panic!("message")
    };
    assert_eq!(transport.message(key), Some(11));
    transport.finish(Delivery::Message(key));
    transport.import(1, &mut notifications, &mut controls);
    let Delivery::Message(key) = notifications.pop().unwrap() else {
        panic!("message")
    };
    assert_eq!(transport.message(key), Some(12));
    transport.finish(Delivery::Message(key));
    assert!(transport.arm());
    assert_eq!(wake.calls.load(Ordering::Relaxed), 0);
}
#[test]
fn publications_after_arm_coalesce_and_poll_return_rearms_cleanly() {
    let (transport, wake) = setup();
    assert!(transport.arm());
    let handle = transport.handle();
    std::thread::spawn(move || {
        handle.try_send(11).unwrap();
        handle.try_send(12).unwrap();
    })
    .join()
    .unwrap();
    assert_eq!(wake.calls.load(Ordering::Relaxed), 1);
    transport.awake();
    assert!(!transport.arm());
}
#[test]
fn ordinary_wake_failure_returns_ownership_and_preserves_armed_gate() {
    let (transport, wake) = setup();
    assert!(transport.arm());
    wake.fail.store(true, Ordering::Relaxed);
    let rejected = transport.handle().try_send(91).unwrap_err();
    assert_eq!(rejected.value, 91);
    assert!(matches!(rejected.reason, RejectionReason::WakeFailed(_)));
    assert_eq!(transport.handle().stats().messages, 0);
    assert_eq!(transport.shared.lock().gate, Gate::Armed);
    transport.handle().try_send(rejected.value).unwrap();
    assert_eq!(wake.calls.load(Ordering::Relaxed), 2);
}
#[test]
fn control_wake_failure_rolls_back_and_wrong_owner_does_not_admit() {
    let (transport, wake) = setup();
    let handle = transport.handle();
    let task = TaskId {
        epoch: 7,
        slot: 0,
        generation: 1,
    };
    assert!(transport.arm());
    wake.fail.store(true, Ordering::Relaxed);
    assert!(matches!(
        handle.request_cancel(task).unwrap_err().reason,
        RejectionReason::WakeFailed(_)
    ));
    let (mut notifications, mut controls) = (Vec::new(), Vec::new());
    transport.import(2, &mut notifications, &mut controls);
    assert!(controls.is_empty());
    assert_eq!(
        handle
            .request_cancel(TaskId { epoch: 8, ..task })
            .unwrap_err()
            .reason,
        RejectionReason::WrongOwner
    );
}
#[test]
fn reserved_wake_failure_keeps_publication_and_latches_fatal_fault() {
    let (transport, wake) = setup();
    let control = transport.control();
    let ticket = control.bind(control.reserve(1024).unwrap()).unwrap();
    let id = ticket.id();
    assert!(transport.arm());
    wake.fail.store(true, Ordering::Relaxed);
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Number(42.0))),
        PublishOutcome::PublishedWakeFailed { .. }
    ));
    assert!(transport.take_fault().is_some());
    let (mut notifications, mut controls) = (Vec::new(), Vec::new());
    transport.import(1, &mut notifications, &mut controls);
    let Delivery::Completion(key) = notifications.pop().unwrap() else {
        panic!("completion")
    };
    let (actual, payload) = transport.completion(key).unwrap();
    assert_eq!(actual, id);
    assert!(matches!(
        payload,
        CompletionPayload::Success(JsData::Number(42.0))
    ));
    assert_eq!(control.cancel(id), CancelOutcome::NotFound);
    transport.finish(Delivery::Completion(key));
    assert_eq!(transport.handle().stats().operations, 0);
}
#[test]
fn oversize_and_abandonment_wake_failures_still_latch_fault() {
    for oversize in [false, true] {
        let (transport, wake) = setup();
        let control = transport.control();
        let ticket = control.bind(control.reserve(1).unwrap()).unwrap();
        assert!(transport.arm());
        wake.fail.store(true, Ordering::Relaxed);
        if oversize {
            assert!(matches!(
                ticket.complete(CompletionPayload::Success(JsData::Null)),
                PublishOutcome::Rejected {
                    reason: PublishRejection::PayloadTooLarge,
                    ..
                }
            ));
        } else {
            drop(ticket);
        }
        assert!(transport.take_fault().is_some());
        assert!(!transport.arm());
    }
}
#[test]
fn stop_wake_failure_closes_admission_even_when_wake_is_broken() {
    let (transport, wake) = setup();
    assert!(transport.arm());
    wake.fail.store(true, Ordering::Relaxed);
    assert!(transport.handle().request_stop().is_err());
    assert!(transport.stopped());
    assert_eq!(
        transport.handle().try_send(1).unwrap_err().reason,
        RejectionReason::Closed
    );
    assert!(!transport.arm());
}
#[test]
fn foreign_bind_rolls_back_the_original_reservation() {
    let (transport, _) = setup();
    let (other, _) = setup();
    let reservation = transport.control().reserve(50).unwrap();
    // Independent instances need independent epochs, as the actual runner supplies.
    other.shared.lock().epoch = 8;
    assert_eq!(
        other.control().bind(reservation).unwrap_err().code,
        FailureCode::WrongOwner
    );
    assert_eq!(transport.handle().stats().reserved_payload_bytes, 0);
}
#[test]
fn canceled_delivery_is_not_redelivered_and_dispatching_is_too_late() {
    let (transport, _) = setup();
    let control = transport.control();
    let ticket = control.bind(control.reserve(1024).unwrap()).unwrap();
    let id = ticket.id();
    assert_eq!(control.cancel(id), CancelOutcome::Canceled);
    assert_eq!(control.cancel(id), CancelOutcome::TooLate);
    drop(ticket);
    let (mut notifications, mut controls) = (Vec::new(), Vec::new());
    transport.import(2, &mut notifications, &mut controls);
    assert_eq!(notifications.len(), 1);
    let Delivery::Completion(key) = notifications.pop().unwrap() else {
        panic!("completion")
    };
    transport.completion(key).unwrap();
    assert_eq!(control.cancel(id), CancelOutcome::Running);
    assert!(transport.completion(key).is_none());
    transport.finish(Delivery::Completion(key));
    assert_eq!(control.cancel(id), CancelOutcome::NotFound);
}

#[test]
fn poison_is_contained_and_closes_all_admission_lanes() {
    let (transport, _) = setup();
    let state = transport.shared.clone();
    assert!(
        std::thread::spawn(move || {
            let _locked = state.state.lock().unwrap();
            panic!("producer panicked while holding internal metadata");
        })
        .join()
        .is_err()
    );
    assert_eq!(
        transport.handle().try_send(42).unwrap_err().reason,
        RejectionReason::Closed
    );
    assert_eq!(transport.take_fault().unwrap().code, FailureCode::HostPanic);
    assert!(!transport.arm());
    transport.close();
}

#[test]
fn closed_and_exhausted_reservations_release_every_byte() {
    let (transport, _) = setup();
    let control = transport.control();
    assert_eq!(control.reserve(0).unwrap_err().code, FailureCode::Capacity);
    assert_eq!(
        control.reserve(usize::MAX).unwrap_err().code,
        FailureCode::Capacity
    );
    let first = control.reserve(30).unwrap();
    let second = control.reserve(40).unwrap();
    assert_eq!(control.reserve(50).unwrap_err().code, FailureCode::Capacity);
    assert_eq!(control.cancel(first.id()), CancelOutcome::NotFound);
    let mut foreign = first.id();
    foreign.context += 1;
    assert_eq!(control.cancel(foreign), CancelOutcome::WrongOwner);
    drop(second);
    transport.close();
    assert_eq!(
        control.reserve(1).unwrap_err().code,
        FailureCode::ClosedContext
    );
    assert_eq!(
        control.bind(first).unwrap_err().code,
        FailureCode::ClosedContext
    );
    assert_eq!(transport.handle().stats().reserved_payload_bytes, 0);
}

#[test]
fn stale_reservation_and_rollback_cannot_consume_a_reused_slot() {
    let (transport, _) = setup();
    let control = transport.control();
    let original = control.reserve(20).unwrap();
    let stale_id = original.id();
    drop(original);
    let live = control.reserve(30).unwrap();
    let stale = OperationReservation {
        id: stale_id,
        rollback: None,
    };
    assert_eq!(
        control.bind(stale).unwrap_err().code,
        FailureCode::InvalidHandle
    );
    transport.shared.rollback(stale_id);
    let mut foreign = stale_id;
    foreign.epoch += 1;
    transport.shared.rollback(foreign);
    assert_eq!(transport.handle().stats().reserved_payload_bytes, 30);
    let ticket = control.bind(live).unwrap();
    transport.shared.rollback(ticket.id());
    assert_eq!(transport.handle().stats().operations, 1);
    drop(ticket);
    transport.close();
}

#[test]
fn invalid_and_already_decided_publishers_return_original_payloads() {
    let (transport, _) = setup();
    let control = transport.control();
    let ticket = control.bind(control.reserve(1024).unwrap()).unwrap();
    let id = ticket.id();
    for mutated in [
        OperationId {
            generation: id.generation + 1,
            ..id
        },
        OperationId {
            epoch: id.epoch + 1,
            ..id
        },
    ] {
        let outcome = Box::new(Publisher {
            shared: transport.shared.clone(),
            id: mutated,
            allowance: 1024,
        })
        .complete(CompletionPayload::Success(JsData::Number(17.0)));
        assert!(matches!(
            outcome,
            PublishOutcome::Rejected {
                reason: PublishRejection::Stale,
                payload: CompletionPayload::Success(JsData::Number(17.0))
            }
        ));
    }
    let duplicate = Box::new(Publisher {
        shared: transport.shared.clone(),
        id,
        allowance: 1024,
    });
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Null)),
        PublishOutcome::Published
    ));
    assert!(matches!(
        duplicate.complete(CompletionPayload::Success(JsData::Bool(true))),
        PublishOutcome::Rejected {
            reason: PublishRejection::Stale,
            payload: CompletionPayload::Success(JsData::Bool(true))
        }
    ));
}

#[test]
fn failure_payload_accounting_includes_nested_backing_storage() {
    let small = CompletionPayload::Failure(OperationFailure {
        code: "Failure".into(),
        message: JsString::from_utf8("failed"),
        details: None,
    });
    assert!(payload_fits(&small, 1024));
    assert!(!payload_fits(&small, 1));
    let with_details = CompletionPayload::Failure(OperationFailure {
        code: "Failure".into(),
        message: JsString::from_utf8("failed"),
        details: Some(JsData::Array(vec![JsData::Null; 64])),
    });
    assert!(payload_fits(&with_details, 16_384));
    assert!(!payload_fits(&with_details, 128));
}

#[test]
fn stop_hook_is_outside_publication_lock_revoked_on_close_and_weakly_owned() {
    let (transport, _) = setup();
    let handle = transport.handle();
    let shared = Arc::downgrade(&transport.shared);
    let called = Arc::new(AtomicUsize::new(0));
    let probe = called.clone();
    handle.install_stop_hook(Arc::new(move || {
        let state = shared.upgrade().unwrap();
        assert!(state.state.try_lock().is_ok());
        probe.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }));
    let weak_wake = handle.wake_callback();
    handle.request_stop().unwrap();
    assert_eq!(called.load(Ordering::Relaxed), 1);
    transport.close();
    handle.request_stop().unwrap();
    assert_eq!(called.load(Ordering::Relaxed), 1);
    drop(handle);
    drop(transport);
    assert!(weak_wake().is_err());
}

#[test]
fn capacity_epoch_exhaustion_faults_without_losing_released_ownership() {
    let (transport, _) = setup();
    transport.handle().try_send(42).unwrap();
    let (mut deliveries, mut controls) = (Vec::new(), Vec::new());
    transport.import(1, &mut deliveries, &mut controls);
    transport.shared.lock().capacity_epoch = u64::MAX;
    transport.finish(deliveries[0]);
    assert_eq!(transport.handle().stats().messages, 0);
    assert_eq!(transport.take_fault().unwrap().class, FailureClass::Host);
    assert!(!transport.arm());
}

#[test]
fn constructor_reports_capacity_overflow_and_fallible_storage_failures() {
    let (_, wake) = setup();
    let overflow = SchedulerConfig {
        max_messages: usize::MAX,
        max_operations: 1,
        ..Default::default()
    };
    assert_eq!(
        Transport::<usize>::new(9, &overflow, wake.clone())
            .err()
            .unwrap()
            .code,
        FailureCode::Capacity
    );
    // controls, message slots/free-list, operation slots/free-list and ready nodes
    // precede the single infallible Arc allocation.
    let config = SchedulerConfig {
        max_messages: 2,
        max_operations: 2,
        max_controls: 2,
        ..Default::default()
    };
    for index in 0..6 {
        let (result, fired) = crate::vm::test_allocator::fail_nth(index, || {
            Transport::<usize>::new(9, &config, wake.clone())
        });
        assert!(fired, "allocation {index} was not exercised");
        assert_eq!(result.err().unwrap().code, FailureCode::Capacity);
    }
    assert!(Transport::<usize>::new(9, &config, wake).is_ok());
}

#[test]
fn closed_and_faulted_sleep_gates_cannot_be_rearmed_by_poll_return() {
    for state in [Gate::Awake, Gate::Armed, Gate::Closed, Gate::Faulted] {
        let mut gate = state;
        gate.awake();
        assert_eq!(
            gate,
            if state == Gate::Armed {
                Gate::Awake
            } else {
                state
            }
        );
        assert!(!gate.arm(true));
        assert_eq!(
            gate.arm(false),
            !matches!(state, Gate::Closed | Gate::Faulted)
        );
    }
}

#[test]
fn poisoned_control_metadata_recovers_and_closed_cancellation_returns_ownership() {
    let (transport, _) = setup();
    let shared = transport.shared.clone();
    assert!(
        std::thread::spawn(move || {
            let _guard = shared.stop_hook.lock().unwrap();
            panic!("control metadata poison");
        })
        .join()
        .is_err()
    );
    let handle = transport.handle();
    handle.install_stop_hook(Arc::new(|| Ok(())));
    handle.request_stop().unwrap();
    let task = TaskId {
        epoch: 7,
        slot: 0,
        generation: 1,
    };
    let rejected = handle.request_cancel(task).unwrap_err();
    assert_eq!(rejected.value, task);
    assert_eq!(rejected.reason, RejectionReason::Closed);
    transport.close();
}

#[test]
fn admission_validation_and_full_lanes_return_original_inputs() {
    let (transport, _) = setup();
    let transport = transport.with_validator(Some(Arc::new(|message| {
        if *message == 0 {
            Err(RejectionReason::Unsupported)
        } else {
            Ok(())
        }
    })));
    let handle = transport.handle();
    let rejected = handle.try_send(0).unwrap_err();
    assert_eq!(
        (rejected.value, rejected.reason),
        (0, RejectionReason::Unsupported)
    );
    handle.try_send(1).unwrap();
    handle.try_send(2).unwrap();
    let rejected = handle.try_send(3).unwrap_err();
    assert_eq!(
        (rejected.value, rejected.reason),
        (3, RejectionReason::Full)
    );
    let id = TaskId {
        epoch: 7,
        slot: 0,
        generation: 1,
    };
    handle.request_cancel(id).unwrap();
    handle.request_cancel(id).unwrap();
    let rejected = handle.request_cancel(id).unwrap_err();
    assert_eq!(rejected.value, id);
    assert_eq!(rejected.reason, RejectionReason::Full);
    let (mut deliveries, mut controls) = (Vec::new(), Vec::new());
    transport.import(8, &mut deliveries, &mut controls);
    assert_eq!(controls, [id, id]);
    assert_eq!(deliveries.len(), 2);
}

#[test]
fn cancellation_or_close_returns_the_unpublished_completion_payload() {
    for close in [false, true] {
        let (transport, _) = setup();
        let control = transport.control();
        let ticket = control.bind(control.reserve(1024).unwrap()).unwrap();
        if close {
            transport.close();
        } else {
            assert_eq!(control.cancel(ticket.id()), CancelOutcome::Canceled);
        }
        assert!(
            matches!(ticket.complete(CompletionPayload::Success(JsData::Number(17.0))),
                PublishOutcome::Rejected { reason, payload: CompletionPayload::Success(JsData::Number(17.0)) }
                if reason == if close { PublishRejection::Closed } else { PublishRejection::Canceled }
            )
        );
        if !close {
            let (mut deliveries, mut controls) = (Vec::new(), Vec::new());
            transport.import(2, &mut deliveries, &mut controls);
            assert_eq!(deliveries.len(), 1);
            let Delivery::Completion(key) = deliveries[0] else {
                panic!("terminal completion");
            };
            assert!(matches!(
                transport.completion(key).unwrap().1,
                CompletionPayload::Failure(_)
            ));
            transport.finish(deliveries[0]);
            assert_eq!(transport.handle().stats().operations, 0);
        }
    }
}

#[test]
fn transport_close_continues_after_message_drop_panic_and_releases_operation_allowances() {
    struct Message {
        panic: bool,
        drops: Arc<AtomicUsize>,
    }
    impl Drop for Message {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
            assert!(!self.panic, "message destructor");
        }
    }
    let wake = Arc::new(TestWake {
        fail: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    });
    let transport = Transport::new(7, &SchedulerConfig::default(), wake).unwrap();
    let drops = Arc::new(AtomicUsize::new(0));
    for panic in [false, true] {
        assert!(
            transport
                .handle()
                .try_send(Message {
                    panic,
                    drops: drops.clone()
                })
                .is_ok()
        );
    }
    let control = transport.control();
    let ticket = control.bind(control.reserve(1024).unwrap()).unwrap();
    transport.close();
    assert_eq!(drops.load(Ordering::Relaxed), 2);
    assert_eq!(transport.handle().stats().messages, 0);
    assert_eq!(transport.handle().stats().operations, 0);
    assert_eq!(transport.handle().stats().reserved_payload_bytes, 0);
    assert_eq!(transport.take_fault().unwrap().code, FailureCode::HostPanic);
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Null)),
        PublishOutcome::Rejected {
            reason: PublishRejection::Closed,
            ..
        }
    ));
    transport.close();
}

/// Counts wakes of a [`CapacityChange`] task.
struct CountingWake(AtomicUsize);
impl std::task::Wake for CountingWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn poll_capacity(future: &mut CapacityChange<usize>, waker: &std::task::Waker) -> bool {
    let mut cx = std::task::Context::from_waker(waker);
    std::pin::Pin::new(future).poll(&mut cx).is_ready()
}

#[test]
fn a_full_producer_is_woken_when_a_delivery_frees_capacity() {
    let (transport, _) = setup();
    let handle = transport.handle();
    handle.try_send(1).unwrap();
    handle.try_send(2).unwrap();
    let seen = handle.stats().capacity_epoch;
    assert!(matches!(
        handle.try_send(3),
        Err(Rejected {
            reason: RejectionReason::Full,
            ..
        })
    ));
    let counter = Arc::new(CountingWake(AtomicUsize::new(0)));
    let waker = std::task::Waker::from(counter.clone());
    let mut change = handle.capacity(seen);
    assert!(
        !poll_capacity(&mut change, &waker),
        "ready before any capacity was released"
    );
    assert!(!poll_capacity(&mut change, &waker));
    let (mut notifications, mut controls) = (Vec::new(), Vec::new());
    transport.import(1, &mut notifications, &mut controls);
    let delivery = notifications.pop().unwrap();
    let Delivery::Message(key) = delivery else {
        panic!("expected a message");
    };
    assert_eq!(transport.message(key), Some(1));
    assert_eq!(counter.0.load(Ordering::Relaxed), 0);
    transport.finish(delivery);
    assert_eq!(
        counter.0.load(Ordering::Relaxed),
        1,
        "one registration, one wake"
    );
    assert!(poll_capacity(&mut change, &waker));
    handle.try_send(3).unwrap();
}

#[test]
fn a_waiting_producer_is_released_when_the_transport_stops_or_closes() {
    for close in [false, true] {
        let (transport, _) = setup();
        let handle = transport.handle();
        let counter = Arc::new(CountingWake(AtomicUsize::new(0)));
        let waker = std::task::Waker::from(counter.clone());
        let mut change = handle.capacity(handle.stats().capacity_epoch);
        assert!(!poll_capacity(&mut change, &waker));
        if close {
            transport.close();
        } else {
            handle.request_stop().unwrap();
        }
        assert_eq!(counter.0.load(Ordering::Relaxed), 1);
        assert!(poll_capacity(&mut change, &waker));
    }
}

#[test]
fn canceling_a_capacity_wait_releases_its_task_without_a_delivery() {
    let (transport, _) = setup();
    let handle = transport.handle();
    let counter = Arc::new(CountingWake(AtomicUsize::new(0)));
    let waker = std::task::Waker::from(counter.clone());
    let mut change = handle.capacity(handle.stats().capacity_epoch);
    assert!(!poll_capacity(&mut change, &waker));

    drop(change);
    drop(waker);
    assert_eq!(Arc::strong_count(&counter), 1);
}

#[test]
fn polling_a_capacity_wait_on_a_new_task_replaces_the_old_waker() {
    let (transport, _) = setup();
    let handle = transport.handle();
    let old = Arc::new(CountingWake(AtomicUsize::new(0)));
    let new = Arc::new(CountingWake(AtomicUsize::new(0)));
    let mut change = handle.capacity(handle.stats().capacity_epoch);
    assert!(!poll_capacity(&mut change, &Waker::from(old.clone())));
    assert!(!poll_capacity(&mut change, &Waker::from(new.clone())));

    handle.request_stop().unwrap();
    assert_eq!(old.0.load(Ordering::Relaxed), 0);
    assert_eq!(new.0.load(Ordering::Relaxed), 1);
}
