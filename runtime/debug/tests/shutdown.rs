//! Real owner teardown, destructor containment, and stale producer isolation.
mod support;
use coldmind_runtime::{debug::DebugRequirements, engine::*, scheduler::*, vm::*};
use std::{
    cell::RefCell,
    num::NonZeroUsize,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
type Log = Arc<Mutex<Vec<&'static str>>>;
struct Dropped {
    label: &'static str,
    log: Log,
    panic: bool,
}
impl Drop for Dropped {
    fn drop(&mut self) {
        self.log.lock().unwrap().push(self.label);
        assert!(!self.panic, "deliberate destructor panic");
    }
}
struct Driver {
    log: Log,
    closes: Arc<AtomicUsize>,
    panic_close: bool,
}
impl Drop for Driver {
    fn drop(&mut self) {
        self.log.lock().unwrap().push("driver.drop");
    }
}
impl LoopDriver for Driver {
    type Job = Dropped;
    type Message = Dropped;
    fn execute_job(
        &mut self,
        _: &mut Dropped,
        _: &dyn LocalHost<Dropped>,
    ) -> Result<(), FailureReport> {
        panic!("shutdown must not execute jobs")
    }
    fn deliver_message(
        &mut self,
        _: Dropped,
        _: &dyn LocalHost<Dropped>,
    ) -> Result<(), FailureReport> {
        panic!("shutdown must not deliver messages")
    }
    fn deliver_completion(
        &mut self,
        _: OperationId,
        _: CompletionPayload,
        _: &dyn LocalHost<Dropped>,
    ) -> Result<(), FailureReport> {
        panic!("shutdown must not deliver completions")
    }
    fn checkpoint(
        &mut self,
        _: NonZeroUsize,
        _: &dyn LocalHost<Dropped>,
    ) -> Result<CheckpointReport, FailureReport> {
        panic!("shutdown must not run checkpoints")
    }
    fn shutdown(&mut self) -> Result<(), FailureReport> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        self.log.lock().unwrap().push("driver.close");
        assert!(!self.panic_close, "driver shutdown panic");
        Ok(())
    }
}
fn runtime(log: &Log, panic_close: bool) -> (EventLoop<Driver>, Arc<AtomicUsize>) {
    let closes = Arc::new(AtomicUsize::new(0));
    (
        EventLoop::new(
            Driver {
                log: log.clone(),
                closes: closes.clone(),
                panic_close,
            },
            SchedulerConfig {
                max_jobs: 2,
                max_messages: 2,
                ..Default::default()
            },
            None,
        )
        .unwrap(),
        closes,
    )
}
fn dropped(label: &'static str, log: &Log, panic: bool) -> Dropped {
    Dropped {
        label,
        log: log.clone(),
        panic,
    }
}
fn exactly_once(log: &[&str], label: &str) {
    assert_eq!(
        log.iter().filter(|entry| **entry == label).count(),
        1,
        "{label}: {log:?}"
    );
}

#[test]
fn cleanup_releases_messages_and_jobs_before_driver_close_and_drop() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (mut runtime, closes) = runtime(&log, false);
    runtime
        .handle()
        .try_send(dropped("message", &log, false))
        .ok()
        .unwrap();
    runtime
        .host()
        .post(dropped("job", &log, false))
        .ok()
        .unwrap();
    runtime.shutdown().unwrap();
    runtime.shutdown().unwrap();
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.stats().live_jobs, 0);
    assert_eq!(runtime.stats().ready_nodes, 0);
    drop(runtime);
    let log = log.lock().unwrap();
    for label in ["message", "job", "driver.close", "driver.drop"] {
        exactly_once(&log, label);
    }
    let at = |label| log.iter().position(|entry| *entry == label).unwrap();
    assert!(
        at("message") < at("job")
            && at("job") < at("driver.close")
            && at("driver.close") < at("driver.drop")
    );
}

