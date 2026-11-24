//! Phase 4: actual bounded publication, independent operation capacity and wakeups.
use coldmind_runtime::{engine::*, scheduler::*};
use std::{
    cell::RefCell,
    num::NonZeroUsize,
    rc::Rc,
    sync::{Arc, Barrier},
    thread,
    time::Duration,
};
struct Driver {
    messages: Rc<RefCell<Vec<usize>>>,
    results: Rc<RefCell<Vec<(OperationId, CompletionPayload)>>>,
}
impl LoopDriver for Driver {
    type Job = ();
    type Message = usize;
    fn execute_job(&mut self, _: &mut (), _: &dyn LocalHost<()>) -> Result<(), FailureReport> {
        Ok(())
    }
    fn deliver_message(
        &mut self,
        value: usize,
        _: &dyn LocalHost<()>,
    ) -> Result<(), FailureReport> {
        self.messages.borrow_mut().push(value);
        Ok(())
    }
    fn deliver_completion(
        &mut self,
        id: OperationId,
        value: CompletionPayload,
        _: &dyn LocalHost<()>,
    ) -> Result<(), FailureReport> {
        self.results.borrow_mut().push((id, value));
        Ok(())
    }
    fn checkpoint(
        &mut self,
        _: NonZeroUsize,
        _: &dyn LocalHost<()>,
    ) -> Result<CheckpointReport, FailureReport> {
        Ok(CheckpointReport::empty())
    }
}
type Results = Rc<RefCell<Vec<(OperationId, CompletionPayload)>>>;
fn setup() -> (EventLoop<Driver>, Rc<RefCell<Vec<usize>>>, Results) {
    let messages = Rc::new(RefCell::new(Vec::new()));
    let results = Rc::new(RefCell::new(Vec::new()));
    let cfg = SchedulerConfig {
        max_jobs: 1,
        max_messages: 1,
        max_operations: 1,
        max_controls: 1,
        ..Default::default()
    };
    (
        EventLoop::new(
            Driver {
                messages: messages.clone(),
                results: results.clone(),
            },
            cfg,
            None,
        )
        .unwrap(),
        messages,
        results,
    )
}
fn failure_code(payload: &CompletionPayload) -> &str {
    match payload {
        CompletionPayload::Failure(error) => &error.code,
        _ => panic!("expected failure"),
    }
}
#[test]
fn timers_cannot_exhaust_completion_capacity() {
    let (mut runtime, _, results) = setup();
    runtime
        .host()
        .timer(Duration::from_secs(3600), None, ())
        .unwrap();
    let reservation = runtime.host().reserve_operation(1024).unwrap();
    let id = reservation.id();
    let ticket = runtime.host().bind_operation(reservation).unwrap();
    assert_eq!(ticket.id(), id);
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Number(42.0))),
        PublishOutcome::Published
    ));
    runtime.tick(false).unwrap();
    assert_eq!(results.borrow().len(), 1);
    assert_eq!(results.borrow()[0].0, id);
}
#[test]
fn full_messages_do_not_block_reserved_completion() {
    let (mut runtime, messages, results) = setup();
    let handle = runtime.handle();
    handle.try_send(17).unwrap();
    assert_eq!(handle.try_send(18).unwrap_err().value, 18);
    let ticket = runtime
        .host()
        .bind_operation(runtime.host().reserve_operation(1024).unwrap())
        .unwrap();
    thread::spawn(move || ticket.complete(CompletionPayload::Success(JsData::Null)))
        .join()
        .unwrap();
    runtime.tick(false).unwrap();
    assert_eq!(*messages.borrow(), [17]);
    assert_eq!(results.borrow().len(), 1);
    handle.try_send(19).unwrap();
    runtime.tick(false).unwrap();
    assert_eq!(*messages.borrow(), [17, 19]);
}

