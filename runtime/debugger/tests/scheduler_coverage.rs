//! Scheduler behavior through the public loop and host. These are ordinary Rust
//! jobs and checkpoints, without a JavaScript engine or a replacement host.
use coldmind_runtime::{engine::*, scheduler::*};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    num::NonZeroUsize,
    rc::Rc,
    time::Duration,
};

type Body = dyn FnMut(&dyn LocalHost<Job>) -> Result<(), FailureReport>;
struct Job {
    body: Box<Body>,
    drops: Option<Rc<Cell<usize>>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        if let Some(drops) = &self.drops {
            drops.set(drops.get() + 1);
        }
    }
}
fn job(body: impl FnMut(&dyn LocalHost<Job>) -> Result<(), FailureReport> + 'static) -> Job {
    Job {
        body: Box::new(body),
        drops: None,
    }
}
fn tracked(
    drops: &Rc<Cell<usize>>,
    body: impl FnMut(&dyn LocalHost<Job>) -> Result<(), FailureReport> + 'static,
) -> Job {
    Job {
        body: Box::new(body),
        drops: Some(drops.clone()),
    }
}

enum Checkpoint {
    Empty,
    Pending,
    Failure(FailureReport),
    ScriptFailure(FailureReport),
    Panic,
    StopPending,
}
#[derive(Default)]
struct Driver {
    checkpoints: VecDeque<Checkpoint>,
    checkpoint_calls: Rc<Cell<usize>>,
}
impl Driver {
    fn checkpoints(actions: impl IntoIterator<Item = Checkpoint>) -> Self {
        Self {
            checkpoints: actions.into_iter().collect(),
            ..Self::default()
        }
    }
}
impl LoopDriver for Driver {
    type Job = Job;
    type Message = ();
    fn execute_job(
        &mut self,
        job: &mut Job,
        host: &dyn LocalHost<Job>,
    ) -> Result<(), FailureReport> {
        (job.body)(host)
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
        self.checkpoint_calls.set(self.checkpoint_calls.get() + 1);
        match self.checkpoints.pop_front().unwrap_or(Checkpoint::Empty) {
            Checkpoint::Empty => Ok(CheckpointReport::empty()),
            Checkpoint::Pending => Ok(CheckpointReport {
                state: CheckpointState::Pending,
                ..CheckpointReport::empty()
            }),
            Checkpoint::Failure(report) => Err(report),
            Checkpoint::ScriptFailure(report) => Ok(CheckpointReport {
                failures: vec![report],
                ..CheckpointReport::empty()
            }),
            Checkpoint::Panic => panic!("checkpoint panic fixture"),
            Checkpoint::StopPending => {
                host.request_stop();
                Ok(CheckpointReport {
                    state: CheckpointState::Pending,
                    ..CheckpointReport::empty()
                })
            }
        }
    }
}

fn config(capacity: usize) -> SchedulerConfig {
    SchedulerConfig {
        max_jobs: capacity,
        ..Default::default()
    }
}
fn event_loop(driver: Driver) -> EventLoop<Driver> {
    EventLoop::new(driver, config(4), None).unwrap()
}

#[test]
fn every_invalid_scheduler_capacity_is_rejected_before_loop_construction() {
    let defaults = SchedulerConfig::default();
    defaults.validate().unwrap();
    for field in 0..7 {
        let mut invalid = defaults.clone();
        match field {
            0 => invalid.max_jobs = 0,
            1 => invalid.max_messages = 0,
            2 => invalid.max_operations = 0,
            3 => invalid.max_controls = 0,
            4 => invalid.task_batch = 0,
            5 => invalid.reserved_payload_bytes = 0,
            _ => invalid.operation_payload_bytes = 0,
        }
        assert_eq!(invalid.validate().unwrap_err().code, FailureCode::Capacity);
        assert_eq!(
            EventLoop::new(Driver::default(), invalid, None)
                .err()
                .unwrap()
                .code,
            FailureCode::Capacity
        );
    }
    if let Some(too_many) = (u32::MAX as usize).checked_add(1) {
        let invalid = config(too_many);
        assert_eq!(invalid.validate().unwrap_err().code, FailureCode::Capacity);
        assert_eq!(
            EventLoop::new(Driver::default(), invalid, None)
                .err()
                .unwrap()
                .code,
            FailureCode::Capacity
        );
    }
}

