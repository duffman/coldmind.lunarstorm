//! Owner pump failure, checkpoint normalization and stale-notification defenses.
use super::*;
use std::{cell::Cell, collections::VecDeque, sync::Arc};

#[derive(Default)]
struct Job {
    drop_hook: Option<Rc<dyn Fn()>>,
}
impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Job").finish_non_exhaustive()
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        if let Some(hook) = self.drop_hook.take() {
            hook();
        }
    }
}
type JobHook = Box<dyn FnMut(&dyn LocalHost<Job>)>;
#[derive(Default)]
struct Driver {
    checkpoints: VecDeque<CheckpointReport>,
    job: Option<FailureReport>,
    message: Option<FailureReport>,
    completion: Option<FailureReport>,
    initialize_panics: bool,
    stop_on_control: bool,
    stop_at_control: Option<usize>,
    controls_serviced: usize,
    checkpoint_calls: Rc<Cell<usize>>,
    control_host: Option<LoopHandle<()>>,
    calls: Rc<Cell<usize>>,
    on_job: Option<JobHook>,
    on_checkpoint: Option<JobHook>,
    checkpoint_error: Option<FailureReport>,
    shutdown_error: Option<FailureReport>,
    shutdown_panics: bool,
    pending_control: Option<Box<dyn Fn() -> bool>>,
}
fn script() -> FailureReport {
    FailureReport::new(
        FailureClass::Script,
        FailureCode::ScriptException,
        "test exception",
    )
}
impl LoopDriver for Driver {
    type Job = Job;
    type Message = ();
    fn initialize(&mut self, handle: LoopHandle<()>) -> Result<(), FailureReport> {
        assert!(!self.initialize_panics, "initialization panic");
        self.control_host = Some(handle);
        Ok(())
    }
    fn service_control(&mut self) -> Result<bool, FailureReport> {
        self.controls_serviced += 1;
        if std::mem::take(&mut self.stop_on_control)
            || self.stop_at_control == Some(self.controls_serviced)
        {
            self.control_host.as_ref().unwrap().request_stop()?;
            return Ok(true);
        }
        Ok(false)
    }
    fn has_control_work(&self) -> bool {
        self.pending_control
            .as_ref()
            .is_some_and(|pending| pending())
    }
    fn execute_job(&mut self, _: &mut Job, host: &dyn LocalHost<Job>) -> Result<(), FailureReport> {
        self.calls.set(self.calls.get() + 1);
        if let Some(hook) = &mut self.on_job {
            hook(host);
        }
        self.job.take().map_or(Ok(()), Err)
    }
    fn deliver_message(&mut self, _: (), _: &dyn LocalHost<Job>) -> Result<(), FailureReport> {
        self.calls.set(self.calls.get() + 1);
        self.message.take().map_or(Ok(()), Err)
    }
    fn deliver_completion(
        &mut self,
        _: OperationId,
        _: CompletionPayload,
        _: &dyn LocalHost<Job>,
    ) -> Result<(), FailureReport> {
        self.calls.set(self.calls.get() + 1);
        self.completion.take().map_or(Ok(()), Err)
    }
    fn checkpoint(
        &mut self,
        _: std::num::NonZeroUsize,
        host: &dyn LocalHost<Job>,
    ) -> Result<CheckpointReport, FailureReport> {
        self.checkpoint_calls.set(self.checkpoint_calls.get() + 1);
        if let Some(error) = self.checkpoint_error.take() {
            return Err(error);
        }
        if let Some(hook) = &mut self.on_checkpoint {
            hook(host);
        }
        Ok(self
            .checkpoints
            .pop_front()
            .unwrap_or_else(CheckpointReport::empty))
    }
    fn shutdown(&mut self) -> Result<(), FailureReport> {
        assert!(!self.shutdown_panics, "driver shutdown panic");
        self.shutdown_error.take().map_or(Ok(()), Err)
    }
}

