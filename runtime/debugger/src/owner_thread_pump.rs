//! Cooperative owner-thread pumping of local jobs, timers, and transport deliveries.

use crate::{
    arena::{Arena, Key},
    config::SchedulerConfig,
    driver::LoopDriver,
    engine::*,
    host::{Host, LocalHost, LocalState, ReadyItem, TurnOutcome},
    owner_io::OwnerIo,
    ready::ReadyQueue,
    timers::{Clock, MonotonicClock, TimerHeap, next_period},
    transport::{Delivery, LoopHandle, Transport},
};
use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static NEXT_LOOP: AtomicU64 = AtomicU64::new(1);
#[cfg(test)]
#[path = "../tests/unit/runner.rs"]
mod tests;

fn capacity_failure() -> FailureReport {
    FailureReport::fault(
        FailureCode::Capacity,
        "scheduler allocation or identity limit",
    )
}
fn loop_epoch(counter: &AtomicU64) -> Result<u64, FailureReport> {
    counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| capacity_failure())
}
/// Maps a failure to build the owner reactor (kqueue/epoll setup) to a host failure.
///
/// # Arguments
///
/// * `error` - the platform error from the reactor constructor
///
/// # Returns
///
/// A host `EngineFailure` report naming the platform error.
fn poll_setup_failure(error: std::io::Error) -> FailureReport {
    FailureReport::new(
        FailureClass::Host,
        FailureCode::EngineFailure,
        &format!("poll backend failed: {error}"),
    )
}
/// How recent the last reactor turn must be for a slice that may block to skip its own.
///
/// Decision: 100 microseconds. A request at one connection takes about 55 from wake to
/// wait, in three slices; the slack keeps those slices free of reactor turns and adds at
/// most 0.1 ms before a pending accept or read is seen. The slack was introduced when a
/// nonblocking turn cost 12 microseconds on macOS; with the patched mio (vendor/mio) a turn
/// costs 0.7. Tried and dropped after that patch: no slack. The HTTP benchmark
/// (scripts/bench-http.sh) showed no gain at 64 connections or at one.
const REACTOR_SLACK: Duration = Duration::from_micros(100);
/// How long the owner watches the transport for a new publication before its idle wait,
/// after a slice that delivered one.
///
/// Fix: the owner parked in the reactor right after answering a cross-thread call, so the
/// caller's next call paid a wake system call and the owner a context switch back in.
/// Decision: 5 microseconds of spinning on `Transport::published`, only when the slices
/// since the last wait delivered a transport message or completion; a loop that serves
/// sockets or timers alone never spins.
const OWNER_SPIN: Duration = Duration::from_micros(5);

