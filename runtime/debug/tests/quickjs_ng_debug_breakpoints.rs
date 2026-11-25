//! Hit policies, logpoints and temporary run-to through the production compiler/VM.
#![cfg(feature = "quickjs-ng")]
use coldmind_runtime::{debug::*, engine::*, engines::quickjs_ng::QuickJsNgEngine, vm::*};
#[path = "support/debugger.rs"]
mod debugger;
use debugger::*;
use serde_json::{Value, json};
use std::{
    thread,
    time::{Duration, Instant},
};
const ORIGIN: &str = "file:///g008-breakpoints.js";
fn source(text: &str) -> ScriptSource {
    ScriptSource::new(JsString::from_utf8(text), ORIGIN.into())
}
fn selector() -> ScriptSelector {
    ScriptSelector {
        origin: ORIGIN.into(),
        expected_digest: None,
    }
}
fn spec(id: u64, line: u32) -> BreakpointSpec {
    BreakpointSpec {
        client_id: id,
        line,
        column: None,
        condition: None,
        enabled: true,
        hit_condition: None,
        log_message: None,
    }
}
fn set(debug: &VmDebugHandle, breakpoints: Vec<BreakpointSpec>) {
    let DebugReply::Breakpoints(infos) = ask(
        debug,
        DebugRequest::SetBreakpoints {
            selector: selector(),
            breakpoints,
        },
    ) else {
        panic!("breakpoint reply")
    };
    assert!(
        infos.iter().all(
            |info| matches!(&info.state,BreakpointState::Verified(found) if !found.is_empty())
        )
    );
}
fn enabled(debug: &VmDebugHandle, client_id: u64, enabled: bool) {
    assert!(matches!(
        ask(
            debug,
            DebugRequest::SetBreakpointEnabled {
                selector: selector(),
                client_id,
                enabled
            }
        ),
        DebugReply::Breakpoints(_)
    ));
}
fn top(debug: &VmDebugHandle, pause: PauseId) -> FrameRef {
    let DebugReply::Stack { frames, .. } = ask(
        debug,
        DebugRequest::Stack {
            pause,
            page: Page { start: 0, count: 1 },
        },
    ) else {
        panic!("stack")
    };
    frames[0].id
}
fn value(debug: &VmDebugHandle, pause: PauseId, expression: &str) -> String {
    let DebugReply::Evaluation {
        value,
        exception: false,
    } = ask(
        debug,
        DebugRequest::Evaluate {
            frame: top(debug, pause),
            expression: JsString::from_utf8(expression),
            allow_side_effects: true,
        },
    )
    else {
        panic!("evaluation")
    };
    value.preview.to_utf8().unwrap()
}
#[derive(Default)]
struct Events {
    logs: Vec<String>,
    diagnostics: Vec<String>,
}
fn stop(debug: &VmDebugHandle, events: &mut Events, reason: PauseReason, ids: &[u64]) -> PauseId {
    loop {
        match update(debug) {
            DebugUpdate::Paused {
                pause,
                reason: actual,
                breakpoint_ids,
                ..
            } => {
                assert_eq!(actual, reason);
                assert_eq!(breakpoint_ids, ids);
                // Authoritative pause delivery intentionally outranks telemetry.
                // Its publication follows all policy output at this marker, so
                // drain the already-published bounded telemetry before asserting it.
                while let Some(pending) = debug.try_recv() {
                    match pending {
                        DebugUpdate::Output { message } => {
                            events.logs.push(message.to_utf8().unwrap())
                        }
                        DebugUpdate::Diagnostic { report, .. } => events
                            .diagnostics
                            .push(report.message.unwrap().to_utf8().unwrap()),
                        DebugUpdate::SessionClosed { .. } => {
                            panic!("closed while inspecting pause")
                        }
                        DebugUpdate::Paused { .. } => {
                            panic!("unexpected second pause without resume")
                        }
                        _ => {}
                    }
                }
                return pause;
            }
            DebugUpdate::Output { message } => events.logs.push(message.to_utf8().unwrap()),
            DebugUpdate::Diagnostic { report, .. } => events
                .diagnostics
                .push(report.message.unwrap().to_utf8().unwrap()),
            DebugUpdate::SessionClosed { .. } => panic!("closed before stop"),
            _ => {}
        }
    }
}
fn resume(debug: &VmDebugHandle, pause: PauseId) {
    debug
        .continue_execution(pause, DebugContinue::Resume)
        .unwrap();
}
fn run(
    program: &str,
    config: VmConfig,
    check: impl FnOnce(&VmDebugHandle, PauseId) + Send + 'static,
) {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let worker = controller(&vm, move || {
        let (entry, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Entry);
        check(&debug, entry);
        debug
    });
    let result = vm.evaluate(source(program));
    let _debug = worker.join().unwrap();
    drop(result.unwrap());
}
#[test]
fn hit_count_order_and_reset_policy() {
    run(
        "globalThis.conditions=0;\nfor(let i=0;i<8;i++){\n i=i;\n debugger;\n}",
        VmConfig::default(),
        |debug, entry| {
            let mut bp = spec(8, 2);
            bp.hit_condition = Some(HitCondition::parse("%2").unwrap());
            bp.condition = Some(JsString::from_utf8("(++globalThis.conditions, i>=1)"));
            set(debug, vec![bp.clone()]);
            let mut events = Events::default();
            resume(debug, entry);
            let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
            assert_eq!(value(debug, p, "i"), "0");
            assert_eq!(value(debug, p, "conditions"), "1");
            resume(debug, p);
            let p = stop(debug, &mut events, PauseReason::Breakpoint, &[8]);
            assert_eq!(value(debug, p, "i"), "1");
            assert_eq!(value(debug, p, "conditions"), "2");
            enabled(debug, 8, false);
            resume(debug, p);
            for i in 1..=3 {
                let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
                assert_eq!(value(debug, p, "i"), i.to_string());
                assert_eq!(value(debug, p, "conditions"), "2");
                if i == 3 {
                    enabled(debug, 8, true);
                }
                resume(debug, p);
            }
            let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
            assert_eq!(value(debug, p, "i"), "4");
            assert_eq!(value(debug, p, "conditions"), "3");
            resume(debug, p);
            let p = stop(debug, &mut events, PauseReason::Breakpoint, &[8]);
            assert_eq!(value(debug, p, "i"), "5");
            assert_eq!(value(debug, p, "conditions"), "4");
            set(debug, vec![bp]);
            resume(debug, p);
            for i in 5..=6 {
                let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
                assert_eq!(value(debug, p, "i"), i.to_string());
                resume(debug, p);
            }
            let p = stop(debug, &mut events, PauseReason::Breakpoint, &[8]);
            assert_eq!(value(debug, p, "i"), "7");
            assert_eq!(value(debug, p, "conditions"), "6");
            resume(debug, p);
            let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
            resume(debug, p);
            assert!(events.diagnostics.is_empty());
        },
    );
}
#[test]
fn logpoint_output_is_bounded_and_does_not_swallow_real_pause() {
    run(
        "let i=0;\ni++;\ni++;\ni++;\ntry {\n throw 7;\n} catch(e) {\n debugger;\n}",
        VmConfig::default(),
        |debug, entry| {
            let mut log = spec(1, 1);
            log.log_message = Some(JsString::from_utf8("i={i} {{ok}}"));
            let real = spec(2, 2);
            let mut middle = log.clone();
            middle.client_id = 4;
            middle.line = 2;
            let mut last = log.clone();
            last.client_id = 5;
            last.line = 3;
            let mut thrown = spec(3, 5);
            thrown.log_message = Some(JsString::from_utf8("throwing"));
            set(debug, vec![log, middle, last, real, thrown]);
            ask(debug, DebugRequest::ExceptionPolicy(ExceptionPolicy::All));
            let mut events = Events::default();
            resume(debug, entry);
            let p = stop(debug, &mut events, PauseReason::Breakpoint, &[2]);
            assert_eq!(events.logs, vec!["i=0 {ok}\n", "i=1 {ok}\n"]);
            debug
                .continue_execution(p, DebugContinue::StepOver)
                .unwrap();
            let p = stop(debug, &mut events, PauseReason::Step, &[]);
            assert_eq!(events.logs.last().unwrap(), "i=2 {ok}\n");
            resume(debug, p);
            let p = stop(debug, &mut events, PauseReason::Exception, &[]);
            assert_eq!(events.logs.last().unwrap(), "throwing\n");
            resume(debug, p);
            let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
            assert_eq!(events.logs.len(), 4);
            assert!(events.diagnostics.is_empty());
            resume(debug, p);
        },
    );
    let mut config = VmConfig::default();
    config.debug_limits.reply_bytes = 1024;
    run(
        "globalThis.after=0;\nlet mark=1;\ndebugger;",
        config,
        |debug, entry| {
            let mut bp = spec(1, 1);
            bp.log_message = Some(JsString::from_utf8(
                "large {'x'.repeat(10000)} {++globalThis.after}",
            ));
            set(debug, vec![bp]);
            let mut malformed = spec(1, 1);
            malformed.log_message = Some(JsString::from_utf8("{missing"));
            assert!(matches!(
                debug.try_request(DebugRequest::SetBreakpoints {
                    selector: selector(),
                    breakpoints: vec![malformed]
                }),
                Err(Rejected {
                    reason: RejectionReason::Debug(DebugError::InvalidRequest),
                    ..
                })
            ));
            let mut events = Events::default();
            resume(debug, entry);
            let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
            assert!(events.logs.is_empty());
            assert_eq!(events.diagnostics.len(), 1);
            assert_eq!(value(debug, p, "after"), "0");
            resume(debug, p);
        },
    );
}
#[test]
fn logpoint_output_saturation_is_diagnostic_and_owner_resumes() {
    let mut config = VmConfig::default();
    config.debug_limits.updates = 1;
    run(
        "let i=0;\nfor(;i<4;i++){\n i=i;\n}\ndebugger;",
        config,
        |debug, entry| {
            let mut bp = spec(1, 2);
            bp.log_message = Some(JsString::from_utf8("tick"));
            set(debug, vec![bp]);
            while debug.try_recv().is_some() {}
            resume(debug, entry);
            // Leave the bounded output queue undrained until the next authoritative
            // application pause, forcing real publish Capacity without timing guesses.
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if matches!(debug.state(),DebugExecutionState::Paused(current) if current!=entry) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "owner did not reach final debugger statement"
                );
                thread::yield_now();
            }
            let mut events = Events::default();
            let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
            assert_eq!(events.logs, vec!["tick\n"]);
            assert_eq!(events.diagnostics.len(), 1);
            assert!(events.diagnostics[0].contains("Capacity"));
            assert_eq!(value(debug, p, "i"), "4");
            resume(debug, p);
        },
    );
}