#[test]
fn stop_from_control_service_prevents_startup_and_post_turn_checkpoint_entry() {
    for (at, expected_jobs, expected_checkpoints) in [(2, 0, 0), (3, 0, 1), (4, 1, 1)] {
        let mut owner = runtime(Driver {
            stop_at_control: Some(at),
            ..Default::default()
        });
        owner.host().post(Job::default()).unwrap();
        assert!(owner.tick(false).unwrap().stopped);
        assert_eq!(owner.driver.calls.get(), expected_jobs);
        assert_eq!(
            owner.driver.checkpoint_calls.get(),
            expected_checkpoints,
            "stop at control {at} must prevent another checkpoint entry"
        );
    }
}
#[test]
fn stop_from_the_checkpoint_after_a_full_batch_prevents_another_turn() {
    let mut owner = EventLoop::new(
        Driver {
            stop_at_control: Some(4),
            ..Default::default()
        },
        SchedulerConfig {
            task_batch: 1,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    owner.host().post(Job::default()).unwrap();
    owner.host().post(Job::default()).unwrap();
    assert!(owner.tick(false).unwrap().stopped);
    assert_eq!(owner.driver.calls.get(), 1);
    assert_eq!(owner.driver.checkpoint_calls.get(), 1);
    assert_eq!(owner.stats().live_jobs, 0);
}

#[test]
fn reserved_wake_failure_is_reported_before_startup_or_application_entry() {
    struct FailedWake;
    impl crate::transport::Wake for FailedWake {
        fn wake(&self) -> std::io::Result<()> {
            Err(std::io::Error::other("poll wake unavailable"))
        }
    }
    let mut owner = runtime(Driver::default());
    // Substitute only the OS-wake dependency; retain the real transport and
    // bind its owner and producer views to the same checked loop epoch.
    let transport =
        Transport::new(owner.handle().epoch(), &owner.config, Arc::new(FailedWake)).unwrap();
    owner.host.control = transport.control();
    owner.driver.control_host = Some(transport.handle());
    owner.transport = transport;
    owner.host().post(Job::default()).unwrap();
    let ticket = owner
        .host()
        .bind_operation(owner.host().reserve_operation(1024).unwrap())
        .unwrap();
    assert!(owner.transport.arm());
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Null)),
        PublishOutcome::PublishedWakeFailed { .. }
    ));
    let failure = owner.tick(false).unwrap_err();
    assert_eq!(failure.class, FailureClass::Host);
    assert_eq!(owner.driver.calls.get(), 0);
    assert_eq!(owner.driver.checkpoint_calls.get(), 0);
    assert!(owner.is_stopped());
    assert_eq!(owner.last_failure().unwrap().code, failure.code);
    assert_eq!(owner.stats().live_jobs, 0);
}
fn runtime(driver: Driver) -> EventLoop<Driver> {
    EventLoop::new(driver, SchedulerConfig::default(), None).unwrap()
}
#[test]
fn initialization_panic_is_detached_before_any_application_turn() {
    let driver = Driver {
        initialize_panics: true,
        ..Default::default()
    };
    let calls = driver.calls.clone();
    assert_eq!(
        EventLoop::new(driver, SchedulerConfig::default(), None)
            .err()
            .unwrap()
            .code,
        FailureCode::HostPanic
    );
    assert_eq!(calls.get(), 0);
}
#[test]
fn identity_exhaustion_is_permanent_and_poll_errors_are_classified() {
    let counter = AtomicU64::new(u64::MAX - 1);
    assert_eq!(loop_epoch(&counter).unwrap(), u64::MAX - 1);
    assert_eq!(
        loop_epoch(&counter).unwrap_err().code,
        FailureCode::Capacity
    );
    assert_eq!(
        loop_epoch(&counter).unwrap_err().code,
        FailureCode::Capacity
    );
    let setup = poll_setup_failure(std::io::Error::other("closed registry"));
    assert_eq!(
        (setup.class, setup.code),
        (FailureClass::Host, FailureCode::EngineFailure)
    );
}
#[test]
fn preallocation_failure_returns_capacity_and_recovers() {
    let config = SchedulerConfig {
        max_jobs: 2,
        task_batch: 2,
        ..Default::default()
    };
    // `local_tables` makes five allocations: two in the job arena, one in the ready queue and
    // two in the timer heap. The count was 6 since commit e9228d4 without a sixth allocation
    // in the code, so the last round never fired; set back to 5 on the owner's word.
    for index in 0..5 {
        let (result, fired) =
            crate::vm::test_allocator::fail_nth(index, || local_tables::<Job>(7, &config, 8));
        assert!(fired);
        assert_eq!(result.err().unwrap().code, FailureCode::Capacity);
    }
    for index in 0..2 {
        let (result, fired) = crate::vm::test_allocator::fail_nth(index, || scratch(&config));
        assert!(fired);
        assert_eq!(result.err().unwrap().code, FailureCode::Capacity);
    }
    assert!(local_tables::<Job>(7, &config, 8).is_ok());
    assert!(scratch(&config).is_ok());
}
#[test]
fn checkpoint_caps_reports_and_preserves_suppression_without_stopping_continue() {
    let reports = Rc::new(RefCell::new(Vec::new()));
    let output = reports.clone();
    let driver = Driver {
        checkpoints: VecDeque::from([CheckpointReport {
            state: CheckpointState::Empty,
            failures: vec![script(); 70],
            suppressed_failures: 3,
        }]),
        ..Default::default()
    };
    let mut loop_ = EventLoop::with_policy(
        driver,
        SchedulerConfig::default(),
        Some(Rc::new(move |report| {
            output.borrow_mut().push(report.clone())
        })),
        ScriptErrorPolicy::Continue,
    )
    .unwrap();
    loop_.tick(false).unwrap();
    let reports = reports.borrow();
    assert_eq!(reports.len(), 65);
    assert!(reports[64].truncated);
    assert!(
        String::from_utf16_lossy(reports[64].message.as_ref().unwrap().as_units())
            .contains("9 additional")
    );
    assert!(!loop_.is_stopped());
}
#[test]
fn continue_sink_panic_is_fatal_and_disabled_after_one_attempt() {
    let sink_calls = Rc::new(Cell::new(0));
    let calls = sink_calls.clone();
    let mut loop_ = EventLoop::with_policy(
        Driver {
            job: Some(script()),
            ..Default::default()
        },
        SchedulerConfig::default(),
        Some(Rc::new(move |_| {
            calls.set(calls.get() + 1);
            panic!("sink panic");
        })),
        ScriptErrorPolicy::Continue,
    )
    .unwrap();
    loop_.host().post(Job::default()).unwrap();
    assert_eq!(loop_.tick(false).unwrap_err().code, FailureCode::HostPanic);
    assert_eq!(sink_calls.get(), 1);
    assert!(loop_.tick(false).unwrap().stopped);
}
#[test]
fn message_and_completion_script_failures_follow_the_same_continue_policy() {
    let reports = Rc::new(RefCell::new(Vec::new()));
    let output = reports.clone();
    let mut loop_ = EventLoop::with_policy(
        Driver {
            message: Some(script()),
            completion: Some(script()),
            ..Default::default()
        },
        SchedulerConfig::default(),
        Some(Rc::new(move |report| {
            output.borrow_mut().push(report.clone())
        })),
        ScriptErrorPolicy::Continue,
    )
    .unwrap();
    loop_.handle().try_send(()).unwrap();
    let host = loop_.host();
    let ticket = host
        .bind_operation(host.reserve_operation(1024).unwrap())
        .unwrap();
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Null)),
        PublishOutcome::Published
    ));
    loop_.tick(false).unwrap();
    assert_eq!(loop_.driver.calls.get(), 2);
    assert_eq!(reports.borrow().len(), 2);
    assert_eq!(loop_.handle().stats().operations, 0);
}
#[test]
fn stale_ready_delivery_faults_instead_of_dispatching_twice() {
    for completion in [false, true] {
        let mut loop_ = runtime(Driver::default());
        let key = Key {
            slot: 0,
            generation: 999,
        };
        let delivery = if completion {
            Delivery::Completion(key)
        } else {
            Delivery::Message(key)
        };
        loop_
            .host
            .state
            .borrow_mut()
            .ready
            .push(loop_.config.max_jobs, ReadyItem::Delivery(delivery));
        assert_eq!(
            loop_.tick(false).unwrap_err().code,
            FailureCode::InvalidHandle
        );
        assert_eq!(loop_.driver.calls.get(), 0);
        assert!(loop_.is_stopped());
    }
}
#[test]
fn control_stop_prevents_startup_checkpoint() {
    let mut loop_ = runtime(Driver::default());
    assert!(loop_.finish_active_turn().unwrap());
    loop_.driver.stop_on_control = true;
    assert!(loop_.tick(false).unwrap().stopped);
    assert_eq!(loop_.driver.calls.get(), 0);
}

