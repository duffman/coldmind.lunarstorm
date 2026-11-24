//! Evaluator-only termination through the production compiler and native VM.
#![cfg(feature = "quickjs-ng")]
use coldmind_runtime::{debug::*, engine::*, engines::quickjs_ng::QuickJsNgEngine, vm::*};
use std::{rc::Rc, sync::mpsc, time::Duration};
#[path = "support/debugger.rs"]
mod debugger;
use debugger::*;
type Vm = VirtualMachine<QuickJsNgEngine>;
fn vm() -> Vm {
    VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap()
}
#[test]
fn evaluator_capability_has_fresh_native_build_identity() {
    let engine = QuickJsNgEngine::new().unwrap();
    let identity = &engine.descriptor().build_fingerprint;
    assert!(
        identity.contains("eval-control-v1") && identity.contains(";abi=14;"),
        "{identity}"
    );
    println!("G004_NATIVE_BUILD={identity}");
    let mut vm = VirtualMachine::new(engine, VmConfig::default()).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let capabilities = debug.descriptor().capabilities;
    assert!(capabilities.contains(DebugCapability::CancelEvaluation));
    assert!(capabilities.contains(DebugCapability::VariableMutation));
    assert!(!capabilities.contains(DebugCapability::ReadOnlyEvaluation));
}
fn source(text: &str) -> ScriptSource {
    ScriptSource::new(JsString::from_utf8(text), "file:///evaluation.js".into())
}
fn top(d: &VmDebugHandle, pause: PauseId) -> FrameRef {
    match ask(
        d,
        DebugRequest::Stack {
            pause,
            page: Page { start: 0, count: 1 },
        },
    ) {
        DebugReply::Stack { frames, .. } => frames[0].id,
        other => panic!("{other:?}"),
    }
}
fn start(d: &VmDebugHandle, frame: FrameRef, text: &str, deadline_ms: u64) -> DebugRequestId {
    send(
        d,
        DebugRequest::EvaluateWithOptions {
            frame,
            expression: JsString::from_utf8(text),
            options: EvaluationOptions {
                deadline_ms,
                allow_side_effects: true,
            },
        },
    )
}
fn outcome(d: &VmDebugHandle, id: DebugRequestId) -> EvaluationOutcome {
    match reply(d, id).unwrap() {
        DebugReply::EvaluationOutcome(outcome) => outcome,
        other => panic!("{other:?}"),
    }
}
fn eval(d: &VmDebugHandle, frame: FrameRef, text: &str) -> String {
    let out = outcome(d, start(d, frame, text, 2_000));
    assert_eq!(out.status, EvaluationStatus::Returned);
    out.value.unwrap().preview.to_utf8().unwrap()
}
fn terminated(out: EvaluationOutcome, status: EvaluationStatus) {
    assert_eq!(out.status, status);
    assert!(out.value.is_none());
    assert!(out.effects_may_remain);
}
struct Checkpoint(mpsc::Sender<()>);
impl HostFunction for Checkpoint {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.0.send(()).unwrap();
        call.import_data(JsData::Undefined)
    }
}
fn checkpoint(vm: &mut Vm) -> mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel();
    vm.bind_function(JsString::from_utf8("entered"), 0, Rc::new(Checkpoint(tx)))
        .unwrap();
    rx
}
#[test]
fn cancel_busy_evaluator_preserves_application_pause() {
    let mut vm = vm();
    let entered = checkpoint(&mut vm);
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let id = start(&debug, frame, "entered();while(true){}", 10_000);
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(
            debug.cancel_evaluation(id).unwrap(),
            DebugControlReceipt::Accepted
        ));
        terminated(outcome(&debug, id), EvaluationStatus::Cancelled);
        assert_eq!(debug.state(), DebugExecutionState::Paused(pause));
        assert_eq!(eval(&debug, frame, "local+1"), "42");
        assert!(matches!(
            ask(&debug, DebugRequest::Scopes { frame }),
            DebugReply::Scopes(_)
        ));
        assert_eq!(
            debug.cancel_evaluation(id).unwrap_err(),
            DebugError::InvalidRequest
        );
        let legacy = send(
            &debug,
            DebugRequest::Evaluate {
                frame,
                expression: JsString::from_utf8("entered();while(true){}"),
                allow_side_effects: true,
            },
        );
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        debug.cancel_evaluation(legacy).unwrap();
        assert_eq!(
            reply(&debug, legacy).unwrap_err(),
            DebugError::EvaluationCancelled {
                effects_may_remain: true
            }
        );
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source("(function(){let local=41;debugger;return local})()"))
        .unwrap();
    c.join().unwrap();
}
#[test]
fn deadline_keeps_previous_writes_and_skips_evaluator_finally() {
    let mut vm = vm();
    let entered = checkpoint(&mut vm);
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let id = start(
            &debug,
            frame,
            "writes=42;entered();try{while(true){}}catch(e){caught++}finally{cleanup++;while(true){}}",
            2_000,
        );
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        terminated(outcome(&debug, id), EvaluationStatus::DeadlineExceeded);
        assert_eq!(
            eval(&debug, frame, "[writes,caught,cleanup].join(',')"),
            "\"42,0,0\""
        );
        // Ordinary throws still run the evaluator's own catch/finally.
        assert_eq!(
            eval(
                &debug,
                frame,
                "try{throw 1}catch(e){caught++}finally{cleanup++};caught+cleanup"
            ),
            "2"
        );
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source("let writes=0,caught=0,cleanup=0;debugger;"))
        .unwrap();
    c.join().unwrap();
}
struct Post(bool);
impl HostFunction for Post {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let job = CallbackJob {
            function: call.retain(call.arguments()[0])?,
            receiver: None,
            arguments: Vec::new(),
        };
        let result = if self.0 {
            call.host().timer(Duration::ZERO, None, job)
        } else {
            call.host().post(job)
        };
        result
            .map_err(|_| FailureReport::fault(FailureCode::Capacity, "queued callback refused"))?;
        call.import_data(JsData::Undefined)
    }
}
#[test]
fn queued_work_survives_cancel_but_waits_for_resume() {
    let mut vm = vm();
    let entered = checkpoint(&mut vm);
    vm.bind_function(JsString::from_utf8("post"), 1, Rc::new(Post(false)))
        .unwrap();
    vm.bind_function(JsString::from_utf8("timer"), 1, Rc::new(Post(true)))
        .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let id = start(
            &debug,
            frame,
            "post(()=>jobs++);timer(()=>timers++);Promise.resolve().then(()=>microtasks++);entered();while(true){}",
            10_000,
        );
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        debug.cancel_evaluation(id).unwrap();
        terminated(outcome(&debug, id), EvaluationStatus::Cancelled);
        assert_eq!(
            eval(&debug, frame, "jobs===0&&microtasks===0&&timers===0"),
            "true"
        );
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    vm.evaluate(source(
        "globalThis.jobs=0;globalThis.microtasks=0;globalThis.timers=0;debugger;",
    ))
    .unwrap();
    let debug = c.join().unwrap();
    vm.tick(false).unwrap();
    // A second real pause proves all three work kinds survived cancellation.
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        assert_eq!(
            eval(
                &debug,
                top(&debug, pause),
                "jobs===1&&microtasks===1&&timers===1"
            ),
            "true"
        );
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source("debugger;")).unwrap();
    c.join().unwrap();
}
struct Blocked {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}
impl HostFunction for Blocked {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.entered.send(()).unwrap();
        self.release.recv_timeout(Duration::from_secs(10)).unwrap();
        call.import_data(JsData::Number(17.0))
    }
}
#[test]
fn blocked_host_callback_reports_pending_cancel_until_return() {
    let mut vm = vm();
    let (tx, entered) = mpsc::channel();
    let (release, rx) = mpsc::channel();
    vm.bind_function(
        JsString::from_utf8("blocked"),
        0,
        Rc::new(Blocked {
            entered: tx,
            release: rx,
        }),
    )
    .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let id = start(&debug, frame, "blocked();afterReturn++", 10_000);
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        debug.cancel_evaluation(id).unwrap();
        assert!(matches!(
            debug.cancel_evaluation(id).unwrap(),
            DebugControlReceipt::AlreadyRequested
        ));
        while let Some(update) = debug.try_recv() {
            assert!(
                !matches!(update, DebugUpdate::Reply { id: actual, .. } if actual == id),
                "native callback has not returned; cancellation is pending"
            );
            assert!(!matches!(update, DebugUpdate::SessionClosed { .. }));
        }
        release.send(()).unwrap();
        terminated(outcome(&debug, id), EvaluationStatus::Cancelled);
        assert_eq!(eval(&debug, frame, "afterReturn"), "0");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source("let afterReturn=0;debugger;")).unwrap();
    c.join().unwrap();
}
#[test]
fn stop_wins_over_cancel_and_detach_with_full_queues() {
    let mut config = VmConfig::default();
    config.debug_limits.requests = 2;
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let (tx, entered) = mpsc::channel();
    let (release, rx) = mpsc::channel();
    vm.bind_function(
        JsString::from_utf8("blocked"),
        0,
        Rc::new(Blocked {
            entered: tx,
            release: rx,
        }),
    )
    .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let stop = vm.handle();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let eval_id = start(&debug, frame, "blocked();while(true){}", 10_000);
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        let queued = send(
            &debug,
            DebugRequest::Sources {
                page: Page { start: 0, count: 1 },
            },
        );
        assert!(matches!(
            debug
                .try_request(DebugRequest::Sources {
                    page: Page { start: 0, count: 1 }
                })
                .unwrap_err()
                .reason,
            RejectionReason::Debug(DebugError::Capacity)
        ));
        debug.cancel_evaluation(eval_id).unwrap();
        stop.request_stop().unwrap();
        let _ = debug.detach();
        release.send(()).unwrap();
        let mut terminal = std::collections::HashSet::new();
        loop {
            match update(&debug) {
                DebugUpdate::Reply { id, result } => {
                    assert!(terminal.insert(id), "duplicate terminal result");
                    if id == eval_id {
                        assert_eq!(result.unwrap_err(), DebugError::Closed);
                    }
                }
                DebugUpdate::SessionClosed { .. } => break,
                _ => {}
            }
        }
        while let Some(update) = debug.try_recv() {
            if let DebugUpdate::Reply { id, .. } = update {
                assert!(terminal.insert(id));
            }
        }
        assert!(terminal.contains(&eval_id) && terminal.contains(&queued));
    });
    let result = vm.evaluate(source("debugger;throw 'application resumed'"));
    c.join().unwrap();
    assert!(matches!(result, Err(VmError::Stopped)), "{result:?}");
}

