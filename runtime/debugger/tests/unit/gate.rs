//! Model tests of the transport's sleep gate: publication, arming and cancellation.

use super::*;
use loom::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use loom::thread;
#[test]
fn model_publication_and_arming_never_leave_hidden_backlog() {
    loom::model(|| {
        let shared = Arc::new(Mutex::new((Gate::Awake, 0usize, false)));
        let wake = Arc::new(AtomicBool::new(false));
        let writer = shared.clone();
        let signaled = wake.clone();
        let producer = thread::spawn(move || {
            let mut state = writer.lock().unwrap();
            state.1 += 1;
            if state.0 == Gate::Armed {
                signaled.store(true, Ordering::Release);
                state.0.awake();
            }
        });
        let owner = shared.clone();
        let consumer = thread::spawn(move || {
            {
                let mut state = owner.lock().unwrap();
                state.1 = state.1.saturating_sub(1);
            }
            thread::yield_now();
            let mut state = owner.lock().unwrap();
            let backlog = state.1 != 0;
            state.2 = state.0.arm(backlog);
        });
        producer.join().unwrap();
        consumer.join().unwrap();
        let state = shared.lock().unwrap();
        if state.2 && state.1 > 0 {
            assert!(wake.load(Ordering::Acquire));
        }
    });
}
#[test]
fn model_cancellation_and_publication_decide_one_terminal_result() {
    loom::model(|| {
        use super::super::OperationState;
        use crate::engine::{CompletionPayload, JsData};
        let decision = Arc::new(Mutex::new(OperationState::Pending));
        let done = decision.clone();
        let canceled = decision.clone();
        let producer = thread::spawn(move || {
            done.lock()
                .unwrap()
                .decide(Some(CompletionPayload::Success(JsData::Number(1.0))))
        });
        let canceler = thread::spawn(move || {
            canceled
                .lock()
                .unwrap()
                .decide(Some(CompletionPayload::Success(JsData::Number(2.0))))
        });
        let a = producer.join().unwrap();
        let b = canceler.join().unwrap();
        assert_ne!(a, b);
        assert!(matches!(
            *decision.lock().unwrap(),
            OperationState::Ready(Some(_))
        ));
    });
}