struct DebugHandoff {
    checks: Rc<Cell<usize>>,
    wake: Option<Arc<dyn Fn() -> Result<(), FailureReport> + Send + Sync>>,
}
impl LoopDriver for DebugHandoff {
    type Job = ();
    type Message = ();
    fn initialize(&mut self, handle: LoopHandle<()>) -> Result<(), FailureReport> {
        self.wake = Some(handle.wake_callback());
        Ok(())
    }
    fn has_control_work(&self) -> bool {
        let before = self.checks.get();
        self.checks.set(before + 1);
        if before == 0 {
            // Publish immediately after the owner's first pending snapshot, while
            // its application sleep gate is still Awake. That wake is coalesced.
            self.wake.as_ref().unwrap()().unwrap();
            false
        } else {
            true
        }
    }
    fn execute_job(&mut self, _: &mut (), _: &dyn LocalHost<()>) -> Result<(), FailureReport> {
        Ok(())
    }
    fn deliver_message(&mut self, _: (), _: &dyn LocalHost<()>) -> Result<(), FailureReport> {
        Ok(())
    }
    fn deliver_completion(
        &mut self,
        _: OperationId,
        _: CompletionPayload,
        _: &dyn LocalHost<()>,
    ) -> Result<(), FailureReport> {
        Ok(())
    }
    fn checkpoint(
        &mut self,
        _: std::num::NonZeroUsize,
        _: &dyn LocalHost<()>,
    ) -> Result<CheckpointReport, FailureReport> {
        Ok(CheckpointReport::empty())
    }
}
#[test]
fn debug_publication_between_pending_snapshot_and_poll_arm_is_rechecked() {
    let checks = Rc::new(Cell::new(0));
    let mut owner = EventLoop::new(
        DebugHandoff {
            checks: checks.clone(),
            wake: None,
        },
        SchedulerConfig::default(),
        None,
    )
    .unwrap();
    let fallback = owner.handle();
    let (done, finished) = std::sync::mpsc::sync_channel(1);
    let watchdog = std::thread::spawn(move || {
        if finished
            .recv_timeout(std::time::Duration::from_secs(1))
            .is_err()
        {
            fallback.request_stop().unwrap();
        }
    });
    owner.tick(true).unwrap();
    let _ = done.send(());
    watchdog.join().unwrap();
    assert_eq!(
        checks.get(),
        2,
        "debug readiness must be rechecked after arming the common OS wake gate"
    );
    assert!(
        !owner.is_stop_requested(),
        "the watchdog must not be needed to rescue the poll"
    );
}