#[test]
fn deadline_bypasses_iterator_return_and_application_stack_formatter() {
    let mut vm = vm();
    let entered = checkpoint(&mut vm);
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let id = start(
            &debug,
            frame,
            "for(const x of iterator){entered();while(true){}}",
            2_000,
        );
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        terminated(outcome(&debug, id), EvaluationStatus::DeadlineExceeded);
        assert_eq!(eval(&debug, frame, "closed+formatted"), "0");
        assert_eq!(eval(&debug, frame, "40+2"), "42");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source("let closed=0,formatted=0;Error.prepareStackTrace=()=>{formatted++;throw 17};let iterator={[Symbol.iterator](){return this},next(){return {value:1,done:false}},return(){closed++;throw 18}};debugger;")).unwrap();
    c.join().unwrap();
}

#[test]
fn original_pending_application_exception_survives_deadline_and_followup_evaluation() {
    let mut vm = vm();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let policy = send(
        &debug,
        DebugRequest::ExceptionPolicy(ExceptionPolicy::Uncaught),
    );
    let c = controller(&vm, move || {
        reply(&debug, policy).unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        let frame = top(&debug, pause);
        terminated(
            outcome(&debug, start(&debug, frame, "while(true){}", 30)),
            EvaluationStatus::DeadlineExceeded,
        );
        assert_eq!(eval(&debug, frame, "6*7"), "42");
        let thrown = outcome(&debug, start(&debug, frame, "throw 99", 2_000));
        assert_eq!(thrown.status, EvaluationStatus::Threw);
        assert_eq!(thrown.value.unwrap().preview.to_utf8().unwrap(), "99");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    let error = vm.evaluate(source("throw 73")).unwrap_err();
    c.join().unwrap();
    let VmError::Execution(report) = error else {
        panic!("{error:?}")
    };
    assert_eq!(report.thrown, Some(JsData::Number(73.0)));
}

#[test]
fn async_and_generator_consumers_propagate_deadline_without_rejection_or_cleanup() {
    for expression in [
        "(async()=>{try{entered();while(true){}}finally{cleanup++}})()",
        "(async function*(){try{entered();while(true){}}finally{cleanup++}})().next()",
        "(function*(){try{entered();while(true){}}finally{cleanup++}})().next()",
        "Promise.resolve({get then(){entered();while(true){}}})",
        "new Promise(()=>{entered();while(true){}})",
        "let stack=new DisposableStack();stack.defer(()=>cleanup++);stack.defer(()=>{entered();while(true){}});stack.dispose()",
    ] {
        let mut vm = vm();
        let entered = checkpoint(&mut vm);
        let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
        let expression = expression.to_owned();
        let c = controller(&vm, move || {
            let (pause, _, _) = paused(&debug);
            let frame = top(&debug, pause);
            let id = start(&debug, frame, &expression, 2_000);
            entered.recv_timeout(Duration::from_secs(10)).unwrap();
            terminated(outcome(&debug, id), EvaluationStatus::DeadlineExceeded);
            assert_eq!(eval(&debug, frame, "cleanup"), "0");
            debug
                .continue_execution(pause, DebugContinue::Resume)
                .unwrap();
        });
        vm.evaluate(source("let cleanup=0;debugger;")).unwrap();
        c.join().unwrap();
    }
}

struct SwallowAndReenter;
impl HostFunction for SwallowAndReenter {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        assert!(call.call(call.arguments()[0], None, &[]).is_err());
        // Consuming the scoped error must not clear cancellation authority.
        assert!(call.call(call.arguments()[1], None, &[]).is_err());
        assert!(call.construct(call.arguments()[2], &[], None).is_err());
        call.import_data(JsData::Number(99.0))
    }
}
#[test]
fn host_consuming_carrier_cannot_reenter_or_poison_application() {
    let mut vm = vm();
    let entered = checkpoint(&mut vm);
    vm.bind_function(
        JsString::from_utf8("swallow"),
        3,
        Rc::new(SwallowAndReenter),
    )
    .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let id = start(
            &debug,
            frame,
            "swallow(()=>{entered();while(true){}},()=>reentered++,Array);afterReturn++",
            10_000,
        );
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        debug.cancel_evaluation(id).unwrap();
        terminated(outcome(&debug, id), EvaluationStatus::Cancelled);
        assert_eq!(eval(&debug, frame, "reentered+afterReturn"), "0");
        assert_eq!(eval(&debug, frame, "21*2"), "42");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source("let reentered=0,afterReturn=0;debugger;"))
        .unwrap();
    c.join().unwrap();
    vm.evaluate(source("1+1")).unwrap();
}

