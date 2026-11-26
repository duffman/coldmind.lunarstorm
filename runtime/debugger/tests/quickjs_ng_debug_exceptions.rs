//! Exception snapshots through the production compiler/native VM.
#![cfg(feature = "quickjs-ng")]
use coldmind_runtime::{debug::*, engine::*, engines::quickjs_ng::QuickJsNgEngine, vm::*};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    rc::{Rc, Weak},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
#[path = "support/debugger.rs"]
mod debugger;
use debugger::{ask, controller, paused, reply, send};
struct SharedEngine(Rc<QuickJsNgEngine>);
impl JsEngine for SharedEngine {
    fn descriptor(&self) -> &EngineDescriptor {
        self.0.descriptor()
    }
    fn create_context(&self, limits: ContextLimits) -> Result<ContextId, FailureReport> {
        self.0.create_context(limits)
    }
    fn close_context(&self, context: ContextId) -> Result<(), FailureReport> {
        self.0.close_context(context)
    }
    fn debug_backend(
        &self,
        context: ContextId,
    ) -> Result<Option<Rc<dyn DebugBackend>>, FailureReport> {
        self.0.debug_backend(context)
    }
    fn with_scope(
        &self,
        context: ContextId,
        host: &dyn HostServices,
        body: &mut dyn ScopeCall,
    ) -> Result<(), FailureReport> {
        self.0.with_scope(context, host, body)
    }
    fn abort_signal(&self, context: ContextId) -> Result<Option<AbortSignal>, FailureReport> {
        self.0.abort_signal(context)
    }
}
struct ForceGc(Weak<QuickJsNgEngine>, Rc<Cell<usize>>);
impl HostFunction for ForceGc {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.0.upgrade().unwrap().collect_garbage(call.context())?;
        self.1.set(self.1.get() + 1);
        call.import_data(JsData::Undefined)
    }
}