#[test]
fn async_cross_thread_send_waits_for_the_owner_or_returns_the_message_on_stop() {
    use std::{
        future::Future,
        pin::pin,
        sync::mpsc,
        task::{Context, Poll, Wake, Waker},
    };

    struct Signal(mpsc::Sender<()>);
    impl Wake for Signal {
        fn wake(self: Arc<Self>) {
            let _ = self.0.send(());
        }
    }

    for stop in [false, true] {
        let (mut runtime, messages, results) = setup();
        let handle = runtime.handle();
        handle.try_send(17).unwrap();
        let ticket = runtime
            .host()
            .bind_operation(runtime.host().reserve_operation(1024).unwrap())
            .unwrap();
        let (waiting, wait) = mpsc::channel();
        #[allow(
            clippy::result_large_err,
            reason = "the thread returns the transport's own rejection, which carries its report"
        )]
        let producer = thread::spawn(move || {
            let (wake, woke) = mpsc::channel();
            let waker = Waker::from(Arc::new(Signal(wake)));
            let mut cx = Context::from_waker(&waker);
            let mut send = pin!(handle.send(18));
            assert!(send.as_mut().poll(&mut cx).is_pending());
            // A one-shot operation may still publish while the stream inbox is full.
            assert!(matches!(
                ticket.complete(CompletionPayload::Success(JsData::Null)),
                PublishOutcome::Published
            ));
            waiting.send(()).unwrap();
            assert!(woke.recv_timeout(Duration::from_secs(5)).is_ok());
            let Poll::Ready(result) = send.as_mut().poll(&mut cx) else {
                panic!("send still waiting after release or stop");
            };
            result
        });
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        if stop {
            runtime.handle().request_stop().unwrap();
            let rejected = producer.join().unwrap().unwrap_err();
            assert_eq!(rejected.value, 18);
            assert!(matches!(rejected.reason, RejectionReason::Closed));
            assert!(runtime.tick(false).unwrap().stopped);
            assert!(messages.borrow().is_empty());
        } else {
            runtime.tick(false).unwrap();
            producer.join().unwrap().unwrap();
            runtime.tick(false).unwrap();
            assert_eq!(*messages.borrow(), [17, 18]);
            assert_eq!(results.borrow().len(), 1);
        }
    }
}
#[test]
fn unbound_reservation_rolls_back_without_abandonment() {
    let (mut runtime, _, results) = setup();
    let reservation = runtime.host().reserve_operation(1024).unwrap();
    assert!(format!("{reservation:?}").contains("OperationReservation"));
    assert!(runtime.host().reserve_operation(1024).is_err());
    drop(reservation);
    let another = runtime.host().reserve_operation(1024).unwrap();
    drop(another);
    runtime.tick(false).unwrap();
    assert!(results.borrow().is_empty());
}
#[test]
fn abandoned_ticket_delivers_failure() {
    let (mut runtime, _, results) = setup();
    let ticket = runtime
        .host()
        .bind_operation(runtime.host().reserve_operation(1).unwrap())
        .unwrap();
    assert!(format!("{ticket:?}").contains("CompletionTicket"));
    drop(ticket);
    runtime.tick(false).unwrap();
    assert_eq!(failure_code(&results.borrow()[0].1), "Abandoned");
}
#[test]
fn oversize_result_does_not_orphan_operation() {
    let (mut runtime, _, results) = setup();
    let ticket = runtime
        .host()
        .bind_operation(runtime.host().reserve_operation(1).unwrap())
        .unwrap();
    let payload = CompletionPayload::Success(JsData::Bytes(vec![5; 100]));
    assert!(matches!(
        ticket.complete(payload),
        PublishOutcome::Rejected {
            reason: PublishRejection::PayloadTooLarge,
            payload: CompletionPayload::Success(_)
        }
    ));
    runtime.tick(false).unwrap();
    assert_eq!(failure_code(&results.borrow()[0].1), "PayloadTooLarge");
}
#[test]
fn completion_and_cancel_have_one_winner() {
    for _ in 0..100 {
        let (mut runtime, _, results) = setup();
        let ticket = runtime
            .host()
            .bind_operation(runtime.host().reserve_operation(1024).unwrap())
            .unwrap();
        let id = ticket.id();
        let barrier = Arc::new(Barrier::new(2));
        let other = barrier.clone();
        let producer = thread::spawn(move || {
            other.wait();
            ticket.complete(CompletionPayload::Success(JsData::Null))
        });
        barrier.wait();
        let canceled = runtime.host().cancel_operation(id);
        let published = producer.join().unwrap();
        assert!(matches!(
            (canceled, published),
            (
                CancelOutcome::Canceled,
                PublishOutcome::Rejected {
                    reason: PublishRejection::Canceled,
                    ..
                }
            ) | (CancelOutcome::TooLate, PublishOutcome::Published)
        ));
        runtime.tick(false).unwrap();
        assert_eq!(results.borrow().len(), 1);
    }
}
#[test]
fn prior_session_ticket_is_stale_and_keeps_owned_payload() {
    let (runtime, _, _) = setup();
    let ticket = runtime
        .host()
        .bind_operation(runtime.host().reserve_operation(1024).unwrap())
        .unwrap();
    let old_id = ticket.id();
    let handle = runtime.handle();
    drop(runtime);
    let (other, _, _) = setup();
    assert_eq!(
        handle.try_send(13).unwrap_err().reason,
        RejectionReason::Closed
    );
    assert_eq!(
        other.host().cancel_operation(old_id),
        CancelOutcome::WrongOwner
    );
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Null)),
        PublishOutcome::Rejected {
            reason: PublishRejection::Closed,
            payload: CompletionPayload::Success(JsData::Null)
        }
    ));
}
#[test]
fn stop_bypasses_full_message_operation_and_control_lanes() {
    let (mut runtime, _, results) = setup();
    let handle = runtime.handle();
    handle.try_send(1).unwrap();
    let task = runtime.host().post(()).unwrap();
    handle.request_cancel(task).unwrap();
    assert_eq!(
        handle.request_cancel(task).unwrap_err().reason,
        RejectionReason::Full
    );
    let ticket = runtime
        .host()
        .bind_operation(runtime.host().reserve_operation(1024).unwrap())
        .unwrap();
    handle.request_stop().unwrap();
    assert!(runtime.tick(false).unwrap().stopped);
    assert!(results.borrow().is_empty());
    assert_eq!(
        runtime.stats().ready_nodes,
        0,
        "stop unlinks imported notifications"
    );
    drop(ticket);
}