#[test]
fn legacy_evaluation_uses_configured_budget_and_reports_typed_expiry() {
    let mut config = VmConfig::default();
    config.debug_limits.evaluation_deadline_ms = 30;
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let id = send(
            &debug,
            DebugRequest::Evaluate {
                frame,
                expression: JsString::from_utf8("while(true){}"),
                allow_side_effects: true,
            },
        );
        assert_eq!(
            reply(&debug, id).unwrap_err(),
            DebugError::EvaluationDeadlineExceeded {
                effects_may_remain: true
            }
        );
        assert!(matches!(
            debug
                .try_request(DebugRequest::EvaluateWithOptions {
                    frame,
                    expression: JsString::from_utf8("1+1"),
                    options: EvaluationOptions {
                        allow_side_effects: false,
                        ..EvaluationOptions::default()
                    },
                })
                .unwrap_err()
                .reason,
            RejectionReason::Debug(DebugError::Unsupported)
        ));
        assert_eq!(eval(&debug, frame, "1+1"), "2");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source("debugger;")).unwrap();
    c.join().unwrap();
}

#[test]
fn invalid_host_debugger_budgets_are_rejected_before_vm_creation() {
    for deadline_ms in [0, u64::MAX] {
        for condition in [false, true] {
            let mut config = VmConfig::default();
            if condition {
                config.debug_limits.condition_deadline_ms = deadline_ms;
            } else {
                config.debug_limits.evaluation_deadline_ms = deadline_ms;
            }
            assert!(matches!(
                VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config),
                Err(VmError::Configuration(_))
            ));
        }
    }
}