#[test]
fn startup_pending_checkpoints_keep_ready_jobs_queued_until_empty() {
    let driver = Driver::checkpoints([
        Checkpoint::Pending,
        Checkpoint::Pending,
        Checkpoint::Empty,
        Checkpoint::Empty,
    ]);
    let checkpoints = driver.checkpoint_calls.clone();
    let mut runtime = event_loop(driver);
    let calls = Rc::new(Cell::new(0));
    let seen = calls.clone();
    runtime
        .host()
        .post(job(move |_| {
            seen.set(seen.get() + 1);
            Ok(())
        }))
        .ok()
        .unwrap();
    for expected in [1, 2] {
        let tick = runtime.tick(false).unwrap();
        assert!(tick.made_progress && !tick.stopped);
        assert_eq!(checkpoints.get(), expected);
        assert_eq!(calls.get(), 0);
        assert_eq!(runtime.stats().live_jobs, 1);
        assert_eq!(runtime.stats().ready_nodes, 1);
    }
    runtime.tick(false).unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(checkpoints.get(), 4);
    assert_eq!(runtime.stats().live_jobs, 0);
    assert!(runtime.last_failure().is_none());
}

#[test]
fn stop_during_pending_startup_prevents_dispatch_and_finishes_capture() {
    let driver = Driver::checkpoints([Checkpoint::StopPending]);
    let checkpoints = driver.checkpoint_calls.clone();
    let mut runtime = event_loop(driver);
    let drops = Rc::new(Cell::new(0));
    let target = runtime
        .host()
        .post(tracked(&drops, |_| {
            panic!("startup stop must prevent dispatch")
        }))
        .ok()
        .unwrap();
    assert_eq!(runtime.run_until(target).unwrap(), TurnOutcome::Stopped);
    assert_eq!(checkpoints.get(), 1);
    assert_eq!(drops.get(), 1);
    assert_eq!(runtime.stats().live_jobs, 0);
    assert_eq!(runtime.stats().ready_nodes, 0);
    let tick = runtime.tick(false).unwrap();
    assert!(tick.stopped && !tick.made_progress);
    runtime.run().unwrap();
    assert_eq!(checkpoints.get(), 1);
    assert_eq!(drops.get(), 1);
}

#[test]
fn run_until_rejects_foreign_tasks_without_closing_either_loop() {
    let mut first = event_loop(Driver::default());
    let mut second = event_loop(Driver::default());
    let first_id = first.host().post(job(|_| Ok(()))).ok().unwrap();
    let second_id = second.host().post(job(|_| Ok(()))).ok().unwrap();
    assert_eq!(
        first.run_until(second_id).unwrap_err().code,
        FailureCode::WrongOwner
    );
    assert!(first.last_failure().is_none());
    assert_eq!(first.stats().live_jobs, 1);
    assert_eq!(first.run_until(first_id).unwrap(), TurnOutcome::Completed);
    assert_eq!(second.run_until(second_id).unwrap(), TurnOutcome::Completed);
}

#[test]
fn already_canceled_capture_returns_without_running_other_ready_jobs() {
    let mut runtime = event_loop(Driver::default());
    let canceled = runtime.host().post(job(|_| Ok(()))).ok().unwrap();
    assert_eq!(runtime.host().cancel(canceled), CancelOutcome::Canceled);
    let calls = Rc::new(Cell::new(0));
    let seen = calls.clone();
    let other = runtime
        .host()
        .post(job(move |_| {
            seen.set(seen.get() + 1);
            Ok(())
        }))
        .ok()
        .unwrap();
    assert_eq!(runtime.run_until(canceled).unwrap(), TurnOutcome::Canceled);
    assert_eq!(calls.get(), 0);
    assert_eq!(runtime.stats().live_jobs, 1);
    assert_eq!(runtime.run_until(other).unwrap(), TurnOutcome::Completed);
    assert_eq!(calls.get(), 1);
}

#[test]
fn stop_before_entry_disposes_ready_and_future_timer_jobs_once() {
    let driver = Driver::default();
    let checkpoints = driver.checkpoint_calls.clone();
    let mut runtime = event_loop(driver);
    let drops = Rc::new(Cell::new(0));
    let target = runtime
        .host()
        .post(tracked(&drops, |_| {
            panic!("stopped loop must not dispatch")
        }))
        .ok()
        .unwrap();
    runtime
        .host()
        .timer(
            Duration::from_secs(3600),
            Some(Duration::from_secs(3600)),
            tracked(&drops, |_| panic!("stopped loop must not run timers")),
        )
        .ok()
        .unwrap();
    runtime.host().request_stop();
    assert_eq!(runtime.run_until(target).unwrap(), TurnOutcome::Stopped);
    let tick = runtime.tick(false).unwrap();
    assert!(tick.stopped && !tick.made_progress);
    assert_eq!(drops.get(), 2);
    assert_eq!(runtime.stats().live_jobs, 0);
    assert_eq!(runtime.stats().ready_nodes, 0);
    assert_eq!(checkpoints.get(), 0);
    runtime.run().unwrap();
    drop(runtime);
    assert_eq!(drops.get(), 2);
}