#[test]
fn logpoint_condition_timeout_keeps_owner_usable() {
    for expression_timeout in [false, true] {
        let mut config = VmConfig::default();
        config.debug_limits.condition_deadline_ms = 10;
        run(
            "globalThis.after=0;\nlet mark=1;\ndebugger;",
            config,
            move |debug, entry| {
                let mut bp = spec(1, 1);
                if expression_timeout {
                    bp.log_message = Some(JsString::from_utf8("never {eval('while(true);')}"));
                } else {
                    bp.condition = Some(JsString::from_utf8("(()=>{while(true){}})()"));
                    bp.log_message = Some(JsString::from_utf8("never"));
                }
                set(debug, vec![bp]);
                let mut events = Events::default();
                resume(debug, entry);
                let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
                assert!(events.logs.is_empty());
                assert_eq!(events.diagnostics.len(), 1);
                assert!(
                    events.diagnostics[0].contains("EvaluationDeadlineExceeded"),
                    "{:?}",
                    events.diagnostics
                );
                assert_eq!(value(debug, p, "after"), "0");
                resume(debug, p);
            },
        );
    }
}

fn target(debug: &VmDebugHandle, line: u32) -> DebugLocation {
    let DebugReply::Locations(mut found) = ask(
        debug,
        DebugRequest::BreakpointLocations {
            selector: selector(),
            start: SourcePosition { line, column: 0 },
            end: SourcePosition {
                line,
                column: u32::MAX,
            },
        },
    ) else {
        panic!("locations")
    };
    assert_eq!(found.len(), 1);
    found.remove(0)
}
#[test]
fn continue_to_cleans_up_on_all_terminal_paths() {
    run(
        "let i=0;\ndebugger;\nfor(;i<3;i++){\n i=i;\n}\ndebugger;",
        VmConfig::default(),
        |debug, entry| {
            let mut events = Events::default();
            resume(debug, entry);
            let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
            assert!(matches!(
                ask(
                    debug,
                    DebugRequest::ContinueTo {
                        pause: p,
                        location: target(debug, 3)
                    }
                ),
                DebugReply::ContinueTo
            ));
            let p = stop(debug, &mut events, PauseReason::Breakpoint, &[]);
            assert_eq!(value(debug, p, "i"), "0");
            resume(debug, p);
            let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
            assert_eq!(value(debug, p, "i"), "3");
            resume(debug, p);
        },
    );
    run(
        "let i=0;\ndebugger;\ni++;\ndebugger;\ni++;\ni++;\ndebugger;",
        VmConfig::default(),
        |debug, entry| {
            let mut events = Events::default();
            resume(debug, entry);
            let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
            ask(
                debug,
                DebugRequest::ContinueTo {
                    pause: p,
                    location: target(debug, 5),
                },
            );
            let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
            assert_eq!(value(debug, p, "i"), "1");
            resume(debug, p);
            let p = stop(debug, &mut events, PauseReason::DebuggerStatement, &[]);
            assert_eq!(value(debug, p, "i"), "3");
            resume(debug, p);
        },
    );
}

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
    fn stack_position(&mut self, expected: (u32, u32)) {
        let stack = self.request(
            "stackTrace",
            json!({"threadId":1,"startFrame":0,"levels":1}),
        );
        let frames = stack["body"]["stackFrames"].as_array().unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["line"], expected.0);
        assert_eq!(frames[0]["column"], expected.1);
        assert!(frames[0]["source"]["sourceReference"].as_u64().unwrap() > 0);
    }
}