fn source(text: &str) -> ScriptSource {
    ScriptSource::new(JsString::from_utf8(text), "file:///exceptions.js".into())
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
fn info(d: &VmDebugHandle, pause: PauseId) -> ExceptionInfo {
    match ask(d, DebugRequest::ExceptionInfo { pause }) {
        DebugReply::ExceptionInfo(info) => info,
        other => panic!("{other:?}"),
    }
}
fn eval(d: &VmDebugHandle, frame: FrameRef, text: &str) -> (DebugValue, bool) {
    match ask(
        d,
        DebugRequest::Evaluate {
            frame,
            expression: JsString::from_utf8(text),
            allow_side_effects: true,
        },
    ) {
        DebugReply::Evaluation { value, exception } => (value, exception),
        other => panic!("{other:?}"),
    }
}
fn setup() -> (
    VirtualMachine<QuickJsNgEngine>,
    VmDebugHandle,
    DebugRequestId,
) {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let policy = send(&debug, DebugRequest::ExceptionPolicy(ExceptionPolicy::All));
    (vm, debug, policy)
}
#[test]
fn exception_info_preserves_primitive_and_object_throw() {
    let engine = Rc::new(QuickJsNgEngine::new().unwrap());
    let mut vm = VirtualMachine::new(SharedEngine(engine.clone()), VmConfig::default()).unwrap();
    let collections = Rc::new(Cell::new(0));
    vm.bind_function(
        JsString::from_utf8("forceGc"),
        0,
        Rc::new(ForceGc(Rc::downgrade(&engine), collections.clone())),
    )
    .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let policy = send(&debug, DebugRequest::ExceptionPolicy(ExceptionPolicy::All));
    let stop = vm.handle();
    let c = std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            reply(&debug, policy).unwrap();
            let (pause, reason, location) = paused(&debug);
            assert_eq!(reason, PauseReason::Exception);
            let primitive = info(&debug, pause);
            assert_eq!(primitive.value.preview.to_utf8().unwrap(), "73");
            assert!(primitive.caught);
            assert_eq!(
                primitive
                    .location
                    .as_ref()
                    .map(|l| (l.script, l.revision, l.line, l.column)),
                location
                    .as_ref()
                    .map(|l| (l.script, l.revision, l.line, l.column))
            );
            assert!(!primitive.stack.is_empty());
            debug
                .continue_execution(pause, DebugContinue::Resume)
                .unwrap();
            let (pause, reason, _) = paused(&debug);
            assert_eq!(reason, PauseReason::Exception);
            // Collect before issuing any inspection reference: the native
            // original-throw lease must itself retain this temporary object.
            assert!(!eval(&debug, top(&debug, pause), "forceGc()").1);
            let after = info(&debug, pause);
            assert!(after.value.object.is_some());
            let DebugReply::Variables { variables, .. } = ask(
                &debug,
                DebugRequest::Variables {
                    parent: VariableParent::Object(after.value.object.unwrap()),
                    page: Page {
                        start: 0,
                        count: 16,
                    },
                },
            ) else {
                panic!("variables")
            };
            assert_eq!(
                variables
                    .iter()
                    .find(|v| v.name.to_utf8().unwrap() == "answer")
                    .unwrap()
                    .value
                    .preview
                    .to_utf8()
                    .unwrap(),
                "42"
            );
            debug
                .continue_execution(pause, DebugContinue::Resume)
                .unwrap();
        }));
        if result.is_err() {
            let _ = stop.request_stop();
        }
        result.unwrap();
    });
    vm.evaluate(source("try{throw 73}catch(e){};function fail(){throw {answer:42}};try{fail()}catch(e){if(e.answer!==42)throw 'changed'}")).unwrap();
    c.join().unwrap();
    assert_eq!(collections.get(), 1);
}
#[test]
fn hostile_error_fields_do_not_execute_getters() {
    let (mut vm, debug, policy) = setup();
    let c = controller(&vm, move || {
        reply(&debug, policy).unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        for _ in 0..3 {
            let snapshot = info(&debug, pause);
            assert!(!snapshot.description.as_units().is_empty());
        }
        assert_eq!(
            eval(&debug, top(&debug, pause), "touched")
                .0
                .preview
                .to_utf8()
                .unwrap(),
            "0"
        );
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        let snapshot = info(&debug, pause);
        let description = snapshot.description.to_utf8().unwrap();
        assert!(description.contains("useful"), "{description}");
        assert!(description.contains("cause: 42"), "{description}");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        assert!(info(&debug, pause).description.as_units().len() <= 512);
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source("var touched=0;let error=new Error('original');for(let key of ['name','message','stack','cause'])Object.defineProperty(error,key,{get(){touched++;throw 9}});try{throw error}catch(e){};try{throw new Error('useful',{cause:42})}catch(e){};let huge=new Error('x'.repeat(2048),{cause:'y'.repeat(2048)});huge.name='z'.repeat(2048);try{throw huge}catch(e){};if(touched!==0)throw 'getter executed'")).unwrap();
    c.join().unwrap();
}
#[test]
fn caught_finally_and_evaluator_errors_stay_distinct() {
    let (mut vm, debug, policy) = setup();
    let c = controller(&vm, move || {
        reply(&debug, policy).unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        assert!(info(&debug, pause).caught);
        let thrown = eval(&debug, top(&debug, pause), "throw 99");
        assert!(thrown.1);
        assert_eq!(thrown.0.preview.to_utf8().unwrap(), "99");
        assert_eq!(info(&debug, pause).value.preview.to_utf8().unwrap(), "7");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        let original = info(&debug, pause);
        assert!(!original.caught, "finally is not a user catch");
        assert_eq!(original.value.preview.to_utf8().unwrap(), "73");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    let error=vm.evaluate(source("try{throw 7}catch(e){};function fail(){try{throw 73}finally{globalThis.cleaned=true}}fail()")).unwrap_err();
    let debug = c.join().unwrap();
    assert!(
        !matches!(debug.try_recv(), Some(DebugUpdate::Paused { .. })),
        "no repeated throw notification"
    );
    let VmError::Execution(report) = error else {
        panic!("{error:?}")
    };
    assert_eq!(report.thrown, Some(JsData::Number(73.0)));
}
struct Checkpoint(mpsc::Sender<()>);
impl HostFunction for Checkpoint {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.0.send(()).unwrap();
        call.import_data(JsData::Undefined)
    }
}
#[test]
fn cancel_during_exception_pause_restores_original_throw() {
    let (mut vm, debug, policy) = setup();
    let (tx, rx) = mpsc::channel();
    vm.bind_function(JsString::from_utf8("entered"), 0, Rc::new(Checkpoint(tx)))
        .unwrap();
    let c = controller(&vm, move || {
        reply(&debug, policy).unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        let frame = top(&debug, pause);
        let evaluation = send(
            &debug,
            DebugRequest::EvaluateWithOptions {
                frame,
                expression: JsString::from_utf8("entered();while(true){}"),
                options: EvaluationOptions {
                    deadline_ms: 10_000,
                    allow_side_effects: true,
                },
            },
        );
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        debug.cancel_evaluation(evaluation).unwrap();
        let DebugReply::EvaluationOutcome(outcome) = reply(&debug, evaluation).unwrap() else {
            panic!("outcome")
        };
        assert_eq!(outcome.status, EvaluationStatus::Cancelled);
        assert!(outcome.effects_may_remain);
        assert_eq!(info(&debug, pause).value.preview.to_utf8().unwrap(), "73");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    let error = vm
        .evaluate(source("function fail(){throw 73}fail()"))
        .unwrap_err();
    c.join().unwrap();
    let VmError::Execution(report) = error else {
        panic!("{error:?}")
    };
    assert_eq!(report.thrown, Some(JsData::Number(73.0)));
}

// Real DAP translation over the production owner-thread VM, not a metadata fixture.
struct DapClient {
    adapter: DapAdapter,
    seq: u64,
    inbox: Vec<Value>,
}
impl DapClient {
    fn decode(wire: &[u8]) -> Value {
        let start = wire.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
        serde_json::from_slice(&wire[start..]).unwrap()
    }
    fn pump(&mut self) {
        self.inbox
            .extend(self.adapter.poll().unwrap().iter().map(|f| Self::decode(f)));
    }
    fn take(&mut self, label: &str, predicate: impl Fn(&Value) -> bool) -> Value {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(index) = self.inbox.iter().position(&predicate) {
                return self.inbox.remove(index);
            }
            assert!(
                Instant::now() < until,
                "DAP deadline for {label}; inbox={:?}",
                self.inbox
            );
            self.pump();
            thread::yield_now();
        }
    }
    fn submit(&mut self, command: &str, arguments: Value) -> u64 {
        self.seq += 1;
        let seq = self.seq;
        let body = serde_json::to_vec(
            &json!({"seq":seq,"type":"request","command":command,"arguments":arguments}),
        )
        .unwrap();
        let mut wire = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        wire.extend(body);
        self.inbox.extend(
            self.adapter
                .receive(&wire)
                .unwrap()
                .iter()
                .map(|f| Self::decode(f)),
        );
        seq
    }
    fn request(&mut self, command: &str, arguments: Value) -> Value {
        let seq = self.submit(command, arguments);
        self.response(command, seq)
    }
    fn response(&mut self, command: &str, seq: u64) -> Value {
        let response = self.take(command, |f| {
            f["type"] == "response" && f["request_seq"] == seq
        });
        assert_eq!(response["command"], command);
        assert_eq!(response["success"], true, "{response}");
        response
    }
    fn stopped(&mut self, reason: &str) {
        let event = self.take("stopped", |f| f["event"] == "stopped");
        assert_eq!(event["body"]["reason"], reason);
    }
}

#[test]
fn dap_exception_info_reports_original_throw_and_policy_break_mode() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let mut dap = DapClient {
        adapter: DapAdapter::new(debug).unwrap(),
        seq: 0,
        inbox: vec![],
    };
    let initialize = dap.request("initialize", json!({}));
    assert_eq!(initialize["body"]["supportsExceptionInfoRequest"], true);
    let policy = dap.submit("setExceptionBreakpoints", json!({"filters":["all"]}));
    let c = controller(&vm, move || {
        dap.response("setExceptionBreakpoints", policy);
        dap.request("configurationDone", json!({}));
        dap.stopped("exception");
        let first = dap.request("exceptionInfo", json!({"threadId":1}));
        assert_eq!(first["body"]["description"], "7");
        assert_eq!(first["body"]["breakMode"], "always");
        assert!(
            first["body"]["details"]["stackTrace"]
                .as_str()
                .unwrap()
                .contains("fail")
        );
        dap.request("setExceptionBreakpoints", json!({"filters":["uncaught"]}));
        dap.request("continue", json!({"threadId":1}));
        dap.stopped("exception");
        let second = dap.request("exceptionInfo", json!({"threadId":1}));
        assert_eq!(second["body"]["description"], "73");
        assert_eq!(second["body"]["breakMode"], "unhandled");
        dap.request("continue", json!({"threadId":1}));
    });
    let error = vm
        .evaluate(source(
            "function fail(value){throw value}try{fail(7)}catch(e){};fail(73)",
        ))
        .unwrap_err();
    c.join().unwrap();
    let VmError::Execution(report) = error else {
        panic!("{error:?}")
    };
    assert_eq!(report.thrown, Some(JsData::Number(73.0)));
}