#[test]
fn evaluator_native_oom_does_not_poison_paused_application() {
    use coldmind_runtime::engines::quickjs_ng::QuickJsNgConfig;
    let engine = QuickJsNgEngine::with_config(QuickJsNgConfig {
        max_memory_bytes: Some(2 * 1024 * 1024),
        ..QuickJsNgConfig::default()
    })
    .unwrap();
    let mut vm = VirtualMachine::new(engine, VmConfig::default()).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let id = start(&debug, frame, "Array(1000000).fill('x')", 2_000);
        assert_eq!(reply(&debug, id).unwrap_err(), DebugError::Capacity);
        assert_eq!(eval(&debug, frame, "6*7"), "42");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source("debugger;")).unwrap();
    c.join().unwrap();
    vm.evaluate(source("1+1")).unwrap();
}

#[test]
fn detach_cancels_active_evaluator_then_resumes_restored_application() {
    let mut vm = vm();
    let (tx, entered) = mpsc::channel();
    let (release, rx) = mpsc::channel();
    vm.bind_function(
        JsString::from_utf8("blocked"),
        0,
        Rc::new(Blocked {
            entered: tx,
            release: rx,
        }),
    )
    .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let id = start(&debug, frame, "blocked();while(true){}", 10_000);
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        debug.detach().unwrap();
        while let Some(update) = debug.try_recv() {
            assert!(!matches!(update, DebugUpdate::Reply { id: actual, .. } if actual == id));
            assert!(!matches!(update, DebugUpdate::SessionClosed { .. }));
        }
        release.send(()).unwrap();
        let mut terminal = false;
        loop {
            match update(&debug) {
                DebugUpdate::Reply { id: actual, result } if actual == id => {
                    assert!(!terminal);
                    terminal = true;
                    let DebugReply::EvaluationOutcome(out) = result.unwrap() else {
                        panic!()
                    };
                    terminated(out, EvaluationStatus::Cancelled);
                }
                DebugUpdate::SessionClosed { .. } => break,
                _ => {}
            }
        }
        // Session closure is allowed to be observed before reserved replies.
        while let Some(update) = debug.try_recv() {
            if let DebugUpdate::Reply { id: actual, result } = update {
                assert_eq!(actual, id);
                assert!(!terminal);
                terminal = true;
                let DebugReply::EvaluationOutcome(out) = result.unwrap() else {
                    panic!()
                };
                terminated(out, EvaluationStatus::Cancelled);
            }
        }
        assert!(terminal);
    });
    vm.evaluate(source("let resumed=0;debugger;resumed=42;"))
        .unwrap();
    c.join().unwrap();
    assert_eq!(vm.debug_state(), DebugExecutionState::Detached);
    vm.evaluate(source("if(resumed!==42)throw 'not resumed'"))
        .unwrap();
}