#[test]
fn producers_deliver_one_hundred_thousand_accepted_messages_exactly_once() {
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let owner = thread::spawn(move || {
        let messages = Rc::new(RefCell::new(Vec::new()));
        let results = Rc::new(RefCell::new(Vec::new()));
        let mut runtime = EventLoop::new(
            Driver {
                messages: messages.clone(),
                results,
            },
            SchedulerConfig {
                max_messages: 128,
                ..Default::default()
            },
            None,
        )
        .unwrap();
        let mut producers = Vec::new();
        for producer in 0..4 {
            let handle = runtime.handle();
            producers.push(thread::spawn(move || {
                for n in 0..25_000 {
                    let mut value = producer * 25_000 + n;
                    loop {
                        match handle.try_send(value) {
                            Ok(()) => break,
                            Err(rejected) => {
                                assert_eq!(rejected.reason, RejectionReason::Full);
                                value = rejected.value;
                                thread::yield_now();
                            }
                        }
                    }
                }
            }));
        }
        while messages.borrow().len() < 100_000 {
            runtime.tick(true).unwrap();
        }
        for producer in producers {
            producer.join().unwrap();
        }
        let mut values = std::mem::take(&mut *messages.borrow_mut());
        values.sort_unstable();
        assert_eq!(values, (0..100_000).collect::<Vec<_>>());
        let stats = runtime.handle().stats();
        assert_eq!(stats.messages, 0);
        send.send(stats).unwrap();
    });
    let stats = receive
        .recv_timeout(Duration::from_secs(10))
        .expect("producer stress stalled");
    owner.join().unwrap();
    assert!(stats.wake_calls <= 100_000);
}
#[test]
fn armed_owner_observes_cross_thread_publication_without_idle_spinning() {
    let (mut runtime, messages, _) = setup();
    let handle = runtime.handle();
    let other = handle.clone();
    let producer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        other.try_send(42).unwrap();
    });
    runtime.tick(true).unwrap();
    runtime.tick(false).unwrap();
    producer.join().unwrap();
    assert_eq!(*messages.borrow(), [42]);
    handle.request_stop().unwrap();
}
#[test]
fn partial_batches_do_not_hide_transport_backlog_before_sleep() {
    let messages = Rc::new(RefCell::new(Vec::new()));
    let results = Rc::new(RefCell::new(Vec::new()));
    let mut runtime = EventLoop::new(
        Driver {
            messages: messages.clone(),
            results,
        },
        SchedulerConfig {
            max_messages: 100,
            task_batch: 1,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    for i in 0..100 {
        runtime.handle().try_send(i).unwrap();
    }
    for _ in 0..100 {
        runtime.tick(true).unwrap();
    }
    assert_eq!(*messages.borrow(), (0..100).collect::<Vec<_>>());
}