#[test]
fn an_unrepresentable_interval_faults_after_its_one_accepted_firing() {
    let mut owner = runtime(Driver::default());
    owner
        .host()
        .timer(
            std::time::Duration::ZERO,
            Some(std::time::Duration::MAX),
            Job::default(),
        )
        .unwrap();
    assert_eq!(owner.tick(false).unwrap_err().code, FailureCode::Capacity);
    assert_eq!(owner.driver.calls.get(), 1);
    assert!(owner.is_stopped());
    assert_eq!(owner.stats().live_jobs, 0);
}

#[test]
fn a_future_system_clock_timer_wakes_the_idle_owner_and_finishes_once() {
    let mut owner = runtime(Driver::default());
    let target = owner
        .host()
        .timer(std::time::Duration::from_millis(10), None, Job::default())
        .unwrap();
    assert_eq!(owner.run_until(target).unwrap(), TurnOutcome::Completed);
    assert_eq!(owner.driver.calls.get(), 1);
    assert_eq!(owner.stats().live_jobs, 0);
}

#[test]
fn pending_checkpoints_preserve_fifo_and_capture_across_slices() {
    fn report(state: CheckpointState) -> CheckpointReport {
        CheckpointReport {
            state,
            failures: Vec::new(),
            suppressed_failures: 0,
        }
    }
    let mut owner = runtime(Driver {
        checkpoints: VecDeque::from([
            report(CheckpointState::Pending),
            report(CheckpointState::Empty),
            report(CheckpointState::Pending),
            report(CheckpointState::Empty),
        ]),
        ..Default::default()
    });
    let target = owner.host().post(Job::default()).unwrap();
    owner.host().post(Job::default()).unwrap();
    assert!(owner.tick(false).unwrap().made_progress);
    assert_eq!(owner.driver.calls.get(), 0);
    assert!(owner.tick(false).unwrap().made_progress);
    assert_eq!(owner.driver.calls.get(), 1);
    assert_eq!(owner.run_until(target).unwrap(), TurnOutcome::Completed);
    assert_eq!(
        owner.driver.calls.get(),
        1,
        "capture must leave later work queued"
    );
    owner.tick(false).unwrap();
    assert_eq!(owner.driver.calls.get(), 2);
    assert_eq!(owner.run_until(target).unwrap(), TurnOutcome::Canceled);
}

