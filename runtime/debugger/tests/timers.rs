//! Timer scheduling on the owner loop: equal deadlines, intervals that skip missed
//! periods, cancellation after a timer is ready, invalid periods and overflow.

use coldmind_runtime::{engine::*, scheduler::*};
use std::{
    cell::{Cell, RefCell},
    num::NonZeroUsize,
    rc::Rc,
    time::{Duration, Instant},
};
struct ClockNow(Cell<Instant>);
impl Clock for ClockNow {
    fn now(&self) -> Instant {
        self.0.get()
    }
}
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
fn job(f: impl FnMut(&dyn LocalHost<Job>) + 'static) -> Job {
    Job(Box::new(f))
}
fn setup(cap: usize) -> (EventLoop<Driver>, Rc<ClockNow>) {
    let clock = Rc::new(ClockNow(Cell::new(Instant::now())));
    (
        EventLoop::with_clock(
            Driver,
            SchedulerConfig {
                max_jobs: cap,
                ..Default::default()
            },
            None,
            clock.clone(),
        )
        .unwrap(),
        clock,
    )
}
#[test]
fn deadlines_and_equal_deadline_insertion_order() {
    let (mut runtime, clock) = setup(3);
    let start = clock.now();
    let seen = Rc::new(RefCell::new(Vec::new()));
    for (delay, n) in [(20, 3), (10, 1), (10, 2)] {
        let seen = seen.clone();
        runtime
            .host()
            .timer(
                Duration::from_millis(delay),
                None,
                job(move |_| seen.borrow_mut().push(n)),
            )
            .ok()
            .unwrap();
    }
    runtime.tick(false).unwrap();
    assert!(seen.borrow().is_empty());
    clock.0.set(start + Duration::from_millis(10));
    runtime.tick(false).unwrap();
    assert_eq!(*seen.borrow(), [1, 2]);
    clock.0.set(start + Duration::from_millis(20));
    runtime.tick(false).unwrap();
    assert_eq!(*seen.borrow(), [1, 2, 3]);
    assert_eq!(runtime.stats().live_jobs, 0);
}
#[test]
fn interval_skips_missed_periods_and_self_cancel_suppresses_rearming() {
    let (mut runtime, clock) = setup(1);
    let start = clock.now();
    let id = Rc::new(Cell::new(None));
    let inside = id.clone();
    let calls = Rc::new(Cell::new(0));
    let seen = calls.clone();
    id.set(Some(
        runtime
            .host()
            .timer(
                Duration::from_millis(10),
                Some(Duration::from_millis(10)),
                job(move |host| {
                    seen.set(seen.get() + 1);
                    if seen.get() == 2 {
                        assert_eq!(host.cancel(inside.get().unwrap()), CancelOutcome::Running);
                    }
                }),
            )
            .ok()
            .unwrap(),
    ));
    clock.0.set(start + Duration::from_millis(99));
    runtime.tick(false).unwrap();
    assert_eq!(calls.get(), 1);
    runtime.tick(false).unwrap();
    assert_eq!(calls.get(), 1);
    clock.0.set(start + Duration::from_millis(100));
    runtime.tick(false).unwrap();
    assert_eq!(calls.get(), 2);
    clock.0.set(start + Duration::from_secs(1));
    runtime.tick(false).unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(runtime.stats().live_jobs, 0);
}
#[test]
fn cancel_after_timer_becomes_ready_unlinks_it() {
    let (mut runtime, _) = setup(2);
    let timer = Rc::new(Cell::new(None));
    let inside = timer.clone();
    runtime
        .host()
        .post(job(move |host| {
            assert_eq!(host.cancel(inside.get().unwrap()), CancelOutcome::Canceled);
        }))
        .ok()
        .unwrap();
    timer.set(Some(
        runtime
            .host()
            .timer(Duration::ZERO, None, job(|_| panic!("canceled timer")))
            .ok()
            .unwrap(),
    ));
    runtime.tick(false).unwrap();
    assert_eq!(runtime.stats().ready_nodes, 0);
    assert_eq!(runtime.stats().live_jobs, 0);
}
#[test]
fn invalid_period_and_overflow_preserve_job_ownership() {
    let (runtime, _) = setup(1);
    let zero = runtime
        .host()
        .timer(Duration::ZERO, Some(Duration::ZERO), job(|_| {}))
        .err()
        .unwrap();
    assert_eq!(zero.reason, RejectionReason::InvalidPeriod);
    let overflow = runtime
        .host()
        .timer(Duration::MAX, None, zero.value)
        .err()
        .unwrap();
    assert_eq!(overflow.reason, RejectionReason::DeadlineOverflow);
    assert_eq!(runtime.stats().live_jobs, 0);
}

