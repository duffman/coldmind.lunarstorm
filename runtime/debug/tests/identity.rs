use coldmind_runtime::{engine::*, scheduler::*};
use static_assertions::assert_not_impl_any;
use std::{
    cell::{Cell, RefCell},
    num::NonZeroUsize,
    rc::Rc,
};

type Body = dyn FnMut(&dyn LocalHost<Job>);
struct Job(Box<Body>);
struct Driver;
impl LoopDriver for Driver {
    type Job = Job;
    type Message = ();
    fn execute_job(
        &mut self,
        job: &mut Job,
        host: &dyn LocalHost<Job>,
    ) -> Result<(), FailureReport> {
        (job.0)(host);
        Ok(())
    }
    fn deliver_message(&mut self, _: (), _: &dyn LocalHost<Job>) -> Result<(), FailureReport> {
        Ok(())
    }
    fn deliver_completion(
        &mut self,
        _: OperationId,
        _: CompletionPayload,
        _: &dyn LocalHost<Job>,
    ) -> Result<(), FailureReport> {
        Ok(())
    }
    fn checkpoint(
        &mut self,
        _: NonZeroUsize,
        _: &dyn LocalHost<Job>,
    ) -> Result<CheckpointReport, FailureReport> {
        Ok(CheckpointReport::empty())
    }
}
assert_not_impl_any!(EventLoop<Driver>: Send, Sync);
fn job(f: impl FnMut(&dyn LocalHost<Job>) + 'static) -> Job {
    Job(Box::new(f))
}
fn config(capacity: usize) -> SchedulerConfig {
    SchedulerConfig {
        max_jobs: capacity,
        ..Default::default()
    }
}

#[test]
fn fifo_posting_during_callback_uses_short_borrows() {
    let mut event_loop = EventLoop::new(Driver, config(3), None).unwrap();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let a = seen.clone();
    event_loop
        .host()
        .post(job(move |host| {
            a.borrow_mut().push(1);
            let b = a.clone();
            assert!(host.post(job(move |_| b.borrow_mut().push(3))).is_ok());
        }))
        .ok()
        .unwrap();
    let a = seen.clone();
    event_loop
        .host()
        .post(job(move |_| a.borrow_mut().push(2)))
        .ok()
        .unwrap();
    assert!(event_loop.tick(false).unwrap().made_progress);
    assert_eq!(*seen.borrow(), [1, 2, 3]);
    assert_eq!(event_loop.stats().live_jobs, 0);
}
#[test]
fn full_admission_preserves_job_without_running_or_stopping() {
    let mut event_loop = EventLoop::new(Driver, config(1), None).unwrap();
    let called = Rc::new(Cell::new(0));
    let a = called.clone();
    event_loop
        .host()
        .post(job(move |_| a.set(a.get() + 1)))
        .ok()
        .unwrap();
    let a = called.clone();
    let rejected = event_loop
        .host()
        .post(job(move |_| a.set(a.get() + 10)))
        .err()
        .unwrap();
    assert_eq!(rejected.reason, RejectionReason::Full);
    assert_eq!(called.get(), 0);
    event_loop.tick(false).unwrap();
    assert_eq!(called.get(), 1);
    event_loop.host().post(rejected.value).ok().unwrap();
    event_loop.tick(false).unwrap();
    assert_eq!(called.get(), 11);
}
#[test]
fn cancellation_unlinks_ready_nodes_and_reuse_rejects_stale_ids() {
    let mut event_loop = EventLoop::new(Driver, config(2), None).unwrap();
    let first = event_loop.host().post(job(|_| {})).ok().unwrap();
    assert_eq!(event_loop.host().cancel(first), CancelOutcome::Canceled);
    for _ in 0..100_000 {
        let id = event_loop.host().post(job(|_| {})).ok().unwrap();
        assert_eq!(event_loop.host().cancel(first), CancelOutcome::NotFound);
        assert_eq!(event_loop.host().cancel(id), CancelOutcome::Canceled);
    }
    assert_eq!(event_loop.stats().ready_nodes, 0);
    assert_eq!(event_loop.stats().allocated_slots, 2);
    assert!(!event_loop.tick(false).unwrap().made_progress);
}
#[test]
fn running_one_shot_is_too_late_and_wrong_loop_is_explicit() {
    let mut event_loop = EventLoop::new(Driver, config(1), None).unwrap();
    let id = Rc::new(Cell::new(None));
    let inside = id.clone();
    id.set(Some(
        event_loop
            .host()
            .post(job(move |host| {
                assert_eq!(host.cancel(inside.get().unwrap()), CancelOutcome::TooLate);
            }))
            .ok()
            .unwrap(),
    ));
    let other = EventLoop::new(Driver, config(1), None).unwrap();
    assert_eq!(
        other.host().cancel(id.get().unwrap()),
        CancelOutcome::WrongOwner
    );
    event_loop.tick(false).unwrap();
    assert_eq!(
        event_loop.host().cancel(id.get().unwrap()),
        CancelOutcome::NotFound
    );
}

