//! Phase 7: one turn path, cooperative checkpoints and detached failure policy.
use coldmind_runtime::{engine::*, scheduler::*};
use std::{
    cell::{Cell, RefCell},
    num::NonZeroUsize,
    rc::Rc,
};
#[derive(Debug)]
enum Job {
    Ok(u8),
    Fail,
    Panic,
    Stop,
}
struct Driver {
    seen: Rc<RefCell<Vec<u8>>>,
    checkpoints: Rc<Cell<usize>>,
    pending_once: bool,
}
fn script() -> FailureReport {
    FailureReport::new(
        FailureClass::Script,
        FailureCode::ScriptException,
        "application exception",
    )
}
impl LoopDriver for Driver {
    type Job = Job;
    type Message = ();
    fn execute_job(
        &mut self,
        job: &mut Job,
        host: &dyn LocalHost<Job>,
    ) -> Result<(), FailureReport> {
        match job {
            Job::Ok(n) => self.seen.borrow_mut().push(*n),
            Job::Fail => return Err(script()),
            Job::Panic => panic!("host panic"),
            Job::Stop => host.request_stop(),
        }
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
        self.checkpoints.set(self.checkpoints.get() + 1);
        Ok(CheckpointReport {
            state: if self.pending_once && self.checkpoints.get() == 2 {
                CheckpointState::Pending
            } else {
                CheckpointState::Empty
            },
            failures: vec![],
            suppressed_failures: 0,
        })
    }
}
type Setup = (Driver, Rc<RefCell<Vec<u8>>>, Rc<Cell<usize>>);
fn setup(pending: bool) -> Setup {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let cp = Rc::new(Cell::new(0));
    (
        Driver {
            seen: seen.clone(),
            checkpoints: cp.clone(),
            pending_once: pending,
        },
        seen,
        cp,
    )
}
#[test]
fn continue_requires_report_sink_before_any_entry() {
    let (driver, seen, cp) = setup(false);
    assert!(
        EventLoop::with_policy(
            driver,
            SchedulerConfig::default(),
            None,
            ScriptErrorPolicy::Continue
        )
        .is_err()
    );
    assert!(seen.borrow().is_empty());
    assert_eq!(cp.get(), 0);
}
#[test]
fn ordinary_task_waits_for_pending_checkpoint() {
    let (driver, seen, cp) = setup(true);
    let mut runtime = EventLoop::new(driver, SchedulerConfig::default(), None).unwrap();
    runtime.host().post(Job::Ok(1)).unwrap();
    runtime.host().post(Job::Ok(2)).unwrap();
    runtime.tick(false).unwrap();
    assert_eq!(*seen.borrow(), [1]);
    assert_eq!(cp.get(), 2);
    runtime.tick(false).unwrap();
    assert_eq!(*seen.borrow(), [1, 2]);
}
#[test]
fn continue_reports_unrelated_errors_once_and_keeps_fifo() {
    let (driver, seen, _) = setup(false);
    let reports = Rc::new(RefCell::new(Vec::new()));
    let sink = reports.clone();
    let mut runtime = EventLoop::with_policy(
        driver,
        SchedulerConfig::default(),
        Some(Rc::new(move |report| {
            sink.borrow_mut().push(report.clone())
        })),
        ScriptErrorPolicy::Continue,
    )
    .unwrap();
    runtime.host().post(Job::Fail).unwrap();
    let target = runtime.host().post(Job::Ok(42)).unwrap();
    runtime.host().post(Job::Ok(99)).unwrap();
    runtime.run_until(target).unwrap();
    assert_eq!(*seen.borrow(), [42]);
    assert_eq!(reports.borrow().len(), 1);
    assert!(runtime.last_failure().is_none());
}
#[test]
fn captured_continue_failure_returns_once_without_calling_sink_or_faulting() {
    let (driver, _, cp) = setup(true);
    let calls = Rc::new(Cell::new(0));
    let sink = calls.clone();
    let mut runtime = EventLoop::with_policy(
        driver,
        SchedulerConfig::default(),
        Some(Rc::new(move |_| sink.set(sink.get() + 1))),
        ScriptErrorPolicy::Continue,
    )
    .unwrap();
    let target = runtime.host().post(Job::Fail).unwrap();
    assert_eq!(runtime.run_until(target).unwrap_err(), script());
    assert_eq!(calls.get(), 0);
    assert_eq!(cp.get(), 3);
    assert!(runtime.last_failure().is_none());
    runtime.host().post(Job::Ok(1)).unwrap();
    runtime.tick(false).unwrap();
}
#[test]
fn host_panic_stops_even_in_continue_and_no_callback_is_retried() {
    let (driver, seen, _) = setup(false);
    let calls = Rc::new(Cell::new(0));
    let sink = calls.clone();
    let mut runtime = EventLoop::with_policy(
        driver,
        SchedulerConfig::default(),
        Some(Rc::new(move |_| sink.set(sink.get() + 1))),
        ScriptErrorPolicy::Continue,
    )
    .unwrap();
    runtime.host().post(Job::Panic).unwrap();
    runtime.host().post(Job::Ok(9)).unwrap();
    assert_eq!(
        runtime.tick(false).unwrap_err().code,
        FailureCode::HostPanic
    );
    assert!(seen.borrow().is_empty());
    assert_eq!(calls.get(), 1);
    assert!(runtime.tick(false).unwrap().stopped);
}
#[test]
fn stop_on_capture_never_returns_completion_or_enters_checkpoint() {
    let (driver, _, cp) = setup(false);
    let mut runtime = EventLoop::new(driver, SchedulerConfig::default(), None).unwrap();
    let target = runtime.host().post(Job::Stop).unwrap();
    assert_eq!(runtime.run_until(target).unwrap(), TurnOutcome::Stopped);
    assert_eq!(cp.get(), 1);
}