#[test]
fn captured_script_failure_is_returned_once_without_reporting_or_faulting_continue() {
    let sink_calls = Rc::new(Cell::new(0));
    let seen = sink_calls.clone();
    let mut owner = EventLoop::with_policy(
        Driver {
            job: Some(script()),
            ..Default::default()
        },
        SchedulerConfig::default(),
        Some(Rc::new(move |_| seen.set(seen.get() + 1))),
        ScriptErrorPolicy::Continue,
    )
    .unwrap();
    let target = owner.host().post(Job::default()).unwrap();
    assert_eq!(
        owner.run_until(target).unwrap_err().class,
        FailureClass::Script
    );
    assert_eq!(sink_calls.get(), 0);
    assert!(!owner.is_stop_requested());
    assert!(owner.last_failure().is_none());
    let next = owner.host().post(Job::default()).unwrap();
    assert_eq!(owner.run_until(next).unwrap(), TurnOutcome::Completed);
    let foreign = TaskId {
        epoch: owner.handle().epoch() + 1,
        ..next
    };
    assert_eq!(
        owner.run_until(foreign).unwrap_err().code,
        FailureCode::WrongOwner
    );
    owner.shutdown().unwrap();
    assert_eq!(owner.run_until(next).unwrap(), TurnOutcome::Stopped);
}

#[test]
fn cleanup_panics_preserve_first_failure_and_close_every_remaining_job() {
    let dropped = Rc::new(Cell::new(0));
    let first = dropped.clone();
    let second = dropped.clone();
    let mut owner = runtime(Driver {
        shutdown_panics: true,
        ..Default::default()
    });
    owner
        .host()
        .post(Job {
            drop_hook: Some(Rc::new(move || {
                first.set(first.get() + 1);
                panic!("job cleanup");
            })),
        })
        .unwrap();
    owner
        .host()
        .post(Job {
            drop_hook: Some(Rc::new(move || second.set(second.get() + 1))),
        })
        .unwrap();
    let failure = owner.shutdown().unwrap_err();
    assert_eq!(failure.code, FailureCode::HostPanic);
    assert!(
        String::from_utf16_lossy(failure.message.as_ref().unwrap().as_units())
            .contains("job destructor")
    );
    assert_eq!(dropped.get(), 2);
    assert_eq!(owner.stats().live_jobs, 0);
    assert_eq!(owner.stats().ready_nodes, 0);
    assert!(owner.is_stopped());
    owner.shutdown().unwrap();

    let mut owner = runtime(Driver {
        shutdown_error: Some(FailureReport::fault(
            FailureCode::EngineFailure,
            "close failed",
        )),
        ..Default::default()
    });
    assert_eq!(
        owner.shutdown().unwrap_err().code,
        FailureCode::EngineFailure
    );
    owner.shutdown().unwrap();
}