#[test]
fn until_turn_finishes_target_and_leaves_newer_fifo_work_queued() {
    let mut event_loop = EventLoop::new(Driver, config(4), None).unwrap();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let a = seen.clone();
    event_loop
        .host()
        .post(job(move |_| a.borrow_mut().push(1)))
        .ok()
        .unwrap();
    let a = seen.clone();
    let target = event_loop
        .host()
        .post(job(move |_| a.borrow_mut().push(2)))
        .ok()
        .unwrap();
    let a = seen.clone();
    event_loop
        .host()
        .post(job(move |_| a.borrow_mut().push(3)))
        .ok()
        .unwrap();
    assert_eq!(
        event_loop.run_until(target).unwrap(),
        TurnOutcome::Completed
    );
    assert_eq!(*seen.borrow(), [1, 2]);
    event_loop.tick(false).unwrap();
    assert_eq!(*seen.borrow(), [1, 2, 3]);
}
#[test]
fn canceling_a_capture_does_not_dispatch_newer_work() {
    let mut event_loop = EventLoop::new(Driver, config(3), None).unwrap();
    let target = Rc::new(Cell::new(None));
    let inside = target.clone();
    event_loop
        .host()
        .post(job(move |host| {
            assert_eq!(host.cancel(inside.get().unwrap()), CancelOutcome::Canceled);
        }))
        .ok()
        .unwrap();
    target.set(Some(
        event_loop
            .host()
            .post(job(|_| panic!("canceled target")))
            .ok()
            .unwrap(),
    ));
    let called = Rc::new(Cell::new(false));
    let inside = called.clone();
    event_loop
        .host()
        .post(job(move |_| inside.set(true)))
        .ok()
        .unwrap();
    assert_eq!(
        event_loop.run_until(target.get().unwrap()).unwrap(),
        TurnOutcome::Canceled
    );
    assert!(!called.get());
}

struct Checkpoints {
    count: Rc<Cell<usize>>,
    partial: bool,
}
impl LoopDriver for Checkpoints {
    type Job = Job;
    type Message = ();
    fn execute_job(
        &mut self,
        job: &mut Job,
        host: &dyn LocalHost<Job>,
    ) -> Result<(), FailureReport> {
        (job.0)(host);
        Ok(())
    }
    fn deliver_message(&mut self, _: (), _: &dyn LocalHost<Job>) -> Result<(), FailureReport> {
        Ok(())
    }
    fn deliver_completion(
        &mut self,
        _: OperationId,
        _: CompletionPayload,
        _: &dyn LocalHost<Job>,
    ) -> Result<(), FailureReport> {
        Ok(())
    }
    fn checkpoint(
        &mut self,
        _: NonZeroUsize,
        _: &dyn LocalHost<Job>,
    ) -> Result<CheckpointReport, FailureReport> {
        self.count.set(self.count.get() + 1);
        Ok(CheckpointReport {
            state: if self.partial && self.count.get() == 2 {
                CheckpointState::Pending
            } else {
                CheckpointState::Empty
            },
            failures: vec![],
            suppressed_failures: 0,
        })
    }
}
#[test]
fn canceled_capture_finishes_older_turn_checkpoint_including_continuations() {
    for partial in [false, true] {
        let count = Rc::new(Cell::new(0));
        let mut runtime = EventLoop::new(
            Checkpoints {
                count: count.clone(),
                partial,
            },
            config(3),
            None,
        )
        .unwrap();
        let target = Rc::new(Cell::new(None));
        let inside = target.clone();
        runtime
            .host()
            .post(job(move |host| {
                host.cancel(inside.get().unwrap());
            }))
            .ok()
            .unwrap();
        target.set(Some(
            runtime
                .host()
                .post(job(|_| panic!("canceled")))
                .ok()
                .unwrap(),
        ));
        assert_eq!(
            runtime.run_until(target.get().unwrap()).unwrap(),
            TurnOutcome::Canceled
        );
        assert_eq!(
            count.get(),
            if partial { 3 } else { 2 },
            "older turn completes before canceled capture returns"
        );
        assert_eq!(runtime.stats().live_jobs, 0);
    }
}
#[test]
fn stop_in_last_callback_does_not_execute_another_checkpoint() {
    for batch in [1, 2, 64] {
        let count = Rc::new(Cell::new(0));
        let mut cfg = config(1);
        cfg.task_batch = batch;
        let mut runtime = EventLoop::new(
            Checkpoints {
                count: count.clone(),
                partial: false,
            },
            cfg,
            None,
        )
        .unwrap();
        runtime
            .host()
            .post(job(|host| host.request_stop()))
            .ok()
            .unwrap();
        assert!(runtime.tick(false).unwrap().stopped);
        assert_eq!(count.get(), 1, "only startup checkpoint runs after stop");
        assert_eq!(runtime.stats().live_jobs, 0);
    }
}