#[test]
fn dap_hit_log_enable_and_continue_to_use_native_policies() {
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
    let caps = dap.request("initialize", json!({}));
    assert_eq!(caps["body"]["supportsHitConditionalBreakpoints"], true);
    assert_eq!(caps["body"]["supportsLogPoints"], true);
    assert_eq!(
        caps["body"]["jetbunny"]["supportsSetBreakpointEnabled"],
        true
    );
    assert_eq!(caps["body"]["jetbunny"]["supportsContinueTo"], true);
    let seq=dap.submit("setBreakpoints",json!({"source":{"path":ORIGIN},"breakpoints":[{"line":3,"logMessage":"i={i}","hitCondition":"%2"},{"line":3,"hitCondition":"3"}]}));
    let worker = controller(&vm, move || {
        let configured = dap.response("setBreakpoints", seq);
        let real = configured["body"]["breakpoints"][1]["id"].clone();
        dap.request("configurationDone", json!({}));
        let event = dap.take("policy stop", |f| f["event"] == "stopped");
        assert_eq!(event["body"]["reason"], "breakpoint");
        assert_eq!(event["body"]["hitBreakpointIds"], json!([real.clone()]));
        let stack = dap.request("stackTrace", json!({"threadId":1,"levels":1}));
        let src = stack["body"]["stackFrames"][0]["source"].clone();
        dap.request(
            "jetbunny/setBreakpointEnabled",
            json!({"source":src.clone(),"breakpointId":real,"enabled":false}),
        );
        let locations = dap.request(
            "breakpointLocations",
            json!({"source":src.clone(),"line":3}),
        );
        let target = &locations["body"]["breakpoints"][0];
        dap.request(
            "jetbunny/continueTo",
            json!({"source":src,"line":target["line"],"column":target["column"]}),
        );
        let event = dap.take("temporary stop", |f| f["event"] == "stopped");
        assert_eq!(event["body"]["reason"], "breakpoint");
        assert_eq!(event["body"]["hitBreakpointIds"], json!([]));
        dap.stack_position((
            target["line"].as_u64().unwrap() as u32,
            target["column"].as_u64().unwrap() as u32,
        ));
        dap.request("continue", json!({"threadId":1}));
        dap.stopped("pause");
        let logs: Vec<_> = dap
            .inbox
            .iter()
            .filter(|f| f["event"] == "output" && f["body"]["category"] == "console")
            .map(|f| f["body"]["output"].as_str().unwrap())
            .collect();
        assert_eq!(logs, vec!["i=1\n", "i=3\n"]);
        dap.request("continue", json!({"threadId":1}));
        dap
    });
    let result = vm.evaluate(source("let i=0;\nfor(;i<4;i++){\n i=i;\n}\ndebugger;"));
    let _dap = worker.join().unwrap();
    drop(result.unwrap());
}