#[test]
fn job_destructor_and_driver_shutdown_panics_are_contained_without_retry() {
    for driver_panic in [false, true] {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (mut runtime, closes) = runtime(&log, driver_panic);
        runtime
            .host()
            .post(dropped("first-job", &log, !driver_panic))
            .ok()
            .unwrap();
        runtime
            .host()
            .post(dropped("second-job", &log, false))
            .ok()
            .unwrap();
        let error = runtime.shutdown().unwrap_err();
        assert_eq!(error.code, FailureCode::HostPanic);
        runtime.shutdown().unwrap();
        assert_eq!(runtime.last_failure(), Some(&error));
        assert_eq!(closes.load(Ordering::SeqCst), 1);
        drop(runtime);
        let log = log.lock().unwrap();
        for label in ["first-job", "second-job", "driver.close", "driver.drop"] {
            exactly_once(&log, label);
        }
    }
}

#[test]
fn message_destructor_panic_does_not_escape_or_skip_later_owner_cleanup() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (mut runtime, closes) = runtime(&log, false);
    runtime
        .handle()
        .try_send(dropped("bad-message", &log, true))
        .ok()
        .unwrap();
    runtime
        .handle()
        .try_send(dropped("good-message", &log, false))
        .ok()
        .unwrap();
    runtime
        .host()
        .post(dropped("job", &log, false))
        .ok()
        .unwrap();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.shutdown()));
    assert!(
        outcome.is_ok(),
        "a queued message destructor must become a detached HostPanic"
    );
    assert_eq!(outcome.unwrap().unwrap_err().code, FailureCode::HostPanic);
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    for label in ["bad-message", "good-message", "job", "driver.close"] {
        exactly_once(&log.lock().unwrap(), label);
    }
}

fn config() -> VmConfig {
    VmConfig {
        requirements: EngineRequirements {
            entry: EntryRequirement::ExistingCallables,
            debug: DebugRequirements::disabled(),
            ..Default::default()
        },
        message_handler: Some(Rc::new(IgnoreMessages)),
        ..Default::default()
    }
}
struct IgnoreMessages;
impl HostMessageHandler for IgnoreMessages {
    fn deliver<'s>(&self, _: &'s dyn JsScope, _: &dyn HostServices, _: JsData) -> JsResult<'s, ()> {
        Ok(())
    }
}
struct Start(Rc<RefCell<Option<CompletionTicket>>>);
impl HostFunction for Start {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let deferred = call.deferred()?;
        let reservation = call.host().reserve_operation(4096)?;
        *self.0.borrow_mut() = Some(call.host().bind_deferred(
            reservation,
            deferred.resolver,
            None,
        )?);
        Ok(deferred.promise)
    }
}
fn pending(
    vm: &mut VirtualMachine<support::RustFixture<support::Inline>>,
) -> (RootedValue, CompletionTicket) {
    let output = Rc::new(RefCell::new(None));
    let function = vm
        .bind_function(
            JsString::from_utf8("start"),
            0,
            Rc::new(Start(output.clone())),
        )
        .unwrap();
    let promise = vm.call(&function, None, &[]).unwrap();
    let ticket = output.borrow_mut().take().unwrap();
    (promise, ticket)
}