#[test]
fn dap_cancel_and_deadline_restart_invalidated_inspection_checkpoints() {
    use serde_json::{Value, json};
    use std::time::Instant;
    fn request(dap: &mut DapAdapter, seq: u64, command: &str, arguments: Value) -> Vec<Value> {
        let body = serde_json::to_vec(
            &json!({"seq":seq,"type":"request","command":command,"arguments":arguments}),
        )
        .unwrap();
        let mut wire = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        wire.extend(body);
        dap.receive(&wire)
            .unwrap()
            .into_iter()
            .map(|frame| decode(&frame))
            .collect()
    }
    fn decode(frame: &[u8]) -> Value {
        let start = frame.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
        serde_json::from_slice(&frame[start..]).unwrap()
    }
    fn response(dap: &mut DapAdapter, seq: u64, mut frames: Vec<Value>) -> Value {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(index) = frames.iter().position(|f| f["request_seq"] == seq) {
                return frames.remove(index);
            }
            assert!(Instant::now() < until, "missing DAP response {seq}");
            frames.extend(dap.poll().unwrap().iter().map(|f| decode(f)));
            std::thread::yield_now();
        }
    }
    for mode in 0..4 {
        let cancel = mode == 1;
        let mut config = VmConfig::default();
        config.debug_limits.requests = 1;
        config.debug_limits.evaluation_deadline_ms = if cancel { 10_000 } else { 250 };
        let engine =
            QuickJsNgEngine::with_config(coldmind_runtime::engines::quickjs_ng::QuickJsNgConfig {
                max_memory_bytes: Some(2 * 1024 * 1024),
                ..Default::default()
            })
            .unwrap();
        let mut vm = VirtualMachine::new(engine, config).unwrap();
        let entered = checkpoint(&mut vm);
        let (gate_tx, gate_entered) = mpsc::channel();
        let (gate_release, gate_rx) = mpsc::channel();
        vm.bind_function(
            JsString::from_utf8("blocked"),
            0,
            Rc::new(Blocked {
                entered: gate_tx,
                release: gate_rx,
            }),
        )
        .unwrap();
        vm.bind_function(JsString::from_utf8("fault"), 0, Rc::new(FaultingHost))
            .unwrap();
        let debug = vm
            .attach_debugger(DebugSessionOptions {
                break_on_start: true,
            })
            .unwrap();
        let mut dap = DapAdapter::new(debug).unwrap();
        let c = controller(&vm, move || {
            let initialized = request(&mut dap, 1, "initialize", json!({}));
            assert_eq!(initialized[0]["body"]["supportsCancelRequest"], true);
            let mut frames = request(&mut dap, 2, "configurationDone", json!({}));
            let until = Instant::now() + Duration::from_secs(10);
            while !frames.iter().any(|f| f["event"] == "stopped") {
                assert!(Instant::now() < until);
                frames.extend(dap.poll().unwrap().iter().map(|f| decode(f)));
                std::thread::yield_now();
            }
            let frames = request(&mut dap, 3, "stackTrace", json!({"threadId":1}));
            let stack = response(&mut dap, 3, frames);
            let frame = stack["body"]["stackFrames"][0]["id"].clone();
            let frames = request(
                &mut dap,
                4,
                "evaluate",
                json!({"frameId":frame,"expression":"object","context":"repl"}),
            );
            let evaluated = response(&mut dap, 4, frames);
            let reference = evaluated["body"]["variablesReference"].clone();
            let frames = request(
                &mut dap,
                5,
                "variables",
                json!({"variablesReference":reference,"start":0,"count":64}),
            );
            assert_eq!(response(&mut dap, 5, frames)["success"], true);
            // Rejection never reached native and must retain a live checkpoint.
            let rejected = request(
                &mut dap,
                6,
                "evaluate",
                json!({"frameId":frame,"expression":"1+1","context":"hover"}),
            );
            assert_eq!(response(&mut dap, 6, rejected)["success"], false);
            let frames = request(
                &mut dap,
                7,
                "variables",
                json!({"variablesReference":reference,"start":64,"count":64}),
            );
            assert_eq!(response(&mut dap, 7, frames)["success"], true);
            let expression = match mode {
                0 => "object.p0=500;entered();while(true){}",
                1 => "object.p0=500;while(true){blocked();}",
                2 => "object.p0=500;fault()",
                _ => "object.p0=500;Array(1000000).fill('x')",
            };
            let mut frames = request(
                &mut dap,
                8,
                "evaluate",
                json!({"frameId":frame,"expression":expression,"context":"repl"}),
            );
            if mode == 0 {
                entered.recv_timeout(Duration::from_secs(10)).unwrap();
            }
            if cancel {
                gate_entered.recv_timeout(Duration::from_secs(10)).unwrap();
                for (seq, arguments) in [
                    (9, json!({"requestId":9999})),
                    (10, json!({"requestId":7})),
                    (11, json!({})),
                    (12, json!({"progressId":"unmatched"})),
                ] {
                    let acknowledged = request(&mut dap, seq, "cancel", arguments);
                    assert_eq!(
                        response(&mut dap, seq, acknowledged.clone())["success"],
                        true
                    );
                    assert!(!acknowledged.iter().any(|f| f["request_seq"] == 8));
                    frames.extend(acknowledged);
                    gate_release.send(()).unwrap();
                    // Wrong-target/no-op cancellation must leave the evaluator
                    // running: it must reach the next genuine native checkpoint.
                    gate_entered.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                let cancelled = request(
                    &mut dap,
                    13,
                    "cancel",
                    json!({"requestId":8,"progressId":"unmatched"}),
                );
                assert_eq!(response(&mut dap, 13, cancelled.clone())["success"], true);
                assert!(!cancelled.iter().any(|f| f["request_seq"] == 8));
                frames.extend(cancelled);
                assert!(
                    !dap.poll()
                        .unwrap()
                        .iter()
                        .map(|f| decode(f))
                        .any(|f| f["request_seq"] == 8)
                );
                gate_release.send(()).unwrap();
            }
            let evaluated = response(&mut dap, 8, std::mem::take(&mut frames));
            assert_eq!(evaluated["success"], false);
            if mode < 2 {
                assert_eq!(
                    evaluated["message"],
                    if cancel {
                        "cancelled"
                    } else {
                        "Evaluation deadline exceeded"
                    }
                );
                assert_eq!(evaluated["body"]["effectsMayRemain"], true);
                assert_eq!(evaluated["body"]["error"]["showUser"], true);
                assert!(
                    evaluated["body"]["error"]["format"]
                        .as_str()
                        .unwrap()
                        .contains("not rolled back")
                );
            } else {
                assert!(
                    evaluated["message"]
                        .as_str()
                        .unwrap()
                        .contains(if mode == 2 {
                            "genuine evaluator host fault"
                        } else {
                            "Capacity"
                        })
                );
            }
            let frames = request(
                &mut dap,
                14,
                "variables",
                json!({"variablesReference":reference,"start":128,"count":2}),
            );
            let page = response(&mut dap, 14, frames);
            assert_eq!(page["success"], true, "{page}");
            assert_eq!(page["body"]["variables"][0]["name"], "p128");
            let frames = request(
                &mut dap,
                15,
                "variables",
                json!({"variablesReference":reference,"start":0,"count":1}),
            );
            let page = response(&mut dap, 15, frames);
            assert_eq!(page["body"]["variables"][0]["value"], "500");
            assert!(
                !dap.poll()
                    .unwrap()
                    .iter()
                    .map(|f| decode(f))
                    .any(|f| f["request_seq"] == 8)
            );
            request(&mut dap, 16, "continue", json!({"threadId":1}));
            for (seq, arguments) in [
                (17, json!({})),
                (18, json!({"progressId":"unmatched"})),
                (19, json!({"requestId":8})),
            ] {
                let frames = request(&mut dap, seq, "cancel", arguments);
                assert_eq!(response(&mut dap, seq, frames)["success"], true);
            }
            dap
        });
        vm.evaluate(source(
            "let object={};for(let i=0;i<130;i++)object['p'+i]=i;debugger;",
        ))
        .unwrap();
        let _dap = c.join().unwrap();
    }
}