#[test]
fn run_returns_after_callback_stop_and_disposes_the_remaining_queue() {
    let mut runtime = event_loop(Driver::default());
    let drops = Rc::new(Cell::new(0));
    let calls = Rc::new(Cell::new(0));
    let seen = calls.clone();
    runtime
        .host()
        .post(tracked(&drops, move |host| {
            seen.set(seen.get() + 1);
            host.request_stop();
            Ok(())
        }))
        .ok()
        .unwrap();
    runtime
        .host()
        .post(tracked(&drops, |_| {
            panic!("newer work must not run after stop")
        }))
        .ok()
        .unwrap();
    runtime.run().unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(drops.get(), 2);
    assert_eq!(runtime.stats().live_jobs, 0);
    assert!(runtime.last_failure().is_none());
    drop(runtime);
    assert_eq!(drops.get(), 2);
}

fn unbounded_failure(class: FailureClass) -> FailureReport {
    FailureReport {
        name: None,
        class,
        code: if class == FailureClass::Script {
            FailureCode::ScriptException
        } else {
            FailureCode::EngineFailure
        },
        message: Some(JsString::from_utf8(&"diagnostic".repeat(1024))),
        thrown: Some(JsData::Bytes(Vec::with_capacity(100_000))),
        stack: Vec::new(),
        truncated: false,
    }
}

fn assert_closed_after_failure(
    runtime: &mut EventLoop<Driver>,
    error: &FailureReport,
    drops: &Rc<Cell<usize>>,
) {
    assert_eq!(runtime.last_failure(), Some(error));
    assert_eq!(drops.get(), 2);
    assert_eq!(runtime.stats().live_jobs, 0);
    assert_eq!(runtime.stats().ready_nodes, 0);
    assert!(runtime.tick(false).unwrap().stopped);
    runtime.run().unwrap();
    assert_eq!(runtime.last_failure(), Some(error));
    assert_eq!(drops.get(), 2);
}

#[test]
fn returned_and_panicked_job_failures_are_contained_and_detached() {
    for panic in [false, true] {
        let mut runtime = event_loop(Driver::default());
        let drops = Rc::new(Cell::new(0));
        let target = runtime
            .host()
            .post(tracked(&drops, move |_| {
                if panic {
                    panic!("job panic fixture");
                }
                Err(unbounded_failure(FailureClass::Engine))
            }))
            .ok()
            .unwrap();
        runtime
            .host()
            .post(tracked(&drops, |_| panic!("failure must stop newer work")))
            .ok()
            .unwrap();
        let error = runtime.run_until(target).unwrap_err();
        if panic {
            assert_eq!(
                (error.class, error.code),
                (FailureClass::Host, FailureCode::HostPanic)
            );
        } else {
            assert_eq!(
                (error.class, error.code),
                (FailureClass::Engine, FailureCode::EngineFailure)
            );
            assert!(error.truncated);
            assert_eq!(error.message.as_ref().unwrap().as_units().len(), 4096);
            assert_eq!(error.thrown, None);
        }
        assert_closed_after_failure(&mut runtime, &error, &drops);
        assert_eq!(runtime.run_until(target).unwrap(), TurnOutcome::Stopped);
    }
}

#[test]
fn startup_and_turn_checkpoint_errors_panics_and_script_reports_stop_the_loop() {
    for after_job in [false, true] {
        for mode in 0..3 {
            let action = match mode {
                0 => Checkpoint::Failure(unbounded_failure(FailureClass::Engine)),
                1 => Checkpoint::Panic,
                _ => Checkpoint::ScriptFailure(unbounded_failure(FailureClass::Script)),
            };
            let mut actions = Vec::new();
            if after_job {
                actions.push(Checkpoint::Empty);
            }
            actions.push(action);
            let mut runtime = event_loop(Driver::checkpoints(actions));
            let drops = Rc::new(Cell::new(0));
            let calls = Rc::new(Cell::new(0));
            let seen = calls.clone();
            runtime
                .host()
                .post(tracked(&drops, move |_| {
                    seen.set(seen.get() + 1);
                    Ok(())
                }))
                .ok()
                .unwrap();
            runtime
                .host()
                .post(tracked(&drops, |_| {
                    panic!("checkpoint failure must stop newer work")
                }))
                .ok()
                .unwrap();
            let error = runtime.run().unwrap_err();
            assert_eq!(calls.get(), usize::from(after_job));
            let expected = match mode {
                0 => (FailureClass::Engine, FailureCode::EngineFailure),
                1 => (FailureClass::Host, FailureCode::HostPanic),
                _ => (FailureClass::Script, FailureCode::ScriptException),
            };
            assert_eq!((error.class, error.code), expected);
            if mode != 1 {
                assert!(error.truncated);
                assert_eq!(error.thrown, None);
            }
            assert_closed_after_failure(&mut runtime, &error, &drops);
        }
    }
}