#[test]
fn old_handles_tickets_and_roots_cannot_reach_a_replacement_vm() {
    let fixture = support::RustFixture::<support::Inline>::new("old-session");
    let old_probe = fixture.probe();
    let mut old = VirtualMachine::new(fixture, config()).unwrap();
    let handle = old.handle();
    let (old_promise, old_ticket) = pending(&mut old);
    old.shutdown().unwrap();
    old.shutdown().unwrap();
    assert_eq!(old_probe.contexts_closed(), 1);
    drop(old);
    let fixture = support::RustFixture::<support::Inline>::new("new-session");
    let probe = fixture.probe();
    let mut new = VirtualMachine::new(fixture, config()).unwrap();
    let (promise, ticket) = pending(&mut new);
    assert!(matches!(
        old_ticket.complete(CompletionPayload::Success(JsData::Number(99.0))),
        PublishOutcome::Rejected {
            reason: PublishRejection::Closed | PublishRejection::Stale,
            ..
        }
    ));
    assert_eq!(
        handle.try_send(JsData::Number(99.0)).unwrap_err().reason,
        RejectionReason::Closed
    );
    handle.request_stop().unwrap();
    assert_eq!(new.state(), VmState::Ready);
    assert_eq!(probe.promise(&promise).unwrap(), None);
    assert_eq!(
        new.call(&old_promise, None, &[]).unwrap_err(),
        VmError::InvalidHandle
    );
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Number(7.0))),
        PublishOutcome::Published
    ));
    new.tick(false).unwrap();
    assert_eq!(
        probe.promise(&promise).unwrap(),
        Some((false, JsData::Number(7.0)))
    );
    assert_eq!(
        old_probe.promise(&old_promise).unwrap_err().code,
        FailureCode::ClosedContext
    );
    drop(old_promise);
    new.shutdown().unwrap();
    drop(new);
    assert_eq!(probe.contexts_closed(), 1);
}

#[test]
fn owner_drop_contains_queued_destructor_panics_and_finishes_cleanup_once() {
    // The case of a registered native source went with the Mio backend (commit e9228d4,
    // `.archive/2026-11-18/src/io`); that commit removed its helper `register_source` and left
    // the call, so this file did not compile. The two queues that still exist are covered.
    for kind in 0..2 {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (runtime, closes) = runtime(&log, false);
        match kind {
            0 => {
                runtime
                    .handle()
                    .try_send(dropped("bad", &log, true))
                    .ok()
                    .unwrap();
            }
            _ => {
                runtime
                    .host()
                    .post(dropped("bad", &log, true))
                    .ok()
                    .unwrap();
            }
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(runtime)));
        assert!(
            outcome.is_ok(),
            "best-effort owner Drop must contain queued resource destructor panics"
        );
        assert_eq!(closes.load(Ordering::SeqCst), 1);
        let log = log.lock().unwrap();
        for label in ["bad", "driver.close", "driver.drop"] {
            exactly_once(&log, label);
        }
    }
}

struct TrackedFunction {
    drops: Rc<std::cell::Cell<usize>>,
    closed_when_dropped: Rc<std::cell::Cell<usize>>,
    probe: support::FixtureProbe<support::Inline>,
}
impl HostFunction for TrackedFunction {
    fn invoke<'s>(&self, _: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        panic!("shutdown must not enter a host callable")
    }
}
impl Drop for TrackedFunction {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
        self.closed_when_dropped.set(self.probe.contexts_closed());
    }
}

#[test]
fn external_root_does_not_keep_callable_registration_alive_after_context_close() {
    let fixture = support::RustFixture::<support::Inline>::new("external-root-close");
    let probe = fixture.probe();
    let mut vm = VirtualMachine::new(fixture, config()).unwrap();
    let drops = Rc::new(std::cell::Cell::new(0));
    let closed_when_dropped = Rc::new(std::cell::Cell::new(usize::MAX));
    let root = vm
        .bind_function(
            JsString::from_utf8("retained"),
            0,
            Rc::new(TrackedFunction {
                drops: drops.clone(),
                closed_when_dropped: closed_when_dropped.clone(),
                probe: probe.clone(),
            }),
        )
        .unwrap();
    assert_eq!(drops.get(), 0);
    vm.shutdown().unwrap();
    assert_eq!(drops.get(), 1);
    assert_eq!(
        closed_when_dropped.get(),
        0,
        "native callable is disposed before context close completes"
    );
    assert_eq!(probe.contexts_closed(), 1);
    assert_eq!(vm.state(), VmState::Stopped);
    drop(root);
    vm.shutdown().unwrap();
    drop(vm);
    assert_eq!(drops.get(), 1);
    assert_eq!(probe.contexts_closed(), 1);
}