struct Dropped(Rc<Cell<usize>>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
#[test]
fn one_shot_and_interval_jobs_are_disposed_once_and_hosts_close_with_owner() {
    let (mut runtime, clock) = setup(2);
    let drops = Rc::new(Cell::new(0));
    let guard = Dropped(drops.clone());
    runtime
        .host()
        .timer(
            Duration::ZERO,
            None,
            job(move |_| {
                let _ = &guard;
            }),
        )
        .ok()
        .unwrap();
    let guard = Dropped(drops.clone());
    let interval = runtime
        .host()
        .timer(
            Duration::ZERO,
            Some(Duration::from_millis(5)),
            job(move |_| {
                let _ = &guard;
            }),
        )
        .ok()
        .unwrap();
    runtime.tick(false).unwrap();
    assert_eq!(drops.get(), 1);
    assert_eq!(runtime.stats().live_jobs, 1);
    clock.0.set(clock.now() + Duration::from_secs(1));
    runtime.tick(false).unwrap();
    assert_eq!(drops.get(), 1);
    assert_eq!(runtime.host().cancel(interval), CancelOutcome::Canceled);
    assert_eq!(drops.get(), 2);
    let host = runtime.host();
    drop(runtime);
    assert_eq!(
        host.post(job(|_| {})).err().unwrap().reason,
        RejectionReason::Closed
    );
}

struct PartialCheckpoint {
    checkpoints: usize,
    drop_count: Rc<Cell<usize>>,
    target: Rc<Cell<Option<TaskId>>>,
}
impl LoopDriver for PartialCheckpoint {
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
        self.checkpoints += 1;
        if self.checkpoints == 2 {
            return Ok(CheckpointReport {
                state: CheckpointState::Pending,
                failures: vec![],
                suppressed_failures: 0,
            });
        }
        if self.checkpoints == 3 {
            assert_eq!(self.drop_count.get(), 0);
            assert_eq!(
                host.post(job(|_| {})).err().unwrap().reason,
                RejectionReason::Full
            );
            assert_eq!(
                host.cancel(self.target.get().unwrap()),
                CancelOutcome::Running
            );
        }
        Ok(CheckpointReport::empty())
    }
}
#[test]
fn running_interval_keeps_slot_and_value_through_pending_checkpoint() {
    let clock = Rc::new(ClockNow(Cell::new(Instant::now())));
    let drops = Rc::new(Cell::new(0));
    let target = Rc::new(Cell::new(None));
    let driver = PartialCheckpoint {
        checkpoints: 0,
        drop_count: drops.clone(),
        target: target.clone(),
    };
    let mut runtime = EventLoop::with_clock(
        driver,
        SchedulerConfig {
            max_jobs: 1,
            ..Default::default()
        },
        None,
        clock.clone(),
    )
    .unwrap();
    let guard = Dropped(drops.clone());
    target.set(Some(
        runtime
            .host()
            .timer(
                Duration::ZERO,
                Some(Duration::from_millis(1)),
                job(move |_| {
                    let _ = &guard;
                }),
            )
            .ok()
            .unwrap(),
    ));
    runtime.tick(false).unwrap();
    assert_eq!(drops.get(), 0);
    assert_eq!(runtime.stats().live_jobs, 1);
    clock.0.set(clock.now() + Duration::from_secs(1));
    runtime.tick(false).unwrap();
    assert_eq!(drops.get(), 1);
    assert_eq!(runtime.stats().live_jobs, 0);
}