#[test]
fn report_sink_receives_the_bounded_failure_after_jobs_are_disposed() {
    let drops = Rc::new(Cell::new(0));
    let reports = Rc::new(RefCell::new(Vec::new()));
    let seen = reports.clone();
    let disposed = drops.clone();
    let sink: ReportSink = Rc::new(move |report| {
        assert_eq!(disposed.get(), 2);
        seen.borrow_mut().push(report.clone());
    });
    let mut runtime = EventLoop::new(Driver::default(), config(2), Some(sink)).unwrap();
    runtime
        .host()
        .post(tracked(&drops, |_| {
            Err(unbounded_failure(FailureClass::Host))
        }))
        .ok()
        .unwrap();
    runtime
        .host()
        .post(tracked(&drops, |_| panic!("failure must stop newer work")))
        .ok()
        .unwrap();
    let error = runtime.tick(false).unwrap_err();
    assert!(error.truncated);
    assert_eq!(&*reports.borrow(), std::slice::from_ref(&error));
    assert_closed_after_failure(&mut runtime, &error, &drops);
    assert_eq!(reports.borrow().len(), 1);
}

#[test]
fn report_sink_panic_becomes_a_host_failure_and_does_not_escape() {
    let calls = Rc::new(Cell::new(0));
    let seen = calls.clone();
    let sink: ReportSink = Rc::new(move |report| {
        assert_eq!(report.code, FailureCode::EngineFailure);
        seen.set(seen.get() + 1);
        panic!("sink panic fixture");
    });
    let mut runtime = EventLoop::new(Driver::default(), config(2), Some(sink)).unwrap();
    let drops = Rc::new(Cell::new(0));
    runtime
        .host()
        .post(tracked(&drops, |_| {
            Err(unbounded_failure(FailureClass::Engine))
        }))
        .ok()
        .unwrap();
    runtime
        .host()
        .post(tracked(&drops, |_| panic!("failure must stop newer work")))
        .ok()
        .unwrap();
    let error = runtime.tick(false).unwrap_err();
    assert_eq!(
        (error.class, error.code),
        (FailureClass::Host, FailureCode::HostPanic)
    );
    assert_eq!(
        error.message.as_ref().unwrap().to_utf8().unwrap(),
        "report sink panicked"
    );
    assert_closed_after_failure(&mut runtime, &error, &drops);
    assert_eq!(calls.get(), 1);
}

#[test]
fn full_timer_admission_returns_the_original_job_for_later_execution() {
    let mut runtime = EventLoop::new(Driver::default(), config(1), None).unwrap();
    let occupied = runtime.host().post(job(|_| Ok(()))).ok().unwrap();
    let drops = Rc::new(Cell::new(0));
    let calls = Rc::new(Cell::new(0));
    let seen = calls.clone();
    let rejected = runtime
        .host()
        .timer(
            Duration::ZERO,
            None,
            tracked(&drops, move |_| {
                seen.set(seen.get() + 1);
                Ok(())
            }),
        )
        .err()
        .unwrap();
    assert_eq!(rejected.reason, RejectionReason::Full);
    assert_eq!(drops.get(), 0);
    assert_eq!(calls.get(), 0);
    assert!(Rc::ptr_eq(rejected.value.drops.as_ref().unwrap(), &drops));
    assert_eq!(runtime.host().cancel(occupied), CancelOutcome::Canceled);
    let recovered = runtime.host().post(rejected.value).ok().unwrap();
    assert_eq!(
        runtime.run_until(recovered).unwrap(),
        TurnOutcome::Completed
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(drops.get(), 1);
}

#[test]
fn owner_drop_disposes_queued_jobs_and_closed_timer_rejection_preserves_ownership() {
    let runtime = event_loop(Driver::default());
    let host = runtime.host();
    let drops = Rc::new(Cell::new(0));
    host.post(tracked(&drops, |_| {
        panic!("unrun job must only be dropped")
    }))
    .ok()
    .unwrap();
    host.timer(
        Duration::from_secs(3600),
        None,
        tracked(&drops, |_| panic!("unrun timer must only be dropped")),
    )
    .ok()
    .unwrap();
    drop(runtime);
    assert_eq!(drops.get(), 2);
    let rejected_drops = Rc::new(Cell::new(0));
    let rejected = host
        .timer(
            Duration::ZERO,
            Some(Duration::ZERO),
            tracked(&rejected_drops, |_| panic!("closed timer must not run")),
        )
        .err()
        .unwrap();
    assert_eq!(rejected.reason, RejectionReason::Closed);
    assert_eq!(rejected_drops.get(), 0);
    assert!(Rc::ptr_eq(
        rejected.value.drops.as_ref().unwrap(),
        &rejected_drops
    ));
    drop(rejected);
    assert_eq!(rejected_drops.get(), 1);
    drop(host);
    assert_eq!(drops.get(), 2);
}