#[test]
fn owner_and_report_sink_panics_become_one_terminal_host_fault() {
    for callback_panic in [false, true] {
        let seen = Rc::new(Cell::new(0));
        let output = seen.clone();
        let driver = Driver {
            job: (!callback_panic).then(script),
            on_job: callback_panic.then(|| {
                Box::new(|_: &dyn LocalHost<Job>| -> () { panic!("callback") }) as JobHook
            }),
            ..Default::default()
        };
        let mut owner = EventLoop::new(
            driver,
            SchedulerConfig::default(),
            Some(Rc::new(move |_| {
                output.set(output.get() + 1);
                panic!("sink");
            })),
        )
        .unwrap();
        owner.host().post(Job::default()).unwrap();
        assert_eq!(owner.tick(false).unwrap_err().code, FailureCode::HostPanic);
        assert_eq!(seen.get(), 1);
        assert!(owner.is_stopped());
        assert!(owner.tick(false).unwrap().stopped);
        assert_eq!(seen.get(), 1);
    }
}

#[test]
fn successful_message_and_completion_deliveries_release_both_permits() {
    let mut owner = runtime(Driver::default());
    owner.handle().try_send(()).unwrap();
    let host = owner.host();
    let ticket = host
        .bind_operation(host.reserve_operation(1024).unwrap())
        .unwrap();
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Null)),
        PublishOutcome::Published
    ));
    owner.tick(false).unwrap();
    assert_eq!(owner.driver.calls.get(), 2);
    assert_eq!(owner.handle().stats().messages, 0);
    assert_eq!(owner.handle().stats().operations, 0);
}

#[test]
fn captured_stop_wins_at_both_callback_batch_boundaries() {
    for batch in [1, 2] {
        let mut owner = EventLoop::new(
            Driver {
                on_job: Some(Box::new(|host| host.request_stop())),
                ..Default::default()
            },
            SchedulerConfig {
                task_batch: batch,
                ..Default::default()
            },
            None,
        )
        .unwrap();
        let target = owner.host().post(Job::default()).unwrap();
        owner.host().post(Job::default()).unwrap();
        assert_eq!(owner.run_until(target).unwrap(), TurnOutcome::Stopped);
        assert_eq!(owner.driver.calls.get(), 1);
        assert!(owner.is_stopped());
    }
}