fn local_tables<J>(
    epoch: u64,
    config: &SchedulerConfig,
    ready_capacity: usize,
) -> Result<LocalState<J>, FailureReport> {
    Ok(LocalState {
        epoch,
        jobs: Arena::new(config.max_jobs).map_err(|_| capacity_failure())?,
        ready: ReadyQueue::new(ready_capacity).map_err(|_| capacity_failure())?,
        timers: TimerHeap::new(config.max_jobs).map_err(|_| capacity_failure())?,
        stopping: false,
        capture: None,
        unrefed: std::collections::HashSet::new(),
    })
}
fn scratch(config: &SchedulerConfig) -> Result<(Vec<Delivery>, Vec<TaskId>), FailureReport> {
    let mut deliveries = Vec::new();
    deliveries
        .try_reserve_exact(config.task_batch)
        .map_err(|_| capacity_failure())?;
    let mut controls = Vec::new();
    controls
        .try_reserve_exact(config.task_batch)
        .map_err(|_| capacity_failure())?;
    Ok((deliveries, controls))
}
/// Policy for application exceptions; host, contract and engine faults always stop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScriptErrorPolicy {
    /// Stop and retain the detached failure.
    #[default]
    Stop,
    /// Report uncaptured application failures through a required sink, then finish
    /// the same turn's checkpoint before dispatching another ordinary task.
    Continue,
}
/// Owner-thread callback receiving detached application and terminal failures.
///
/// Reports are bounded owned data. Continue requires this sink; Stop remains the
/// default. A sink panic is contained and replaces the returned/stored
/// diagnostic with a host-panic report; that sink is then disabled.
pub type ReportSink = Rc<dyn Fn(&FailureReport)>;
/// Outcome of one bounded pump slice, including a cooperative checkpoint slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopTick {
    /// Whether the owner advanced work, including a checkpoint that remains pending.
    pub made_progress: bool,
    /// Whether shutdown has closed transport and disposed admitted local jobs.
    pub stopped: bool,
}
/// Owner-local scheduling counters; transport admission has separate statistics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopStats {
    /// Live local job records, including timers and the current unfinished turn.
    pub live_jobs: usize,
    /// Linked ready entries, including imported messages and operation completions.
    pub ready_nodes: usize,
    /// Reserved local job-table slots, including currently unused slots.
    pub allocated_slots: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PumpGoal {
    Continuous,
    OneSlice,
    UntilTurn(TaskId),
}
/// Only callbacks and delivered application data own a following engine
/// checkpoint. Native I/O service cannot leave an active engine turn behind.
#[derive(Clone, Copy)]
enum ActiveTurn {
    Job(Key),
    Delivery(Delivery),
    /// A socket event the owner reactor published to its inbox.
    OwnerDelivery(Key),
}
/// Owner-thread event loop combining bounded scheduling and cross-thread transport.
///
/// Construct and pump the loop on its owner thread. Local jobs and the driver may
/// contain non-`Send` native state; transferable producers receive only a
/// [`LoopHandle`], or a completion ticket obtained through [`LocalHost`]. This
/// generic scheduler does not create a JavaScript VM; native sources publish owned
/// messages through the same bounded transport used by other producers.
///
/// A startup checkpoint must become empty before the first ordinary turn. Every
/// local callback, message, or completion is followed by another checkpoint, and
/// `Pending` blocks later ordinary turns until that checkpoint finishes. Native
/// readiness still advances in bounded batches between checkpoint slices, without
/// entering the engine or dispatching the resulting application messages. Stop is
/// cooperative and takes priority at owner checks. Driver errors, reported script
/// failures, and driver panics stop the loop and retain a bounded diagnostic.
/// Dropping the owner closes transport and disposes queued local jobs without
/// dispatching them; retained host/producer handles cannot restart it.
pub struct EventLoop<D: LoopDriver> {
    driver: D,
    host: Host<D::Job>,
    config: SchedulerConfig,
    /// The loop's only reactor: a current-thread tokio runtime on this owner thread.
    /// Sockets, timers and cross-thread wakes all land here; one wait per idle slice.
    /// `None` only after disposal closed it.
    ///
    /// Decision (2026-11-18): the raw Mio `NativeSource` backend that used to run beside
    /// it as a second reactor (chosen per VM by granted capabilities) is archived in
    /// `.archive/2026-11-18/src/io`. Two reactors meant two wait paths, two wake protocols
    /// and a cfg split in every pump phase; JS-visible networking only ever used this one.
    owner_io: Option<OwnerIo<D::Message>>,
    /// When the reactor last asked the operating system for readiness.
    reactor_at: Instant,
    /// A transport delivery was imported since the last idle wait; see [`OWNER_SPIN`].
    transport_recent: bool,
    transport: Transport<D::Message>,
    deliveries: Vec<Delivery>,
    controls: Vec<TaskId>,
    startup: bool,
    current: Option<ActiveTurn>,
    last_failure: Option<FailureReport>,
    sink: Option<ReportSink>,
    policy: ScriptErrorPolicy,
    capture_failure: Option<FailureReport>,
    shutdown_complete: bool,
}
impl<D: LoopDriver> EventLoop<D> {
    /// Creates a loop using the system monotonic clock and a fresh transport epoch.
    ///
    /// Validates configuration and reserves bounded storage before returning.
    /// Invalid/exhausted capacities return `Capacity`; poll setup failures return
    /// a host `EngineFailure`. Calls the driver's initialization hook after all
    /// storage is ready, before any checkpoint or ordinary turn.
    pub fn new(
        driver: D,
        config: SchedulerConfig,
        sink: Option<ReportSink>,
    ) -> Result<Self, FailureReport> {
        Self::with_clock(driver, config, sink, Rc::new(MonotonicClock))
    }
    /// Construct with explicit script-error policy. Continue requires a sink and
    /// still stops on engine/contract/host faults. Validation precedes driver entry.
    pub fn with_policy(
        driver: D,
        config: SchedulerConfig,
        sink: Option<ReportSink>,
        policy: ScriptErrorPolicy,
    ) -> Result<Self, FailureReport> {
        Self::construct(driver, config, sink, Rc::new(MonotonicClock), policy)
    }
    /// Creates a loop with an owner-local monotonic clock implementation.
    ///
    /// Allocation and callback behavior match [`Self::new`]. Injected clocks must
    /// never move backwards. A virtual test clock should normally be paired with
    /// nonblocking ticks, because OS poll waits do not advance that clock.
    pub fn with_clock(
        driver: D,
        config: SchedulerConfig,
        sink: Option<ReportSink>,
        clock: Rc<dyn Clock>,
    ) -> Result<Self, FailureReport> {
        Self::construct(driver, config, sink, clock, ScriptErrorPolicy::Stop)
    }
    fn construct(
        mut driver: D,
        config: SchedulerConfig,
        sink: Option<ReportSink>,
        clock: Rc<dyn Clock>,
        policy: ScriptErrorPolicy,
    ) -> Result<Self, FailureReport> {
        config.validate()?;
        if policy == ScriptErrorPolicy::Continue && sink.is_none() {
            return Err(FailureReport::fault(
                FailureCode::Unsupported,
                "Continue requires a report sink",
            ));
        }
        let epoch = loop_epoch(&NEXT_LOOP)?;
        let validator = driver.message_validator();
        let owner_io = OwnerIo::new(config.max_messages, validator.clone()).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::Unsupported => {
                    FailureReport::fault(FailureCode::Unsupported, &error.to_string())
                }
                _ => poll_setup_failure(error),
            }
        })?;
        let transport = Transport::new(epoch, &config, owner_io.waker())?.with_validator(validator);
        // Jobs, transport deliveries, completions and owner-reactor socket events share
        // one ready queue; each kind owns a disjoint node range.
        let ready_capacity = config
            .max_jobs
            .checked_add(config.max_messages)
            .and_then(|n| n.checked_add(config.max_operations))
            .and_then(|n| n.checked_add(config.max_messages))
            .ok_or_else(capacity_failure)?;
        let host = Host {
            state: Rc::new(RefCell::new(local_tables(epoch, &config, ready_capacity)?)),
            clock,
            control: transport.control(),
        };
        let (deliveries, controls) = scratch(&config)?;
        match catch_unwind(AssertUnwindSafe(|| {
            driver.attach_owner_io(owner_io.handle());
            driver.initialize(transport.handle())
        })) {
            Ok(result) => result?,
            Err(_) => {
                return Err(FailureReport::new(
                    FailureClass::Host,
                    FailureCode::HostPanic,
                    "driver initialization panicked",
                ));
            }
        }
        Ok(Self {
            driver,
            host,
            config,
            owner_io: Some(owner_io),
            reactor_at: Instant::now(),
            transport_recent: false,
            transport,
            deliveries,
            controls,
            startup: true,
            current: None,
            last_failure: None,
            sink,
            policy,
            capture_failure: None,
            shutdown_complete: false,
        })
    }
    /// Clones an owner-local host sharing this loop's job tables and admission state.
    ///
    /// The host may be used during callbacks; it cannot be sent to a worker or
    /// dispatch work by itself. Its admission closes when the loop stops or drops.
    pub fn host(&self) -> Host<D::Job> {
        self.host.clone()
    }
    /// Clones a transferable producer handle for this loop's existing transport.
    ///
    /// It admits owned messages and metadata controls without carrying driver or
    /// native engine values. Cloning it never creates or starts another loop.
    pub fn handle(&self) -> LoopHandle<D::Message> {
        self.transport.handle()
    }
    /// Snapshots local scheduling usage without executing callbacks.
    ///
    /// Use [`LoopHandle::stats`] for ordinary-message and reserved-operation usage.
    pub fn stats(&self) -> LoopStats {
        let state = self.host.state.borrow();
        LoopStats {
            live_jobs: state.jobs.len(),
            ready_nodes: state.ready.len(),
            allocated_slots: state.jobs.capacity(),
        }
    }
    /// What keeps the loop alive, for the end-of-run trace report: keepalive jobs (timers,
    /// intervals), ready turns, queued messages, native operations in flight, open I/O sources.
    pub fn pending_work(&self) -> serde_json::Value {
        let usage = self.transport.handle().stats();
        let local = self.host.state.borrow();
        serde_json::json!({
            "keepalive_jobs": local.keepalive_jobs(),
            "live_jobs": local.jobs.len(),
            "next_timer_ms": local.timers.next().map(|d| {
                d.saturating_duration_since(self.host.clock.now()).as_secs_f64() * 1e3
            }),
            "ready": local.ready.len(),
            "messages": usage.messages,
            "operations": usage.operations,
            "socket_events": self.owner_io.as_ref().map_or(0, |reactor| reactor.len()),
            "turn_running": self.current.is_some(),
        })
    }
    /// Reports tracked keepalive work without entering the driver or waiting.
    ///
    /// Takes no transport lock. External producers may submit after this snapshot; idle is not admission closure.
    pub fn has_tracked_work(&self) -> bool {
        if self
            .owner_io
            .as_ref()
            .is_some_and(|reactor| reactor.len() != 0)
        {
            return true;
        }
        // Fix: each query locked the transport for a full `stats()` snapshot and scanned the
        // unref set. Decision: lock-free transport counters plus `keepalive_jobs` without a scan.
        let local = self.host.state.borrow();
        self.startup
            || self.current.is_some()
            || local.keepalive_jobs() != 0
            || local.ready.len() != 0
            || self.transport.has_admitted_work()
            || self.driver.has_control_work()
    }
    /// The startup checkpoint has not run. It counts as tracked work, but nothing wakes an
    /// idle wait after it, so a caller draining tracked work must not block for it.
    pub(crate) fn startup_pending(&self) -> bool {
        self.startup
    }
    /// Pumps the existing scheduler until no tracked work remains, or stop/failure.
    /// Registered I/O, pending operations, and timers retain their existing keepalive semantics.
    pub fn run_until_idle(&mut self) -> Result<(), FailureReport> {
        while !self.is_stopped() && !self.is_stop_requested() && self.has_tracked_work() {
            self.tick(!self.startup)?;
        }
        if self.is_stop_requested() && !self.is_stopped() {
            self.shutdown()?;
        }
        Ok(())
    }
    /// Report a published stop request even before owner teardown has run.
    pub fn is_stop_requested(&self) -> bool {
        self.stopping()
    }
    /// Report completed owner teardown; no application work can run afterward.
    pub fn is_stopped(&self) -> bool {
        self.shutdown_complete
    }
    /// Idempotently close admission and dispose owner resources without running
    /// application callbacks, checkpoints, or JavaScript settlement during cleanup.
    pub fn shutdown(&mut self) -> Result<(), FailureReport> {
        self.host.request_stop();
        let first = !self.shutdown_complete;
        self.dispose();
        if first && let Some(error) = self.last_failure.as_ref() {
            return Err(error.clone());
        }
        Ok(())
    }
    pub(crate) fn driver(&self) -> &D {
        &self.driver
    }
    /// Borrows the bounded detached diagnostic from the last terminal pump failure.
    ///
    /// Remains available after shutdown. Normal stop and a rejected foreign task
    /// ID do not create a failure here. No scoped engine handle is retained.
    pub fn last_failure(&self) -> Option<&FailureReport> {
        self.last_failure.as_ref()
    }
    /// Pumps one cooperative slice of maintenance, checkpoints, and ordinary turns.
    ///
    /// `block` permits an idle poll wait for a producer wake or timer deadline;
    /// `false` always uses a nonblocking poll. Pending checkpoints return to the
    /// caller without dispatching newer ordinary turns, while native I/O may
    /// progress and publish bounded messages for later dispatch. A returned error stops
    /// the loop, disposes queued work, and updates [`Self::last_failure`].
    pub fn tick(&mut self, block: bool) -> Result<LoopTick, FailureReport> {
        self.pump(PumpGoal::OneSlice, block)
    }
    /// Pumps, blocking when idle, until cooperative stop or a terminal failure.
    ///
    /// An idle loop stays alive until a stop request or failure; draining all jobs
    /// is not an implicit stop. A stopped loop returns immediately on later calls.
    pub fn run(&mut self) -> Result<(), FailureReport> {
        while !self.pump(PumpGoal::Continuous, true)?.stopped {}
        Ok(())
    }
    /// Finish a queued target and its checkpoint, preserving older FIFO work
    /// and leaving newer ordinary work queued. Cancellation is terminal too.
    ///
    /// May block for a future timer or resume a pending checkpoint across slices.
    /// For a repeating timer, `Completed` means one selected firing completed, not
    /// that the interval was removed. Missing, canceled, or previously finished
    /// local IDs return `Canceled`. Shutdown returns `Stopped`. A foreign loop ID
    /// returns `WrongOwner` without stopping this loop; pump failures stop it and
    /// are also retained by [`Self::last_failure`].
    pub fn run_until(&mut self, task: TaskId) -> Result<TurnOutcome, FailureReport> {
        let key = Key {
            slot: task.slot,
            generation: task.generation,
        };
        {
            let mut state = self.host.state.borrow_mut();
            if task.epoch != state.epoch {
                return Err(FailureReport::fault(
                    FailureCode::WrongOwner,
                    "task belongs to another loop",
                ));
            }
            if state.stopping {
                return Ok(TurnOutcome::Stopped);
            }
            if state.jobs.get(key).is_none() {
                return Ok(TurnOutcome::Canceled);
            }
            state.capture = Some((key, None));
            self.capture_failure = None;
        }
        loop {
            if let Err(error) = self.pump(PumpGoal::UntilTurn(task), true) {
                self.host.state.borrow_mut().capture = None;
                return Err(error);
            }
            let mut state = self.host.state.borrow_mut();
            if let Some((_, Some(outcome))) = state.capture
                && self.current.is_none()
            {
                state.capture = None;
                return self.capture_failure.take().map_or(Ok(outcome), Err);
            }
        }
    }
    fn pump(&mut self, goal: PumpGoal, block: bool) -> Result<LoopTick, FailureReport> {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let tick = self.slice(goal, block)?;
            // The turns of this slice queued responses and writes on socket tasks. They are
            // polled now, on every way out of the slice, so the bytes leave before the loop
            // does anything else; no reactor turn is needed to send.
            if tick.made_progress
                && let Some(reactor) = &self.owner_io
            {
                reactor.run_ready();
            }
            Ok::<LoopTick, FailureReport>(tick)
        }));
        match result {
            Ok(Ok(tick)) => Ok(tick),
            Ok(Err(report)) if report.code == FailureCode::Interrupted && self.stopping() => {
                // Native stop uses an uncatchable unwind, not an application or
                // engine fault. Other failures retain the existing fatal path.
                self.stopped_tick(true)
            }
            other => {
                let report = match other {
                    Ok(Err(report)) => report.bounded(),
                    _ => FailureReport::new(
                        FailureClass::Host,
                        FailureCode::HostPanic,
                        "owner callback panicked",
                    ),
                };
                let captured = self.current_is_capture();
                self.last_failure = Some(report.clone());
                self.dispose();
                let report = if let Some(sink) = self.sink.as_ref().filter(|_| !captured) {
                    if catch_unwind(AssertUnwindSafe(|| sink(&report))).is_err() {
                        self.sink = None;
                        FailureReport::new(
                            FailureClass::Host,
                            FailureCode::HostPanic,
                            "report sink panicked",
                        )
                    } else {
                        report
                    }
                } else {
                    report
                };
                self.last_failure = Some(report.clone());
                Err(report)
            }
        }
    }
    fn current_is_capture(&self) -> bool {
        matches!(self.current,Some(ActiveTurn::Job(key)) if self.host.state.borrow().capture.is_some_and(|(target,_)|target==key))
    }
    fn apply_failure(&mut self, report: FailureReport) -> Result<(), FailureReport> {
        let report = report.bounded();
        crate::trace::emit(
            crate::trace::Cat::Errors,
            "failure",
            crate::trace::failure(&report),
        );
        if report.class != FailureClass::Script || self.policy == ScriptErrorPolicy::Stop {
            return Err(report);
        }
        if self.current_is_capture() {
            self.capture_failure.get_or_insert(report);
        } else if let Some(sink) = self.sink.as_ref()
            && catch_unwind(AssertUnwindSafe(|| sink(&report))).is_err()
        {
            self.sink = None;
            return Err(FailureReport::new(
                FailureClass::Host,
                FailureCode::HostPanic,
                "report sink panicked",
            ));
        }
        Ok(())
    }
    fn checkpoint(&mut self) -> Result<bool, FailureReport> {
        self.driver.service_control()?;
        if self.stopping() {
            return Ok(true);
        }
        let mut report = self
            .driver
            .checkpoint(self.config.microtask_hint, &self.host)?;
        if let Some(index) = report
            .failures
            .iter()
            .position(|failure| failure.class != FailureClass::Script)
        {
            return Err(report.failures.remove(index).bounded());
        }
        let overflow = report.failures.len().saturating_sub(64) as u64;
        for failure in report.failures.into_iter().take(64) {
            self.apply_failure(failure)?;
        }
        let suppressed = report.suppressed_failures.saturating_add(overflow);
        if suppressed > 0 {
            let mut diagnostic = FailureReport::new(
                FailureClass::Script,
                FailureCode::ScriptException,
                &format!("{suppressed} additional checkpoint failures suppressed"),
            );
            diagnostic.truncated = true;
            self.apply_failure(diagnostic)?;
        }
        Ok(report.state == CheckpointState::Empty)
    }
    fn stopping(&self) -> bool {
        self.host.state.borrow().stopping || self.transport.stopped()
    }
    /// Services debug control, turns the reactor when one is due and imports transport
    /// deliveries into the ready queue.
    ///
    /// # Arguments
    /// * `block` - whether this slice may end in the idle wait
    ///
    /// # Returns
    /// Whether the reactor was turned.
    ///
    /// # Errors
    /// A control or transport fault, which stops the loop.
    fn maintenance(&mut self, block: bool) -> Result<bool, FailureReport> {
        self.driver.service_control()?;
        // Decision: a slice that may block skips its nonblocking reactor turn while the
        // last turn is younger than `REACTOR_SLACK`; its idle wait turns the reactor anyway.
        // The unconditional turn this replaces cost five kqueue calls per request at one
        // connection (12 microseconds each on macOS). A slice that may not block has no
        // idle wait and always turns it.
        //
        // Tried and dropped: a turn only after `task_batch` turns without one. Under a
        // burst of connects against a slow build, 64 turns outlasted the 128-entry listen
        // backlog and clients timed out in `connect`
        // (`http_pending_requests_are_bounded_and_dropped_on_disconnect`).
        let mut turned = false;
        if !self.stopping()
            && let Some(reactor) = &self.owner_io
            && (!block || self.reactor_at.elapsed() >= REACTOR_SLACK)
        {
            reactor.poll(Some(Duration::ZERO));
            self.reactor_at = Instant::now();
            turned = true;
        }
        if let Some(error) = self.transport.take_fault() {
            return Err(error);
        }
        self.transport.import(
            self.config.task_batch,
            &mut self.deliveries,
            &mut self.controls,
        );
        for task in self.controls.drain(..) {
            self.host.cancel(task);
        }
        self.transport_recent |= !self.deliveries.is_empty();
        let mut state = self.host.state.borrow_mut();
        for delivery in self.deliveries.drain(..) {
            let node = self.config.max_jobs
                + match delivery {
                    Delivery::Message(key) => key.slot as usize,
                    Delivery::Completion(key) => self.config.max_messages + key.slot as usize,
                };
            state.ready.push(node, ReadyItem::Delivery(delivery));
        }
        Ok(turned)
    }
    /// Moves up to one batch of owner-reactor socket events into the ready queue.
    fn import_owner_io(&mut self) {
        if let Some(reactor) = &self.owner_io {
            let mut state = self.host.state.borrow_mut();
            let offset =
                self.config.max_jobs + self.config.max_messages + self.config.max_operations;
            for _ in 0..self.config.task_batch {
                let Some(key) = reactor.next() else { break };
                state
                    .ready
                    .push(offset + key.slot as usize, ReadyItem::OwnerDelivery(key));
            }
        }
    }
    fn stopped_tick(&mut self, made_progress: bool) -> Result<LoopTick, FailureReport> {
        let first = !self.shutdown_complete;
        self.dispose();
        if first && let Some(error) = self.last_failure.as_ref() {
            return Err(error.clone());
        }
        Ok(LoopTick {
            made_progress,
            stopped: true,
        })
    }
    fn finish_active_turn(&mut self) -> Result<bool, FailureReport> {
        let Some(item) = self.current else {
            return Ok(true);
        };
        let empty = self.checkpoint()?;
        if self.stopping() {
            return Ok(true);
        }
        if !empty {
            return Ok(false);
        }
        match item {
            ActiveTurn::Job(key) => self.finish_turn(key)?,
            ActiveTurn::Delivery(delivery) => self.transport.finish(delivery),
            ActiveTurn::OwnerDelivery(key) => {
                if let Some(reactor) = &self.owner_io {
                    reactor.finish(key);
                }
            }
        };
        self.current = None;
        Ok(true)
    }
    fn capture_finished(&self, goal: PumpGoal) -> bool {
        matches!(goal, PumpGoal::UntilTurn(_))
            && self.current.is_none()
            && self
                .host
                .state
                .borrow()
                .capture
                .is_some_and(|(_, outcome)| outcome.is_some())
    }
    fn slice(&mut self, goal: PumpGoal, block: bool) -> Result<LoopTick, FailureReport> {
        let turned = self.maintenance(block)?;
        if self.stopping() {
            return self.stopped_tick(false);
        }
        self.import_owner_io();
        let now = self.host.clock.now();
        {
            let mut state = self.host.state.borrow_mut();
            while let Some(key) = state.timers.pop_due(now) {
                state.ready.push(key.slot as usize, ReadyItem::Job(key));
            }
        }
        let mut made_progress = false;
        if self.startup {
            let empty = self.checkpoint()?;
            if self.stopping() {
                return self.stopped_tick(true);
            }
            if !empty {
                return Ok(LoopTick {
                    made_progress: true,
                    stopped: false,
                });
            }
            self.startup = false;
        }
        // True when the batch ended on an empty ready queue; the stop check right before the
        // pop and the finished turn before it make the checks after the loop redundant then.
        let mut drained = false;
        for _ in 0..self.config.task_batch {
            if self.stopping() {
                return self.stopped_tick(made_progress);
            }
            if self.current.is_some() {
                made_progress = true;
                if !self.finish_active_turn()? {
                    return Ok(LoopTick {
                        made_progress: true,
                        stopped: false,
                    });
                }
                // Check again after a checkpoint AND the finished job's destructor:
                // both can issue controls through the shared owner-local host.
                // Fix: this check also ran right after the one at the top of the
                // iteration with nothing in between. Decision: only after a finished turn.
                if self.stopping() {
                    return self.stopped_tick(made_progress);
                }
            }
            if self.capture_finished(goal) {
                return Ok(LoopTick {
                    made_progress,
                    stopped: false,
                });
            }
            self.driver.service_control()?;
            if self.stopping() {
                return self.stopped_tick(made_progress);
            }
            let item = self.host.state.borrow_mut().ready.pop();
            let Some(item) = item else {
                drained = true;
                break;
            };
            self.current = match item {
                ReadyItem::Job(key) => Some(ActiveTurn::Job(key)),
                ReadyItem::Delivery(delivery) => Some(ActiveTurn::Delivery(delivery)),
                ReadyItem::OwnerDelivery(key) => Some(ActiveTurn::OwnerDelivery(key)),
            };
            made_progress = true;
            let traced = crate::trace::on(crate::trace::Cat::Loop).then(|| {
                let kind = match &item {
                    ReadyItem::Job(_) => "job",
                    ReadyItem::Delivery(Delivery::Message(_)) => "message",
                    ReadyItem::Delivery(Delivery::Completion(_)) => "completion",
                    ReadyItem::OwnerDelivery(_) => "socket",
                };
                (std::time::Instant::now(), kind)
            });
            match item {
                ReadyItem::Job(key) => {
                    let mut job = {
                        let mut state = self.host.state.borrow_mut();
                        let record = state.jobs.get_mut(key).expect("ready key is live");
                        record.running = true;
                        record.job.take().expect("not dispatched twice")
                    };
                    let result = self.driver.execute_job(&mut job, &self.host);
                    self.host
                        .state
                        .borrow_mut()
                        .jobs
                        .get_mut(key)
                        .expect("running marker survives cancellation")
                        .job = Some(job);
                    if let Err(error) = result {
                        self.apply_failure(error)?;
                    }
                }
                ReadyItem::Delivery(Delivery::Message(key)) => {
                    let message = self.transport.message(key).ok_or_else(|| {
                        FailureReport::fault(FailureCode::InvalidHandle, "stale ready message")
                    })?;
                    if let Err(error) = self.driver.deliver_message(message, &self.host) {
                        self.apply_failure(error)?;
                    }
                }
                ReadyItem::Delivery(Delivery::Completion(key)) => {
                    let (id, payload) = self.transport.completion(key).ok_or_else(|| {
                        FailureReport::fault(FailureCode::InvalidHandle, "stale ready completion")
                    })?;
                    if let Err(error) = self.driver.deliver_completion(id, payload, &self.host) {
                        self.apply_failure(error)?;
                    }
                }
                ReadyItem::OwnerDelivery(key) => {
                    let message = self
                        .owner_io
                        .as_ref()
                        .and_then(|reactor| reactor.take(key))
                        .ok_or_else(|| {
                            FailureReport::fault(
                                FailureCode::InvalidHandle,
                                "stale owner I/O message",
                            )
                        })?;
                    if let Err(error) = self.driver.deliver_message(message, &self.host) {
                        self.apply_failure(error)?;
                    }
                }
            }
            if let Some((start, kind)) = traced {
                crate::trace::emit(
                    crate::trace::Cat::Loop,
                    "turn",
                    serde_json::json!({"kind": kind, "ms": crate::trace::ms(start)}),
                );
            }
        }
        // Fix: after the loop, `stopping()` ran twice with at most a finished turn between, and
        // once more right after the loop broke on an empty queue. Decision: skip the check
        // when the loop drained (current is `None` then), and the second one when no turn ran.
        if !drained {
            if self.stopping() {
                return self.stopped_tick(made_progress);
            }
            if self.current.is_some() {
                made_progress = true;
                if !self.finish_active_turn()? {
                    return Ok(LoopTick {
                        made_progress,
                        stopped: false,
                    });
                }
                if self.stopping() {
                    return self.stopped_tick(made_progress);
                }
            }
        }
        if self.capture_finished(goal) {
            return Ok(LoopTick {
                made_progress,
                stopped: false,
            });
        }
        let runnable = self.host.state.borrow().ready.len() != 0
            || self.driver.has_control_work()
            || self
                .owner_io
                .as_ref()
                .is_some_and(|reactor| reactor.has_ready());
        let mut published = false;
        if block && !runnable && !made_progress && std::mem::take(&mut self.transport_recent) {
            let start = Instant::now();
            while !published && start.elapsed() < OWNER_SPIN {
                published = self.transport.published();
                std::hint::spin_loop();
            }
        }
        let timeout = if !block || runnable || made_progress || published || !self.transport.arm()
        {
            Some(Duration::ZERO)
        } else if self.driver.has_control_work() {
            // Debug control has its own mailbox, but shares this OS wake gate.
            // A publication before arm may have coalesced its wake while Awake;
            // a publication after this recheck observes Armed and wakes poll.
            self.transport.awake();
            Some(Duration::ZERO)
        } else {
            let now = self.host.clock.now();
            self.host
                .state
                .borrow()
                .timers
                .next()
                .map(|deadline| deadline.saturating_duration_since(now))
        };
        if timeout != Some(Duration::ZERO) {
            // The single idle wait: the reactor sleeps in kqueue/epoll until a socket
            // task publishes, a producer thread wakes the transport, or the next JS
            // timer is due. Socket tasks that only progress internally run inside it.
            let waited = crate::trace::on(crate::trace::Cat::Loop).then(std::time::Instant::now);
            if let Some(reactor) = &self.owner_io {
                reactor.poll(timeout);
                self.reactor_at = Instant::now();
            }
            self.transport.awake();
            if let Some(start) = waited {
                crate::trace::emit(
                    crate::trace::Cat::Loop,
                    "wait",
                    serde_json::json!({
                        "ms": crate::trace::ms(start),
                        "until": timeout.map_or("woken".to_owned(), |t| format!("timer in {:.3}ms", t.as_secs_f64() * 1e3)),
                    }),
                );
            }
        } else {
            // Runnable work will be observed at the next slice. No syscall is
            // needed merely to recheck already-authoritative owner queues.
            self.transport.awake();
            // A slice that ran no turn and does not wait still turns the reactor once.
            // Without it a loop kept from waiting by pending debug control or a refused
            // arm would spin through slices and never read a socket.
            if !made_progress
                && !turned
                && let Some(reactor) = &self.owner_io
            {
                reactor.poll(Some(Duration::ZERO));
                self.reactor_at = Instant::now();
            }
        }
        Ok(LoopTick {
            made_progress,
            stopped: false,
        })
    }
    fn finish_turn(&mut self, key: Key) -> Result<(), FailureReport> {
        let now = self.host.clock.now();
        let removed = {
            let mut state = self.host.state.borrow_mut();
            if let Some((target, outcome)) = &mut state.capture
                && *target == key
            {
                *outcome = Some(TurnOutcome::Completed);
            }
            let record = state.jobs.get_mut(key).expect("current turn record");
            if let Some(mut schedule) = record.schedule.filter(|schedule| schedule.period.is_some())
                && !record.canceled
            {
                schedule.deadline = next_period(schedule, now).ok_or_else(|| {
                    FailureReport::fault(FailureCode::Capacity, "interval deadline exhausted")
                })?;
                record.schedule = Some(schedule);
                record.running = false;
                state.timers.insert(key, schedule.deadline).map_err(|_| {
                    FailureReport::fault(FailureCode::Capacity, "timer sequence exhausted")
                })?;
                None
            } else {
                // Fix: unref marks were pruned by scanning. Decision: remove the mark here.
                state.remove_job(key)
            }
        };
        drop(removed);
        Ok(())
    }
    fn dispose(&mut self) {
        if self.shutdown_complete {
            return;
        }
        self.transport.close();
        if let Some(error) = self.transport.take_fault() {
            self.last_failure.get_or_insert(error);
        }
        {
            let mut state = self.host.state.borrow_mut();
            state.stopping = true;
            if let Some((_, outcome)) = &mut state.capture
                && outcome.is_none()
            {
                *outcome = Some(TurnOutcome::Stopped);
            }
        }
        if let Some(reactor) = self.owner_io.take()
            && catch_unwind(AssertUnwindSafe(|| reactor.close())).is_err()
        {
            self.last_failure.get_or_insert_with(|| {
                FailureReport::new(
                    FailureClass::Host,
                    FailureCode::HostPanic,
                    "owner socket reactor cleanup panicked",
                )
            });
        }
        loop {
            let removed = {
                let mut state = self.host.state.borrow_mut();
                let key = state.jobs.keys().next();
                key.and_then(|key| {
                    state.ready.remove(key.slot as usize);
                    state.timers.remove(key);
                    state.remove_job(key)
                })
            };
            if removed.is_none() {
                break;
            }
            if catch_unwind(AssertUnwindSafe(|| drop(removed))).is_err() {
                self.last_failure.get_or_insert_with(|| {
                    FailureReport::new(
                        FailureClass::Host,
                        FailureCode::HostPanic,
                        "job destructor panicked",
                    )
                });
            }
        }
        // Shared transport closure already disposed delivery ownership. Unlink
        // imported scalar notifications as well as job/source nodes before reuse.
        while self.host.state.borrow_mut().ready.pop().is_some() {}
        self.current = None;
        match catch_unwind(AssertUnwindSafe(|| self.driver.shutdown())) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.last_failure.get_or_insert(error.bounded());
            }
            Err(_) => {
                self.last_failure.get_or_insert_with(|| {
                    FailureReport::new(
                        FailureClass::Host,
                        FailureCode::HostPanic,
                        "driver shutdown panicked",
                    )
                });
            }
        }
        self.shutdown_complete = true;
    }
}
impl<D: LoopDriver> Drop for EventLoop<D> {
    fn drop(&mut self) {
        self.dispose();
    }
}