struct FaultingHost;
impl HostFunction for FaultingHost {
    fn invoke<'s>(&self, _: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        Err(FailureReport::fault(FailureCode::EngineFailure, "genuine evaluator host fault").into())
    }
}
#[test]
fn genuine_evaluator_host_fault_is_reported_without_poisoning_application() {
    let mut vm = vm();
    vm.bind_function(JsString::from_utf8("fault"), 0, Rc::new(FaultingHost))
        .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let id = start(&debug, frame, "fault()", 2_000);
        let DebugError::EngineFailure(report) = reply(&debug, id).unwrap_err() else {
            panic!()
        };
        assert_eq!(report.code, FailureCode::EngineFailure);
        assert!(
            report
                .message
                .as_ref()
                .unwrap()
                .to_utf8()
                .unwrap()
                .contains("genuine evaluator host fault")
        );
        assert_eq!(eval(&debug, frame, "6*7"), "42");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source("debugger;")).unwrap();
    c.join().unwrap();
    vm.evaluate(source("1+1")).unwrap();
}

#[test]
fn cancelled_evaluator_preserves_application_error_identity_and_trace() {
    let mut vm = vm();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let policy = send(&debug, DebugRequest::ExceptionPolicy(ExceptionPolicy::All));
    let c = controller(&vm, move || {
        reply(&debug, policy).unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        let frame = top(&debug, pause);
        let value = outcome(&debug, start(&debug, frame, "original", 2_000))
            .value
            .unwrap();
        let original = value.object.unwrap();
        terminated(
            outcome(&debug, start(&debug, frame, "while(true){}", 30)),
            EvaluationStatus::DeadlineExceeded,
        );
        let page = ask(
            &debug,
            DebugRequest::Inspect(InspectionRequest {
                parent: VariableParent::Object(original),
                page: Page {
                    start: 0,
                    count: 64,
                },
                cursor: None,
            }),
        );
        assert!(matches!(page, DebugReply::Inspection(_)));
        assert_eq!(eval(&debug, frame, "original.stack===trace"), "true");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source("let original=new Error('application');let trace=original.stack;try{throw original}catch(error){if(error!==original||error.stack!==trace)throw 'identity or trace changed'}42")).unwrap();
    c.join().unwrap();
}

#[test]
fn configured_condition_expiry_does_not_stop_or_pause_application() {
    let mut config = VmConfig::default();
    config.debug_limits.condition_deadline_ms = 30;
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let setup = send(
        &debug,
        DebugRequest::SetBreakpoints {
            selector: ScriptSelector {
                origin: "file:///evaluation.js".into(),
                expected_digest: None,
            },
            breakpoints: vec![BreakpointSpec {
                client_id: 1,
                line: 1,
                column: None,
                condition: Some(JsString::from_utf8("while(true){}")),
                enabled: true,
                hit_condition: None,
                log_message: None,
            }],
        },
    );
    let c = controller(&vm, move || {
        reply(&debug, setup).unwrap();
        loop {
            match update(&debug) {
                DebugUpdate::Diagnostic { report, .. } => {
                    assert!(
                        report
                            .message
                            .as_ref()
                            .unwrap()
                            .to_utf8()
                            .unwrap()
                            .contains("evaluator budget")
                    );
                    break;
                }
                DebugUpdate::Paused { .. } => panic!("expired condition must not match"),
                _ => {}
            }
        }
        debug
    });
    vm.evaluate(source(
        "globalThis.ran=0;\nran++;\nif(ran!==1)throw 'application failed'",
    ))
    .unwrap();
    let _debug = c.join().unwrap();
}

#[test]
fn real_native_return_and_throw_race_with_cancel_publish_one_terminal_result() {
    let mut vm = vm();
    let (tx, entered) = mpsc::channel();
    let (release, rx) = mpsc::channel();
    vm.bind_function(
        JsString::from_utf8("blocked"),
        0,
        Rc::new(Blocked {
            entered: tx,
            release: rx,
        }),
    )
    .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        for (expression, completed, value) in [
            ("blocked();42", EvaluationStatus::Returned, "42"),
            ("blocked();throw 99", EvaluationStatus::Threw, "99"),
        ] {
            for _ in 0..16 {
                let id = start(&debug, frame, expression, 2_000);
                entered.recv_timeout(Duration::from_secs(10)).unwrap();
                let release = release.clone();
                let racing = std::thread::spawn(move || release.send(()).unwrap());
                let cancellation = debug.cancel_evaluation(id);
                racing.join().unwrap();
                let out = outcome(&debug, id);
                match out.status {
                    EvaluationStatus::Cancelled => terminated(out, EvaluationStatus::Cancelled),
                    status => {
                        assert_eq!(status, completed);
                        assert_eq!(out.value.unwrap().preview.to_utf8().unwrap(), value);
                    }
                }
                assert!(matches!(
                    cancellation,
                    Ok(_) | Err(DebugError::InvalidRequest)
                ));
                while let Some(update) = debug.try_recv() {
                    assert!(
                        !matches!(update, DebugUpdate::Reply { id: actual, .. } if actual == id)
                    );
                }
            }
        }
        assert_eq!(eval(&debug, frame, "6*7"), "42");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source("debugger;")).unwrap();
    c.join().unwrap();
}
