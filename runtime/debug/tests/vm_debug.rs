//! ExistingCallables integration over Rust host-call frames and the real VM/debug
//! transports. There is no JavaScript parser, native GC, or engine qualification.
mod support;
use coldmind_runtime::{debug::*, engine::*, vm::*};
use std::{
    cell::{Cell, RefCell},
    num::NonZeroU64,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use support::{FixtureProbe, Inline, RustFixture};
struct Signal {
    calls: AtomicUsize,
    revoked: AtomicBool,
    after_revoke: AtomicUsize,
}
impl DebugSignal for Signal {
    fn wake(&self) -> Result<(), DebugError> {
        if self.revoked.load(Ordering::Acquire) {
            self.after_revoke.fetch_add(1, Ordering::SeqCst);
            return Err(DebugError::Closed);
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
struct Backend {
    descriptor: DebugDescriptor,
    port: RefCell<Option<Rc<dyn DebugPort>>>,
    session: Cell<Option<DebugSessionId>>,
    observer: Cell<bool>,
    panic_after_detach: Cell<bool>,
    signal: Arc<Signal>,
    stopped: Arc<AtomicBool>,
}
impl Backend {
    fn new() -> Self {
        Self {
            descriptor: DebugDescriptor {
                api_version: 1,
                capabilities: DebugCapabilities::baseline(),
                native_requirements: vec![
                    "Rust control fixture only; no native JS qualification".to_owned(),
                ],
                interrupt_mechanism: "Rust Condvar mailbox plus revocable atomic signal".to_owned(),
            },
            port: RefCell::new(None),
            session: Cell::new(None),
            observer: Cell::new(false),
            panic_after_detach: Cell::new(false),
            signal: Arc::new(Signal {
                calls: AtomicUsize::new(0),
                revoked: AtomicBool::new(false),
                after_revoke: AtomicUsize::new(0),
            }),
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }
    fn port(&self) -> Rc<dyn DebugPort> {
        self.port.borrow().as_ref().unwrap().clone()
    }
    fn dispatch(&self, directive: DebugDirective) -> Result<Option<bool>, DebugError> {
        let port = self.port();
        match directive {
            DebugDirective::Continue { pause, .. } => {
                port.identities().end_pause(pause)?;
                port.publish(DebugUpdate::Resumed {
                    sequence: port.identities().next_update_sequence()?,
                    previous: pause,
                })?;
                Ok(Some(false))
            }
            DebugDirective::Stop => {
                self.stopped.store(true, Ordering::Release);
                self.detach()?;
                Ok(Some(true))
            }
            DebugDirective::Detach => {
                self.detach()?;
                Ok(Some(false))
            }
            DebugDirective::Pause => {
                self.pause_loop()?;
                Ok(None)
            }
            DebugDirective::Request(command) => {
                let result = match command.request {
                    DebugRequest::Stack { pause, .. } => {
                        let frame = match port.identities().register(
                            pause,
                            ReferenceKind::Frame,
                            NonZeroU64::new(1).unwrap(),
                        )? {
                            DebugReference::Frame(frame) => frame,
                            _ => unreachable!(),
                        };
                        Ok(DebugReply::Stack {
                            frames: vec![DebugFrame {
                                id: frame,
                                name: JsString::from_utf8("active Rust host frame"),
                                generated: None,
                                original: None,
                            }],
                            total: 1,
                        })
                    }
                    _ => Err(DebugError::Unsupported),
                };
                port.publish(DebugUpdate::Reply {
                    id: command.id,
                    result,
                })?;
                Ok(None)
            }
        }
    }
    fn pause_loop(&self) -> Result<bool, DebugError> {
        let port = self.port();
        let pause = port
            .identities()
            .begin_pause(self.session.get().ok_or(DebugError::NotAttached)?)?;
        port.publish(DebugUpdate::Paused {
            sequence: port.identities().next_update_sequence()?,
            pause,
            reason: PauseReason::Requested,
            location: None,
            original: None,
            breakpoint_ids: vec![],
        })?;
        loop {
            let Some(directive) = port.next_directive(true) else {
                return Err(DebugError::Closed);
            };
            if let Some(stopped) = self.dispatch(directive)? {
                return Ok(stopped);
            }
        }
    }
}
impl DebugBackend for Backend {
    fn descriptor(&self) -> &DebugDescriptor {
        &self.descriptor
    }
    fn initialize(&self, port: Rc<dyn DebugPort>) -> Result<(), DebugError> {
        *self.port.borrow_mut() = Some(port);
        Ok(())
    }
    fn attach(&self, options: DebugAttachOptions) -> Result<Arc<dyn DebugSignal>, DebugError> {
        self.session.set(Some(options.session));
        self.observer.set(true);
        self.signal.revoked.store(false, Ordering::Release);
        Ok(self.signal.clone())
    }
    fn service(&self) -> Result<DebugServiceReport, DebugError> {
        let port = self.port();
        let mut made_progress = false;
        while let Some(directive) = port.next_directive(false) {
            made_progress = true;
            self.dispatch(directive)?;
        }
        Ok(DebugServiceReport {
            made_progress,
            state: port.identities().state(),
        })
    }
    fn detach(&self) -> Result<(), DebugError> {
        if let Some(session) = self.session.take() {
            let port = self.port();
            if let DebugExecutionState::Paused(pause) = port.identities().state() {
                port.identities().end_pause(pause)?;
            }
            port.identities().close_session(session)?;
            port.publish(DebugUpdate::SessionClosed {
                session,
                reason: None,
            })?;
        }
        self.observer.set(false);
        self.signal.revoked.store(true, Ordering::Release);
        assert!(
            !self.panic_after_detach.replace(false),
            "native detach reporting panic"
        );
        Ok(())
    }
}
struct Engine {
    inner: RustFixture<Inline>,
    backend: Rc<Backend>,
}
impl JsEngine for Engine {
    fn descriptor(&self) -> &EngineDescriptor {
        self.inner.descriptor()
    }
    fn create_context(&self, limits: ContextLimits) -> Result<ContextId, FailureReport> {
        self.inner.create_context(limits)
    }
    fn close_context(&self, context: ContextId) -> Result<(), FailureReport> {
        assert!(
            !self.backend.observer.get(),
            "observers must close before native resources"
        );
        self.inner.close_context(context)
    }
    fn debug_backend(
        &self,
        context: ContextId,
    ) -> Result<Option<Rc<dyn DebugBackend>>, FailureReport> {
        self.inner.debug_backend(context)?;
        Ok(Some(self.backend.clone()))
    }
    fn with_scope(
        &self,
        context: ContextId,
        host: &dyn HostServices,
        body: &mut dyn ScopeCall,
    ) -> Result<(), FailureReport> {
        self.inner.with_scope(context, host, body)
    }
}
struct Tick(Arc<AtomicUsize>);
impl HostFunction for Tick {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        call.import_data(JsData::Undefined)
    }
}
struct PauseCall {
    backend: Rc<Backend>,
    tick: RootedValue,
    probe: FixtureProbe<Inline>,
    checkpoint_at_pause: Arc<AtomicUsize>,
}
impl HostFunction for PauseCall {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let tick = call.retain(call.local(&self.tick)?)?;
        call.host()
            .post(CallbackJob {
                function: tick,
                receiver: None,
                arguments: vec![],
            })
            .map_err(|_| {
                FailureReport::fault(FailureCode::Capacity, "fixture ordinary callback rejected")
            })?;
        self.checkpoint_at_pause
            .store(self.probe.checkpoint_calls(), Ordering::Release);
        let stopped = self.backend.pause_loop().map_err(|error| {
            FailureReport::fault(FailureCode::EngineFailure, &error.to_string())
        })?;
        if stopped {
            return Err(FailureReport::fault(
                FailureCode::ClosedContext,
                "Rust host frame unwound after VM stop",
            )
            .into());
        }
        call.import_data(JsData::Number(71.0))
    }
}
type Fixture = (
    VirtualMachine<Engine>,
    Rc<Backend>,
    FixtureProbe<Inline>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    RootedValue,
);
fn fixture() -> Fixture {
    fixture_with_handler(None)
}
fn fixture_with_handler(handler: Option<Rc<dyn HostMessageHandler>>) -> Fixture {
    let inner = RustFixture::<Inline>::new("vm-debug-rust-control-fixture");
    let probe = inner.probe();
    let backend = Rc::new(Backend::new());
    let mut config = VmConfig::default();
    config.requirements.entry = EntryRequirement::ExistingCallables;
    config.debug_limits.requests = 1;
    config.message_handler = handler;
    let mut vm = VirtualMachine::new(
        Engine {
            inner,
            backend: backend.clone(),
        },
        config,
    )
    .unwrap();
    let ordinary = Arc::new(AtomicUsize::new(0));
    let checkpoints = Arc::new(AtomicUsize::new(0));
    let tick = vm
        .bind_function(
            JsString::from_utf8("tick"),
            0,
            Rc::new(Tick(ordinary.clone())),
        )
        .unwrap();
    let pause = vm
        .bind_function(
            JsString::from_utf8("pause"),
            0,
            Rc::new(PauseCall {
                backend: backend.clone(),
                tick,
                probe: probe.clone(),
                checkpoint_at_pause: checkpoints.clone(),
            }),
        )
        .unwrap();
    (vm, backend, probe, ordinary, checkpoints, pause)
}
fn wait_pause(handle: &VmDebugHandle) -> PauseId {
    loop {
        match handle.wait_update().expect("active session") {
            DebugUpdate::Paused { pause, .. } => return pause,
            DebugUpdate::SessionClosed { .. } => panic!("closed before pause"),
            _ => {}
        }
    }
}
#[test]
fn synchronous_vm_call_stays_on_its_owner_frame_until_external_debug_resume() {
    let (mut vm, backend, probe, ordinary, checkpoints, function) = fixture();
    assert_eq!(vm.state(), VmState::Ready);
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let observed = ordinary.clone();
    let before = checkpoints.clone();
    let controller = std::thread::spawn(move || {
        let pause = wait_pause(&debug);
        assert_eq!(observed.load(Ordering::SeqCst), 0);
        assert!(before.load(Ordering::Acquire) > 0);
        let id = match debug
            .try_request(DebugRequest::Stack {
                pause,
                page: Page { start: 0, count: 4 },
            })
            .unwrap()
        {
            DebugRequestReceipt::Accepted { id }
            | DebugRequestReceipt::AcceptedWakeFailed { id, .. } => id,
        };
        loop {
            if let DebugUpdate::Reply { id: actual, result } = debug.wait_update().unwrap() {
                assert_eq!(actual, id);
                match result.unwrap() {
                    DebugReply::Stack { frames, .. } => {
                        assert_eq!(frames[0].name.to_utf8().unwrap(), "active Rust host frame")
                    }
                    _ => panic!(),
                };
                break;
            }
        }
        assert_eq!(observed.load(Ordering::SeqCst), 0);
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    let result = vm.call(&function, None, &[]).unwrap();
    let debug = controller.join().unwrap();
    assert_eq!(
        ordinary.load(Ordering::SeqCst),
        0,
        "ordinary callback stayed queued throughout paused call"
    );
    assert_eq!(vm.debug_state(), DebugExecutionState::Running);
    assert!(probe.checkpoint_calls() > checkpoints.load(Ordering::Acquire));
    vm.tick(false).unwrap();
    assert_eq!(ordinary.load(Ordering::SeqCst), 1);
    drop(result);
    vm.shutdown().unwrap();
    assert!(!backend.observer.get());
    assert_eq!(probe.contexts_closed(), 1);
    assert_eq!(debug.state(), DebugExecutionState::Closed);
    let calls = backend.signal.calls.load(Ordering::SeqCst);
    assert_eq!(debug.request_pause().unwrap_err(), DebugError::Closed);
    assert_eq!(backend.signal.calls.load(Ordering::SeqCst), calls);
    assert_eq!(backend.signal.after_revoke.load(Ordering::SeqCst), 0);
}
#[test]
fn vm_stop_interrupts_the_restricted_pause_lane_when_request_permits_are_full() {
    let (mut vm, backend, probe, ordinary, _checkpoints, function) = fixture();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let stop = vm.handle();
    let stopped = backend.stopped.clone();
    let observed = ordinary.clone();
    let controller = std::thread::spawn(move || {
        let pause = wait_pause(&debug);
        debug
            .try_request(DebugRequest::Stack {
                pause,
                page: Page { start: 0, count: 4 },
            })
            .unwrap();
        assert!(
            debug
                .try_request(DebugRequest::Stack {
                    pause,
                    page: Page { start: 0, count: 1 }
                })
                .is_err()
        );
        stop.request_stop().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !stopped.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        if !stopped.load(Ordering::Acquire) {
            let _ = debug.detach();
            panic!("VM stop did not reach the native restricted pump")
        }
        assert_eq!(observed.load(Ordering::SeqCst), 0);
        debug
    });
    assert!(vm.call(&function, None, &[]).is_err());
    let debug = controller.join().unwrap();
    assert_eq!(ordinary.load(Ordering::SeqCst), 0);
    vm.shutdown().unwrap();
    assert_eq!(probe.contexts_closed(), 1);
    assert!(!backend.observer.get());
    assert_eq!(backend.signal.after_revoke.load(Ordering::SeqCst), 0);
    assert!(matches!(
        debug.state(),
        DebugExecutionState::Closed | DebugExecutionState::Detached
    ));
}

struct MessageCounter(Arc<AtomicUsize>);
impl HostMessageHandler for MessageCounter {
    fn deliver<'s>(
        &self,
        _scope: &'s dyn JsScope,
        _host: &dyn HostServices,
        _payload: JsData,
    ) -> JsResult<'s, ()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn dap_wire(seq: u64, command: &str) -> Vec<u8> {
    let body=serde_json::to_vec(&serde_json::json!({"seq":seq,"type":"request","command":command,"arguments":{"threadId":1}})).unwrap();
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend(body);
    frame
}
fn is_stopped(frame: &[u8]) -> bool {
    let offset = frame
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .unwrap()
        + 4;
    serde_json::from_slice::<serde_json::Value>(&frame[offset..]).unwrap()["event"] == "stopped"
}
#[test]
fn dap_configuration_gate_blocks_real_vm_messages_and_calls_until_configuration_done() {
    let messages = Arc::new(AtomicUsize::new(0));
    let (mut vm, _backend, _probe, ordinary, entered, function) =
        fixture_with_handler(Some(Rc::new(MessageCounter(messages.clone()))));
    let controller = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let mut dap = DapAdapter::new(controller).unwrap();
    vm.handle().try_send(JsData::Null).unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let observed_messages = messages.clone();
    let observed_ordinary = ordinary.clone();
    let worker = std::thread::spawn(move || {
        dap.receive(&dap_wire(1, "initialize")).unwrap();
        ready_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_millis(50);
        while entered.load(Ordering::Acquire) == 0
            && observed_messages.load(Ordering::SeqCst) == 0
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        let entered_early = entered.load(Ordering::Acquire) != 0
            || observed_messages.load(Ordering::SeqCst) != 0
            || observed_ordinary.load(Ordering::SeqCst) != 0;
        let mut output = dap.receive(&dap_wire(2, "configurationDone")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !output.iter().any(|frame| is_stopped(frame)) && Instant::now() < deadline {
            output.extend(dap.poll().unwrap());
            std::thread::sleep(Duration::from_millis(1));
        }
        if !output.iter().any(|frame| is_stopped(frame)) {
            let _ = dap.receive(&dap_wire(4, "disconnect"));
            panic!("Rust callback did not reach its real pause pump after configuration")
        }
        dap.receive(&dap_wire(3, "continue")).unwrap();
        (dap, entered_early)
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let result = vm.call(&function, None, &[]).unwrap();
    let (_dap, entered_early) = worker.join().unwrap();
    assert!(
        !entered_early,
        "application engine/message entry occurred before DAP configurationDone"
    );
    assert_eq!(
        messages.load(Ordering::SeqCst),
        0,
        "run_until leaves work queued after its target"
    );
    assert_eq!(ordinary.load(Ordering::SeqCst), 0);
    for _ in 0..4 {
        if messages.load(Ordering::SeqCst) == 1 && ordinary.load(Ordering::SeqCst) == 1 {
            break;
        }
        vm.tick(false).unwrap();
    }
    assert_eq!(messages.load(Ordering::SeqCst), 1);
    assert_eq!(ordinary.load(Ordering::SeqCst), 1);
    drop(result);
    vm.shutdown().unwrap();
}

#[test]
fn native_detach_panic_does_not_skip_vm_context_close() {
    let (mut vm, backend, probe, _ordinary, _checkpoints, _function) = fixture();
    let handle = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    backend.panic_after_detach.set(true);
    assert!(
        matches!(vm.shutdown(),Err(VmError::Execution(ref report)) if report.code==FailureCode::HostPanic)
    );
    assert_eq!(probe.contexts_closed(), 1);
    assert!(!backend.observer.get());
    assert_eq!(
        backend.port().identities().state(),
        DebugExecutionState::Closed
    );
    assert_eq!(handle.state(), DebugExecutionState::Closed);
    assert_eq!(backend.signal.after_revoke.load(Ordering::SeqCst), 0);
    vm.shutdown().unwrap();
}