struct Gate {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}
impl HostFunction for Gate {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.entered.send(()).unwrap();
        self.release.recv_timeout(Duration::from_secs(10)).unwrap();
        call.import_data(JsData::Undefined)
    }
}
struct ReplaceSource;
impl HostFunction for ReplaceSource {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        call.source()
            .unwrap()
            .evaluate(&source("globalThis.replaced=1;"))?;
        call.import_data(JsData::Undefined)
    }
}
#[test]
fn continue_to_stop_detach_and_source_replacement_remove_temporary_policy() {
    for stopping in [false, true] {
        let mut vm =
            VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
        let (tx, entered) = std::sync::mpsc::channel();
        let (release, rx) = std::sync::mpsc::channel();
        vm.bind_function(
            JsString::from_utf8("gate"),
            0,
            std::rc::Rc::new(Gate {
                entered: tx,
                release: rx,
            }),
        )
        .unwrap();
        let debug = vm
            .attach_debugger(DebugSessionOptions {
                break_on_start: true,
            })
            .unwrap();
        let handle = vm.handle();
        let worker = controller(&vm, move || {
            let (entry, _, _) = paused(&debug);
            resume(&debug, entry);
            let mut events = Events::default();
            let p = stop(&debug, &mut events, PauseReason::DebuggerStatement, &[]);
            ask(
                &debug,
                DebugRequest::ContinueTo {
                    pause: p,
                    location: target(&debug, 4),
                },
            );
            entered.recv_timeout(Duration::from_secs(10)).unwrap();
            if stopping {
                handle.request_stop().unwrap();
            } else {
                debug.detach().unwrap();
            }
            release.send(()).unwrap();
            loop {
                match update(&debug) {
                    DebugUpdate::SessionClosed { .. } => break,
                    DebugUpdate::Paused { .. } => panic!("temporary marker survived termination"),
                    _ => {}
                }
            }
            debug
        });
        let result = vm.evaluate(source(
            "globalThis.reached=0;\ndebugger;\ngate();\nreached++;\nreached++;\ndebugger;",
        ));
        let _debug = worker.join().unwrap();
        if stopping {
            assert!(matches!(result, Err(VmError::Stopped)));
        } else {
            drop(result.unwrap());
            assert_eq!(vm.debug_state(), DebugExecutionState::Detached);
        }
    }
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    vm.bind_function(
        JsString::from_utf8("replaceSource"),
        0,
        std::rc::Rc::new(ReplaceSource),
    )
    .unwrap();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let worker = controller(&vm, move || {
        let (entry, _, _) = paused(&debug);
        resume(&debug, entry);
        let mut events = Events::default();
        let p = stop(&debug, &mut events, PauseReason::DebuggerStatement, &[]);
        let old = target(&debug, 3);
        ask(
            &debug,
            DebugRequest::ContinueTo {
                pause: p,
                location: old.clone(),
            },
        );
        let p = stop(&debug, &mut events, PauseReason::DebuggerStatement, &[]);
        assert_eq!(value(&debug, p, "replaced"), "1");
        assert_eq!(value(&debug, p, "reached"), "1");
        let id = send(
            &debug,
            DebugRequest::ContinueTo {
                pause: p,
                location: old,
            },
        );
        assert!(matches!(reply(&debug, id), Err(DebugError::InvalidSource)));
        resume(&debug, p);
        debug
    });
    let result = vm.evaluate(source(
        "globalThis.reached=0;\ndebugger;\nreplaceSource();\nreached++;\ndebugger;",
    ));
    let _debug = worker.join().unwrap();
    drop(result.unwrap());
}