#[test]
fn a_full_batch_retains_its_pending_checkpoint_and_then_returns_capture() {
    let mut owner = EventLoop::new(
        Driver {
            checkpoints: VecDeque::from([
                CheckpointReport::empty(),
                CheckpointReport {
                    state: CheckpointState::Pending,
                    failures: vec![],
                    suppressed_failures: 0,
                },
                CheckpointReport::empty(),
            ]),
            ..Default::default()
        },
        SchedulerConfig {
            task_batch: 1,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    let target = owner.host().post(Job::default()).unwrap();
    owner.tick(false).unwrap();
    assert!(owner.current.is_some());
    assert_eq!(owner.run_until(target).unwrap(), TurnOutcome::Completed);
    assert_eq!(owner.driver.calls.get(), 1);
}

#[test]
fn fatal_checkpoint_report_wins_over_script_errors_and_missing_continue_sink_is_rejected() {
    assert_eq!(
        EventLoop::with_policy(
            Driver::default(),
            SchedulerConfig::default(),
            None,
            ScriptErrorPolicy::Continue
        )
        .err()
        .unwrap()
        .code,
        FailureCode::Unsupported
    );
    for direct in [false, true] {
        let fatal = FailureReport::fault(FailureCode::EngineFailure, "native checkpoint failed");
        let mut owner = runtime(if direct {
            Driver {
                checkpoint_error: Some(fatal),
                ..Default::default()
            }
        } else {
            Driver {
                checkpoints: VecDeque::from([CheckpointReport {
                    state: CheckpointState::Empty,
                    failures: vec![script(), fatal],
                    suppressed_failures: 0,
                }]),
                ..Default::default()
            }
        });
        assert_eq!(
            owner.tick(false).unwrap_err().code,
            FailureCode::EngineFailure
        );
        assert!(owner.is_stopped());
        assert_eq!(owner.driver.calls.get(), 0);
    }
}

#[test]
fn timer_rejection_returns_the_job_and_running_interval_cancellation_prevents_rearm() {
    use std::time::Duration;
    let mut owner = EventLoop::new(
        Driver::default(),
        SchedulerConfig {
            max_jobs: 1,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    let host = owner.host();
    assert_eq!(
        host.timer(Duration::ZERO, Some(Duration::ZERO), Job::default())
            .unwrap_err()
            .reason,
        RejectionReason::InvalidPeriod
    );
    assert_eq!(
        host.timer(Duration::MAX, None, Job::default())
            .unwrap_err()
            .reason,
        RejectionReason::DeadlineOverflow
    );
    let timer = host.timer(Duration::ZERO, None, Job::default()).unwrap();
    assert_eq!(
        host.timer(Duration::ZERO, None, Job::default())
            .unwrap_err()
            .reason,
        RejectionReason::Full
    );
    assert_eq!(host.cancel(timer), CancelOutcome::Canceled);
    host.state.borrow_mut().timers.exhaust_sequence_for_test();
    let rejected = host
        .timer(Duration::ZERO, None, Job::default())
        .unwrap_err();
    assert_eq!(rejected.reason, RejectionReason::IdentityExhausted);
    let task = host.post(rejected.value).unwrap();
    assert_eq!(owner.run_until(task).unwrap(), TurnOutcome::Completed);
    owner.shutdown().unwrap();
    assert_eq!(
        host.timer(Duration::ZERO, None, Job::default())
            .unwrap_err()
            .reason,
        RejectionReason::Closed
    );

    let identity = Rc::new(Cell::new(None));
    let inside = identity.clone();
    let mut owner = runtime(Driver {
        on_job: Some(Box::new(move |host| {
            assert_eq!(host.cancel(inside.get().unwrap()), CancelOutcome::Running)
        })),
        ..Default::default()
    });
    identity.set(Some(
        owner
            .host()
            .timer(Duration::ZERO, Some(Duration::from_secs(1)), Job::default())
            .unwrap(),
    ));
    owner.tick(false).unwrap();
    assert_eq!(owner.driver.calls.get(), 1);
    assert_eq!(owner.stats().live_jobs, 0);
}

#[test]
fn interval_rearms_before_sequence_exhaustion_and_cleans_up_the_second_firing() {
    use std::time::{Duration, Instant};
    struct ClockNow(Cell<Instant>);
    impl Clock for ClockNow {
        fn now(&self) -> Instant {
            self.0.get()
        }
    }
    let clock = Rc::new(ClockNow(Cell::new(Instant::now())));
    let mut owner = EventLoop::with_clock(
        Driver::default(),
        SchedulerConfig::default(),
        None,
        clock.clone(),
    )
    .unwrap();
    let host = owner.host();
    host.timer(
        Duration::ZERO,
        Some(Duration::from_millis(1)),
        Job::default(),
    )
    .unwrap();
    owner.tick(false).unwrap();
    assert_eq!(owner.driver.calls.get(), 1);
    assert_eq!(owner.stats().live_jobs, 1);
    clock.0.set(clock.now() + Duration::from_millis(1));
    host.state.borrow_mut().timers.exhaust_sequence_for_test();
    assert_eq!(owner.tick(false).unwrap_err().code, FailureCode::Capacity);
    assert_eq!(owner.driver.calls.get(), 2);
    assert_eq!(owner.stats().live_jobs, 0);
    assert!(owner.is_stopped());
}

#[test]
fn owner_local_cancellation_checks_foreign_stale_and_running_single_shot_ids() {
    let mut owner = runtime(Driver::default());
    let host = owner.host();
    let task = host.post(Job::default()).unwrap();
    assert_eq!(
        host.cancel(TaskId {
            epoch: task.epoch + 1,
            ..task
        }),
        CancelOutcome::WrongOwner
    );
    assert_eq!(host.cancel(task), CancelOutcome::Canceled);
    assert_eq!(host.cancel(task), CancelOutcome::NotFound);
    let inside = Rc::new(Cell::new(None));
    let identity = inside.clone();
    owner.driver.on_job = Some(Box::new(move |host| {
        assert_eq!(host.cancel(identity.get().unwrap()), CancelOutcome::TooLate)
    }));
    let task = host.post(Job::default()).unwrap();
    inside.set(Some(task));
    assert_eq!(owner.run_until(task).unwrap(), TurnOutcome::Completed);
    assert_eq!(host.cancel(task), CancelOutcome::NotFound);
}

#[test]
fn until_turn_repeats_pending_slices_and_reports_a_terminal_pump_failure() {
    let pending = || CheckpointReport {
        state: CheckpointState::Pending,
        failures: vec![],
        suppressed_failures: 0,
    };
    let mut owner = runtime(Driver {
        checkpoints: VecDeque::from([
            pending(),
            CheckpointReport::empty(),
            pending(),
            CheckpointReport::empty(),
        ]),
        ..Default::default()
    });
    let task = owner.host().post(Job::default()).unwrap();
    assert_eq!(owner.run_until(task).unwrap(), TurnOutcome::Completed);
    assert_eq!(owner.driver.calls.get(), 1);
    assert_eq!(owner.driver.checkpoint_calls.get(), 4);
    let mut owner = runtime(Driver {
        job: Some(FailureReport::fault(
            FailureCode::EngineFailure,
            "dispatch failed",
        )),
        ..Default::default()
    });
    let task = owner.host().post(Job::default()).unwrap();
    assert_eq!(
        owner.run_until(task).unwrap_err().code,
        FailureCode::EngineFailure
    );
    assert!(owner.host.state.borrow().capture.is_none());
    assert!(owner.is_stopped());
}

#[test]
fn capture_returns_after_the_checkpoint_at_the_end_of_a_full_batch() {
    let mut owner = EventLoop::new(
        Driver::default(),
        SchedulerConfig {
            task_batch: 1,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    let target = owner.host().post(Job::default()).unwrap();
    assert_eq!(owner.run_until(target).unwrap(), TurnOutcome::Completed);
    assert_eq!(owner.driver.calls.get(), 1);
}

#[test]
#[cfg(not(miri))]
fn owner_disposal_reports_transport_failures_without_skipping_driver_close() {
    let mut owner = runtime(Driver::default());
    owner.handle().install_stop_hook(Arc::new(|| {
        Err(FailureReport::fault(
            FailureCode::EngineFailure,
            "stop failed",
        ))
    }));
    assert_eq!(
        owner.shutdown().unwrap_err().code,
        FailureCode::EngineFailure
    );
    assert!(owner.is_stopped());
}

#[test]
fn older_callback_can_cancel_the_waited_for_turn_before_dispatch() {
    let selected = Rc::new(Cell::new(None));
    let target = selected.clone();
    let mut owner = runtime(Driver {
        on_job: Some(Box::new(move |host| {
            assert_eq!(host.cancel(target.get().unwrap()), CancelOutcome::Canceled)
        })),
        ..Default::default()
    });
    owner.host().post(Job::default()).unwrap();
    let task = owner.host().post(Job::default()).unwrap();
    selected.set(Some(task));
    assert_eq!(owner.run_until(task).unwrap(), TurnOutcome::Canceled);
    assert_eq!(owner.driver.calls.get(), 1);
    assert_eq!(owner.stats().live_jobs, 0);
}

#[test]
fn pending_debug_recheck_uses_the_same_driver_as_normal_turns() {
    let mut owner = runtime(Driver::default());
    let checks = Rc::new(Cell::new(0));
    let inspected = checks.clone();
    let wake = owner.handle().wake_callback();
    owner.driver.pending_control = Some(Box::new(move || {
        let count = inspected.get();
        inspected.set(count + 1);
        if count == 0 {
            wake().unwrap();
            false
        } else {
            true
        }
    }));
    let fallback = owner.handle();
    let (done, finished) = std::sync::mpsc::sync_channel(1);
    let watchdog = std::thread::spawn(move || {
        if finished
            .recv_timeout(std::time::Duration::from_secs(1))
            .is_err()
        {
            fallback.request_stop().unwrap();
        }
    });
    owner.tick(true).unwrap();
    let _ = done.send(());
    watchdog.join().unwrap();
    assert_eq!(checks.get(), 2);
    assert!(!owner.is_stop_requested());
}

#[test]
fn run_until_idle_on_a_fresh_loop_returns_after_the_startup_checkpoint() {
    let mut owner = runtime(Driver::default());
    let rescue = owner.handle();
    crate::vm::contract_fixture::with_watchdog(
        move || drop(rescue.request_stop()),
        || owner.run_until_idle(),
    )
    .unwrap();
    assert!(!owner.is_stop_requested(), "the loop waited for nothing");
    assert!(!owner.has_tracked_work());
}