struct CheckpointControl {
    count: usize,
    target: Rc<Cell<Option<TaskId>>>,
    stop: bool,
    partial: bool,
}
impl LoopDriver for CheckpointControl {
    type Job = Job;
    type Message = ();
    fn execute_job(
        &mut self,
        job: &mut Job,
        host: &dyn LocalHost<Job>,
    ) -> Result<(), FailureReport> {
        (job.0)(host);
        Ok(())
    }
    fn deliver_message(&mut self, _: (), _: &dyn LocalHost<Job>) -> Result<(), FailureReport> {
        Ok(())
    }
    fn deliver_completion(
        &mut self,
        _: OperationId,
        _: CompletionPayload,
        _: &dyn LocalHost<Job>,
    ) -> Result<(), FailureReport> {
        Ok(())
    }
    fn checkpoint(
        &mut self,
        _: NonZeroUsize,
        host: &dyn LocalHost<Job>,
    ) -> Result<CheckpointReport, FailureReport> {
        self.count += 1;
        if self.count == 2 {
            if self.stop {
                host.request_stop();
            } else {
                assert_eq!(
                    host.cancel(self.target.get().unwrap()),
                    CancelOutcome::Canceled
                );
            }
        }
        Ok(CheckpointReport {
            state: if self.count == 2 && self.partial {
                CheckpointState::Pending
            } else {
                CheckpointState::Empty
            },
            failures: vec![],
            suppressed_failures: 0,
        })
    }
}
#[test]
fn checkpoint_controls_are_observed_before_new_dispatch() {
    for partial in [false, true] {
        for stop in [false, true] {
            let target = Rc::new(Cell::new(None));
            let mut runtime = EventLoop::new(
                CheckpointControl {
                    count: 0,
                    target: target.clone(),
                    stop,
                    partial,
                },
                config(3),
                None,
            )
            .unwrap();
            runtime.host().post(job(|_| {})).ok().unwrap();
            target.set(Some(
                runtime
                    .host()
                    .post(job(|_| panic!("target canceled or stopped")))
                    .ok()
                    .unwrap(),
            ));
            runtime
                .host()
                .post(job(|_| panic!("newer task must stay queued")))
                .ok()
                .unwrap();
            assert_eq!(
                runtime.run_until(target.get().unwrap()).unwrap(),
                if stop {
                    TurnOutcome::Stopped
                } else {
                    TurnOutcome::Canceled
                }
            );
            assert_eq!(runtime.stats().live_jobs, if stop { 0 } else { 1 });
        }
    }
}
struct AfterTurn(Rc<dyn Fn()>);
impl Drop for AfterTurn {
    fn drop(&mut self) {
        (self.0)();
    }
}
#[test]
fn job_destructor_controls_are_observed_before_new_dispatch() {
    for stop in [false, true] {
        let mut runtime = EventLoop::new(Driver, config(3), None).unwrap();
        let host = runtime.host();
        let target = Rc::new(Cell::new(None));
        let inside = target.clone();
        let cleanup = AfterTurn(Rc::new(move || {
            if stop {
                host.request_stop();
            } else {
                host.cancel(inside.get().unwrap());
            }
        }));
        runtime
            .host()
            .post(job(move |_| {
                let _ = &cleanup;
            }))
            .ok()
            .unwrap();
        target.set(Some(
            runtime
                .host()
                .post(job(|_| panic!("target canceled or stopped")))
                .ok()
                .unwrap(),
        ));
        runtime
            .host()
            .post(job(|_| panic!("newer task must stay queued")))
            .ok()
            .unwrap();
        assert_eq!(
            runtime.run_until(target.get().unwrap()).unwrap(),
            if stop {
                TurnOutcome::Stopped
            } else {
                TurnOutcome::Canceled
            }
        );
        assert_eq!(runtime.stats().live_jobs, if stop { 0 } else { 1 });
    }
}
