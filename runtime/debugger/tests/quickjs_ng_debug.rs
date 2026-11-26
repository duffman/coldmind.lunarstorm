//! Development debugger acceptance (spec §12 D-rows) against native QuickJS (production Rust compiler).
//!
//! The VM owner runs JavaScript on the test thread; a controller thread drives
//! the session through `VmDebugHandle`, as an IDE frontend would.
#![cfg(feature = "quickjs-ng")]
use coldmind_runtime::{debug::*, engine::*, engines::quickjs_ng::QuickJsNgEngine, vm::*};
use std::{
    thread,
    time::{Duration, Instant},
};

type Vm = VirtualMachine<QuickJsNgEngine>;

fn development() -> Vm {
    VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap()
}
#[path = "support/debugger.rs"]
mod debugger;
use debugger::*;

fn source(text: &str, origin: &str) -> ScriptSource {
    ScriptSource::new(JsString::from_utf8(text), origin.into())
}
fn utf8(value: &JsString) -> String {
    value.to_utf8().unwrap()
}
fn top(debug: &VmDebugHandle, pause: PauseId) -> DebugFrame {
    match ask(
        debug,
        DebugRequest::Stack {
            pause,
            page: Page { start: 0, count: 1 },
        },
    ) {
        DebugReply::Stack { mut frames, .. } => frames.remove(0),
        other => panic!("{other:?}"),
    }
}
fn variables(debug: &VmDebugHandle, parent: VariableParent) -> Vec<DebugVariable> {
    match ask(
        debug,
        DebugRequest::Variables {
            parent,
            page: Page {
                start: 0,
                count: 64,
            },
        },
    ) {
        DebugReply::Variables { variables, .. } => variables,
        other => panic!("{other:?}"),
    }
}
fn local_scope(debug: &VmDebugHandle, frame: FrameRef) -> ScopeRef {
    match ask(debug, DebugRequest::Scopes { frame }) {
        DebugReply::Scopes(scopes) => {
            scopes
                .into_iter()
                .find(|s| utf8(&s.name) == "Local")
                .unwrap()
                .id
        }
        other => panic!("{other:?}"),
    }
}
fn find<'a>(vars: &'a [DebugVariable], name: &str) -> &'a DebugValue {
    &vars
        .iter()
        .find(|v| utf8(&v.name) == name)
        .unwrap_or_else(|| panic!("no {name}"))
        .value
}
fn resume(debug: &VmDebugHandle, pause: PauseId, action: DebugContinue) {
    debug.continue_execution(pause, action).unwrap();
}
fn number(vm: &mut Vm, program: &str) -> f64 {
    let root = vm.evaluate(source(program, "file:///probe.js")).unwrap();
    let out = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = out.clone();
    vm.execute(Box::new(move |scope: &dyn JsScope| {
        let value = scope.local(&root).map_err(|e| scope.detach(e))?;
        *slot.borrow_mut() = Some(
            scope
                .export_data(value, ExportLimits::default())
                .map_err(|e| scope.detach(e))?,
        );
        Ok(())
    }))
    .unwrap();
    match out.borrow_mut().take().unwrap() {
        JsData::Number(n) => n,
        other => panic!("{other:?}"),
    }
}

#[test]
fn d01_d16_development_default_advertises_baseline_and_verified_inspection_and_evaluation() {
    let vm = development();
    let descriptor = vm.debug_descriptor().unwrap();
    assert!(
        descriptor
            .capabilities
            .contains_all(DebugCapabilities::baseline())
    );
    assert!(
        descriptor
            .capabilities
            .contains(DebugCapability::VariableMutation)
    );
    assert!(
        descriptor
            .capabilities
            .contains(DebugCapability::ColumnBreakpoints)
    );
    assert!(
        descriptor
            .capabilities
            .contains(DebugCapability::ExceptionDetails)
    );
    assert!(
        descriptor
            .capabilities
            .contains(DebugCapability::AsyncStacks)
    );
    assert!(
        descriptor
            .capabilities
            .contains(DebugCapability::UnhandledRejections)
    );
    for optional in [
        DebugCapability::ReadOnlyEvaluation,
        DebugCapability::LiveEdit,
        DebugCapability::WorkerTargets,
    ] {
        assert!(!descriptor.capabilities.contains(optional));
    }
    assert!(
        descriptor
            .capabilities
            .contains(DebugCapability::LazyInspection)
    );
    assert!(
        descriptor
            .capabilities
            .contains(DebugCapability::RichInspection)
    );
    assert!(
        descriptor
            .capabilities
            .contains(DebugCapability::CancelEvaluation)
    );
    assert_eq!(vm.debug_state(), DebugExecutionState::Detached);
}

#[test]
fn d02_break_on_start_then_pending_breakpoint_verifies_and_hits() {
    let mut vm = development();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let origin = "file:///d02.js";
    let id = send(
        &debug,
        DebugRequest::SetBreakpoints {
            selector: ScriptSelector {
                origin: origin.into(),
                expected_digest: None,
            },
            breakpoints: vec![BreakpointSpec {
                client_id: 9,
                line: 2,
                column: None,
                condition: None,
                enabled: true,
                hit_condition: None,
                log_message: None,
            }],
        },
    );
    let controller = controller(&vm, move || {
        let DebugReply::Breakpoints(infos) = reply(&debug, id).unwrap() else {
            panic!()
        };
        assert!(
            matches!(infos[0].state, BreakpointState::Pending),
            "source not loaded yet"
        );
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Entry);
        resume(&debug, pause, DebugContinue::Resume);
        let (pause, reason, location) = paused(&debug);
        assert_eq!(reason, PauseReason::Breakpoint);
        assert_eq!(location.unwrap().line, 2);
        resume(&debug, pause, DebugContinue::Resume);
        debug
    });
    let value = vm
        .evaluate(source("let a = 1;\nlet b = 2;\nlet c = a + b;\nc", origin))
        .unwrap();
    let debug = controller.join().unwrap();
    drop(value);
    assert_eq!(debug.state(), DebugExecutionState::Running);
}

#[test]
fn d03_d08_pause_reaches_busy_javascript_and_frame_evaluation_ends_it() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let controller = controller(&vm, move || {
        let (entry, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::DebuggerStatement);
        resume(&debug, entry, DebugContinue::Resume);
        // Continue is asynchronous. A pause requested before the owner has
        // actually resumed is correctly coalesced with the existing pause.
        while !matches!(update(&debug), DebugUpdate::Resumed { .. }) {}
        debug.request_pause().unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Requested);
        let frame = top(&debug, pause);
        let DebugReply::Evaluation { value, exception } = ask(
            &debug,
            DebugRequest::Evaluate {
                frame: frame.id,
                expression: JsString::from_utf8("spin = false"),
                allow_side_effects: true,
            },
        ) else {
            panic!()
        };
        assert!(!exception);
        assert_eq!(utf8(&value.preview), "false");
        // Side-effect-free evaluation is not advertised; admission rejects it.
        let read_only = debug
            .try_request(DebugRequest::Evaluate {
                frame: frame.id,
                expression: JsString::from_utf8("1"),
                allow_side_effects: false,
            })
            .unwrap_err();
        assert_eq!(
            read_only.reason,
            RejectionReason::Debug(DebugError::Unsupported)
        );
        resume(&debug, pause, DebugContinue::Resume);
        debug
    });
    let n = number(
        &mut vm,
        "globalThis.spin = true; let n = 0; debugger; while (spin) n++; 1",
    );
    controller.join().unwrap();
    assert_eq!(n, 1.0);
}

#[test]
fn d05_d09_stepping_across_calls_invalidates_old_references() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let program = "function g(x) {\n  return x * 2;\n}\nfunction f() {\n  debugger;\n  let y = g(3);\n  return y + 1;\n}\nf();";
    let controller = controller(&vm, move || {
        let (pause, reason, location) = paused(&debug);
        assert_eq!(reason, PauseReason::DebuggerStatement);
        assert_eq!(location.unwrap().line, 4);
        let old = top(&debug, pause);
        resume(&debug, pause, DebugContinue::StepOver);
        let (pause, reason, location) = paused(&debug);
        assert_eq!(reason, PauseReason::Step);
        assert_eq!(location.unwrap().line, 5);
        // References from the previous pause are rejected at admission.
        let stale = debug
            .try_request(DebugRequest::Scopes { frame: old.id })
            .unwrap_err();
        assert_eq!(stale.reason, RejectionReason::Debug(DebugError::StalePause));
        resume(&debug, pause, DebugContinue::StepInto);
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        assert_eq!(utf8(&frame.name), "g");
        let locals = variables(&debug, VariableParent::Scope(local_scope(&debug, frame.id)));
        assert_eq!(utf8(&find(&locals, "x").preview), "3");
        resume(&debug, pause, DebugContinue::StepOut);
        let (pause, _, _) = paused(&debug);
        assert_eq!(utf8(&top(&debug, pause).name), "f");
        resume(&debug, pause, DebugContinue::Resume);
        debug
    });
    drop(vm.evaluate(source(program, "file:///d05.js")).unwrap());
    controller.join().unwrap();
}

#[test]
fn d06_exception_policy_distinguishes_caught_and_uncaught() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let id = send(&debug, DebugRequest::ExceptionPolicy(ExceptionPolicy::All));
    let controller = controller(&vm, move || {
        assert!(matches!(
            reply(&debug, id).unwrap(),
            DebugReply::ExceptionPolicy(ExceptionPolicy::All)
        ));
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        resume(&debug, pause, DebugContinue::Resume);
        debug
    });
    assert_eq!(
        number(&mut vm, "let r = 0; try { throw 7 } catch (e) { r = e } r"),
        7.0
    );
    let debug = controller.join().unwrap();
    let id = send(
        &debug,
        DebugRequest::ExceptionPolicy(ExceptionPolicy::Uncaught),
    );
    assert_eq!(
        number(&mut vm, "let s = 0; try { throw 8 } catch (e) { s = e } s"),
        8.0
    );
    assert!(reply(&debug, id).is_ok());
    assert!(
        !matches!(debug.try_recv(), Some(DebugUpdate::Paused { .. })),
        "caught throw must not pause"
    );
}

#[test]
fn d07_d15_object_pages_are_safe_and_never_run_getters_or_traps() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let program = "globalThis.touched = 0;\n\
        (function () {\n\
          const o = { a: 1, nested: { b: 2 }, get boom() { touched++; return 0 },\n\
                      p: new Proxy({}, { ownKeys() { touched++; return [] } }) };\n\
          debugger;\n\
          return o.a;\n\
        })();";
    let controller = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let locals = variables(&debug, VariableParent::Scope(local_scope(&debug, frame.id)));
        let object = find(&locals, "o").object.expect("expandable object");
        let members = variables(&debug, VariableParent::Object(object));
        assert_eq!(utf8(&find(&members, "a").preview), "1");
        assert!(find(&members, "boom").accessor);
        let proxy = find(&members, "p");
        assert_eq!(utf8(&proxy.preview), "Proxy");
        assert!(proxy.unavailable.is_some() && proxy.object.is_none());
        let nested = variables(
            &debug,
            VariableParent::Object(find(&members, "nested").object.unwrap()),
        );
        assert_eq!(utf8(&find(&nested, "b").preview), "2");
        resume(&debug, pause, DebugContinue::Resume);
        debug
    });
    drop(vm.evaluate(source(program, "file:///d07.js")).unwrap());
    controller.join().unwrap();
    assert_eq!(number(&mut vm, "touched"), 0.0);
}

#[test]
fn d12_detach_while_paused_resumes_by_default() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let controller = controller(&vm, move || {
        paused(&debug);
        debug.detach().unwrap();
        loop {
            if let DebugUpdate::SessionClosed { reason, .. } = update(&debug) {
                assert!(reason.is_none(), "normal detach must not report a fault");
                break;
            }
        }
    });
    assert_eq!(number(&mut vm, "debugger; 5"), 5.0);
    controller.join().unwrap();
    assert_eq!(vm.debug_state(), DebugExecutionState::Detached);
    assert_eq!(
        number(&mut vm, "debugger; 6"),
        6.0,
        "unattached debugger statements do not pause"
    );
}

#[test]
fn d13_backend_retained_across_shutdown_keeps_descriptor_and_rejects_signals() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    vm.shutdown().unwrap();
    assert_eq!(debug.state(), DebugExecutionState::Closed);
    assert_eq!(debug.request_pause().unwrap_err(), DebugError::Closed);
    assert!(
        debug
            .descriptor()
            .capabilities
            .contains_all(DebugCapabilities::baseline())
    );
}

#[test]
fn d17_d19_dynamic_sources_register_before_execution() {
    let mut vm = development();
    // Unattached registration (D19): runs without a client.
    assert_eq!(number(&mut vm, "eval('1 + 1')"), 2.0);
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    assert_eq!(
        number(&mut vm, "eval('2 + 2') + new Function('return 1')()"),
        5.0
    );
    let mut origins = Vec::new();
    while let Some(update) = debug.try_recv() {
        if let DebugUpdate::ScriptRegistered(info) = update {
            origins.push(info.origin);
        }
    }
    assert!(origins.iter().any(|o| o.contains("eval")), "{origins:?}");
    assert!(
        origins.iter().any(|o| o.contains("function")),
        "{origins:?}"
    );
    let id = send(
        &debug,
        DebugRequest::Sources {
            page: Page {
                start: 0,
                count: 256,
            },
        },
    );
    assert_eq!(number(&mut vm, "0"), 0.0);
    let DebugReply::Sources { scripts, .. } = reply(&debug, id).unwrap() else {
        panic!()
    };
    assert!(scripts.iter().any(|s| s.origin == "file:///probe.js"));
}

#[test]
fn caller_block_inspection_and_frame_eval_agree_after_a_branch() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let controller = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let DebugReply::Stack { frames, .. } = ask(
            &debug,
            DebugRequest::Stack {
                pause,
                page: Page {
                    start: 0,
                    count: 16,
                },
            },
        ) else {
            panic!("expected stack")
        };
        let caller = frames
            .iter()
            .find(|f| utf8(&f.name) == "outer")
            .expect("caller frame");
        let DebugReply::Scopes(scopes) = ask(&debug, DebugRequest::Scopes { frame: caller.id })
        else {
            panic!("expected scopes")
        };
        let visible: Vec<_> = scopes
            .iter()
            .flat_map(|scope| variables(&debug, VariableParent::Scope(scope.id)))
            .collect();
        assert!(
            visible
                .iter()
                .any(|v| utf8(&v.name) == "x" && utf8(&v.value.preview) == "20"),
            "shadowed caller x must be inspectable"
        );
        let DebugReply::Evaluation { value, exception } = ask(
            &debug,
            DebugRequest::Evaluate {
                frame: caller.id,
                expression: JsString::from_utf8("x = 42"),
                allow_side_effects: true,
            },
        ) else {
            panic!("expected evaluation")
        };
        assert!(!exception);
        assert_eq!(utf8(&value.preview), "42");
        resume(&debug, pause, DebugContinue::Resume);
    });
    assert_eq!(
        number(
            &mut vm,
            "function inner(){debugger;} function outer(flag){let x=10; {let x=20; if(flag){x+=0;} inner(); return x;}} outer(true)"
        ),
        42.0
    );
    controller.join().unwrap();
    assert_eq!(
        number(&mut vm, "typeof x === 'undefined' ? 1 : 0"),
        1.0,
        "selected-frame assignment must not leak into global scope"
    );
}

#[test]
fn uncaught_primitive_survives_debugger_evaluator_exception() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let policy = send(
        &debug,
        DebugRequest::ExceptionPolicy(ExceptionPolicy::Uncaught),
    );
    let controller = controller(&vm, move || {
        assert!(reply(&debug, policy).is_ok());
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        let frame = top(&debug, pause);
        let DebugReply::Evaluation { exception, .. } = ask(
            &debug,
            DebugRequest::Evaluate {
                frame: frame.id,
                expression: JsString::from_utf8("throw 99"),
                allow_side_effects: true,
            },
        ) else {
            panic!("expected evaluator result")
        };
        assert!(
            exception,
            "the evaluator's throw must be reported separately"
        );
        resume(&debug, pause, DebugContinue::Resume);
    });
    let error = vm
        .evaluate(source("throw 73", "file:///uncaught.js"))
        .unwrap_err();
    controller.join().unwrap();
    let VmError::Execution(report) = error else {
        panic!("expected application exception: {error:?}")
    };
    assert_eq!(
        report.thrown,
        Some(JsData::Number(73.0)),
        "debugger eval must preserve the original thrown value"
    );
}

/// A dynamic source arriving after the debugger's script budget is full still runs.
///
/// Decision (c4ccc05): the debugger budget never limits the application. This test used to
/// assert the opposite, that the `eval` was refused and its body never ran; that expectation
/// was replaced on the user's approval. The host script takes the only registration, so the
/// `eval` body compiles unregistered, and it must run and not throw.
#[test]
fn dynamic_source_past_the_script_budget_runs_unregistered() {
    let mut config = VmConfig::default();
    config.debug_limits.scripts = 1;
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    assert_eq!(
        number(
            &mut vm,
            "globalThis.touched=0; let threw=0; try { eval('touched=1') } catch(e) { threw=1 } touched*10+threw"
        ),
        10.0
    );
}

#[test]
fn stop_requested_during_pause_never_resumes_application_code() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let stop = vm.handle();
    let controller = controller(&vm, move || {
        paused(&debug);
        stop.request_stop().unwrap();
    });
    let result = vm.evaluate(source(
        "debugger; throw 'application must not resume'",
        "file:///stop.js",
    ));
    controller.join().unwrap();
    assert!(
        matches!(result, Err(VmError::Stopped)),
        "stop must win over application continuation: {result:?}"
    );
}

#[test]
fn uncaught_policy_reports_once_through_finally_only_handlers() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let policy = send(
        &debug,
        DebugRequest::ExceptionPolicy(ExceptionPolicy::Uncaught),
    );
    let worker = controller(&vm, move || {
        assert!(reply(&debug, policy).is_ok());
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        resume(&debug, pause, DebugContinue::Resume);
        let mut pauses = 1;
        let deadline = Instant::now() + Duration::from_secs(10);
        while debug.state() != DebugExecutionState::Closed && Instant::now() < deadline {
            if let Some(DebugUpdate::Paused { pause, .. }) = debug.try_recv() {
                pauses += 1;
                resume(&debug, pause, DebugContinue::Resume);
            }
            thread::yield_now();
        }
        assert_eq!(
            pauses, 1,
            "implicit finally rethrow is the same propagation"
        );
    });
    let error = vm
        .evaluate(source(
            "function f(){try{throw 73;}finally{globalThis.cleaned=true;}} f();",
            "file:///finally.js",
        ))
        .unwrap_err();
    worker.join().unwrap();
    let VmError::Execution(report) = error else {
        panic!("expected uncaught exception")
    };
    assert_eq!(report.thrown, Some(JsData::Number(73.0)));
}

#[test]
fn uncaught_policy_ignores_finally_when_an_outer_user_catch_handles_it() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let policy = send(
        &debug,
        DebugRequest::ExceptionPolicy(ExceptionPolicy::Uncaught),
    );
    // Process queued policy before evaluation. Any incorrect pause is resumed so
    // this test reports a count mismatch rather than relying on a timeout.
    let worker = controller(&vm, move || {
        assert!(reply(&debug, policy).is_ok());
        let mut pauses = 0;
        loop {
            let (pause, reason, _) = paused(&debug);
            if reason == PauseReason::DebuggerStatement {
                resume(&debug, pause, DebugContinue::Resume);
                break;
            }
            pauses += 1;
            resume(&debug, pause, DebugContinue::Resume);
        }
        assert_eq!(pauses, 0, "outer catch handles the exception after finally");
    });
    assert_eq!(
        number(
            &mut vm,
            "function f(){try{throw 73;}finally{globalThis.cleaned=true;}} let result=0;try{f();}catch(e){result=e;}debugger;result"
        ),
        73.0
    );
    worker.join().unwrap();
}

fn dap_message(seq: u64, command: &str) -> Vec<u8> {
    let body = serde_json::to_vec(&serde_json::json!({"seq":seq,"type":"request","command":command,"arguments":{"threadId":1}})).unwrap();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend(body);
    out
}
fn dap_json(frame: &[u8]) -> serde_json::Value {
    let start = frame.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
    serde_json::from_slice(&frame[start..]).unwrap()
}
#[test]
fn dap_configuration_entry_pause_and_continue_use_the_native_backend() {
    let mut vm = development();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let mut dap = DapAdapter::new(debug).unwrap();
    let (ready, configured) = std::sync::mpsc::sync_channel(1);
    let worker = controller(&vm, move || {
        dap.receive(&dap_message(1, "initialize")).unwrap();
        let mut frames = dap.receive(&dap_message(2, "configurationDone")).unwrap();
        ready.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !frames.iter().any(|f| dap_json(f)["event"] == "stopped") {
            assert!(
                Instant::now() < deadline,
                "DAP never published native pause"
            );
            frames.extend(dap.poll().unwrap());
            thread::yield_now();
        }
        let stopped = frames
            .iter()
            .map(|f| dap_json(f))
            .find(|f| f["event"] == "stopped")
            .unwrap();
        assert_eq!(stopped["body"]["reason"], "pause");
        dap.receive(&dap_message(3, "continue")).unwrap();
        dap
    });
    configured.recv_timeout(Duration::from_secs(10)).unwrap();
    // DAP consumes/continues Entry as its configuration gate; a real debugger
    // statement after that gate must still stop through the native backend.
    assert_eq!(number(&mut vm, "debugger;6*7"), 42.0);
    let _dap = worker.join().unwrap();
}

#[test]
fn urgent_stop_is_admitted_with_all_normal_reply_permits_retained() {
    let mut config = VmConfig::default();
    config.debug_limits.requests = 1;
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let stop = vm.handle();
    let worker = controller(&vm, move || {
        paused(&debug);
        let request = || DebugRequest::Sources {
            page: Page { start: 0, count: 1 },
        };
        // Deliberately do not consume the reply: the single request permit stays
        // reserved even if the owner already serviced this command.
        send(&debug, request());
        assert!(
            debug.try_request(request()).is_err(),
            "normal admission must be full"
        );
        stop.request_stop().unwrap();
    });
    let result = vm.evaluate(source("debugger; while(true){}", "file:///full-queue.js"));
    worker.join().unwrap();
    assert!(
        matches!(result, Err(VmError::Stopped)),
        "urgent stop must bypass normal permits: {result:?}"
    );
}

#[test]
fn oversized_property_inspection_reports_capacity_without_running_getters() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let locals = variables(&debug, VariableParent::Scope(local_scope(&debug, frame.id)));
        let object = find(&locals, "large")
            .object
            .expect("ordinary object is expandable");
        let id = send(
            &debug,
            DebugRequest::Variables {
                parent: VariableParent::Object(object),
                page: Page { start: 0, count: 1 },
            },
        );
        assert!(
            matches!(reply(&debug, id), Err(DebugError::Capacity)),
            "oversized native enumeration must report Capacity, never an empty successful object"
        );
        resume(&debug, pause, DebugContinue::Resume);
    });
    assert_eq!(
        number(
            &mut vm,
            "globalThis.getterHits=0; (function(){const large={get forbidden(){getterHits++;return 0;}}; for(let i=0;i<65537;i++){large['k'+i]=i;} debugger; return getterHits;})()"
        ),
        0.0
    );
    worker.join().unwrap();
    assert_eq!(number(&mut vm, "getterHits"), 0.0);
}

#[test]
fn stepping_revisits_the_same_marker_on_the_next_loop_iteration() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        resume(&debug, pause, DebugContinue::StepOver);
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Step);
        // One-statement loop body: successive executions have the same marker.
        resume(&debug, pause, DebugContinue::StepInto);
        let (pause, reason, first) = paused(&debug);
        assert_eq!(reason, PauseReason::Step);
        resume(&debug, pause, DebugContinue::StepOver);
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Step);
        // Advance through the loop body, then revisit the condition on the
        // next iteration. Those two condition locations must agree.
        resume(&debug, pause, DebugContinue::StepOver);
        let (pause, reason, next) = paused(&debug);
        assert_eq!(reason, PauseReason::Step);
        assert_eq!(
            first
                .as_ref()
                .map(|p| (p.script, p.revision, p.line, p.column)),
            next.as_ref()
                .map(|p| (p.script, p.revision, p.line, p.column)),
            "same body marker is revisited on iteration two"
        );
        let frame = top(&debug, pause);
        let DebugReply::Evaluation { value, exception } = ask(
            &debug,
            DebugRequest::Evaluate {
                frame: frame.id,
                expression: JsString::from_utf8("i"),
                allow_side_effects: true,
            },
        ) else {
            panic!()
        };
        assert!(!exception);
        assert_eq!(utf8(&value.preview), "1");
        resume(&debug, pause, DebugContinue::Resume);
    });
    assert_eq!(number(&mut vm, "let i=0;debugger;while(i<3)i++;i"), 3.0);
    worker.join().unwrap();
}

#[test]
fn caught_throw_in_finally_does_not_repeat_the_original_exception_event() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let policy = send(&debug, DebugRequest::ExceptionPolicy(ExceptionPolicy::All));
    let worker = controller(&vm, move || {
        assert!(reply(&debug, policy).is_ok());
        let mut count = 0;
        loop {
            match update(&debug) {
                DebugUpdate::Paused { pause, reason, .. } => {
                    assert_eq!(reason, PauseReason::Exception);
                    count += 1;
                    resume(&debug, pause, DebugContinue::Resume);
                }
                DebugUpdate::SessionClosed { .. } => break,
                _ => {}
            }
        }
        assert_eq!(
            count, 2,
            "one event for 7 and one for the caught 8, never another for 7"
        );
    });
    let error = vm
        .evaluate(source(
            "try {throw 7;} finally {try{throw 8;}catch(e){}}",
            "file:///nested-finally.js",
        ))
        .unwrap_err();
    worker.join().unwrap();
    let VmError::Execution(report) = error else {
        panic!("expected original exception")
    };
    assert_eq!(report.thrown, Some(JsData::Number(7.0)));
}

#[test]
fn conditional_breakpoint_only_pauses_when_its_expression_is_true() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let configured = send(
        &debug,
        DebugRequest::SetBreakpoints {
            selector: ScriptSelector {
                origin: "file:///condition.js".into(),
                expected_digest: None,
            },
            breakpoints: vec![BreakpointSpec {
                client_id: 1,
                line: 2,
                column: None,
                condition: Some(JsString::from_utf8("i === 2")),
                enabled: true,
                hit_condition: None,
                log_message: None,
            }],
        },
    );
    let worker = controller(&vm, move || {
        assert!(reply(&debug, configured).is_ok());
        let (pause, reason, location) = paused(&debug);
        assert_eq!(reason, PauseReason::Breakpoint);
        assert_eq!(location.unwrap().line, 2);
        let frame = top(&debug, pause);
        let DebugReply::Evaluation { value, exception } = ask(
            &debug,
            DebugRequest::Evaluate {
                frame: frame.id,
                expression: JsString::from_utf8("i"),
                allow_side_effects: true,
            },
        ) else {
            panic!()
        };
        assert!(!exception);
        assert_eq!(utf8(&value.preview), "2");
        resume(&debug, pause, DebugContinue::Resume);
        // Detach after the sole matching iteration; the program still completes.
        debug.detach().unwrap();
    });
    drop(
        vm.evaluate(source(
            "let total=0;\nfor(let i=0;i<3;i++){\n total+=i;\n}\ntotal",
            "file:///condition.js",
        ))
        .unwrap(),
    );
    worker.join().unwrap();
}

#[test]
fn source_map_breakpoint_resolves_to_native_generated_statement() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let configured = send(
        &debug,
        DebugRequest::SetBreakpoints {
            selector: ScriptSelector {
                origin: "file:///original.ts".into(),
                expected_digest: None,
            },
            breakpoints: vec![BreakpointSpec {
                client_id: 5,
                line: 1,
                column: None,
                condition: None,
                enabled: true,
                hit_condition: None,
                log_message: None,
            }],
        },
    );
    let worker = controller(&vm, move || {
        assert!(reply(&debug, configured).is_ok());
        let (pause, reason, location) = paused(&debug);
        assert_eq!(reason, PauseReason::Breakpoint);
        assert_eq!(location.unwrap().line, 1);
        resume(&debug, pause, DebugContinue::Resume);
    });
    let mut script = source("let mapped=40;\nmapped+=2;\nmapped", "file:///generated.js");
    script.source_map = Some(SourceMapAttachment { json: br#"{"version":3,"file":"generated.js","sources":["original.ts"],"sourcesContent":["let mapped: number = 40;\nmapped += 2;\nmapped"],"names":[],"mappings":"AAAA;AACA;AACA"}"#.to_vec(), originals: vec![], preceding_maps: vec![] });
    drop(vm.evaluate(script).unwrap());
    worker.join().unwrap();
}

#[test]
fn debug_artifact_retains_breakpoints_across_serialization_and_loading() {
    let mut vm = development();
    let script = source(
        "let artifactValue=40;\nartifactValue+=2;\nartifactValue",
        "file:///artifact-debug.js",
    );
    let artifact = vm
        .compile(script, QuickJsNgEngine::debug_compile_options())
        .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let configured = send(
        &debug,
        DebugRequest::SetBreakpoints {
            selector: ScriptSelector {
                origin: "file:///artifact-debug.js".into(),
                expected_digest: None,
            },
            breakpoints: vec![BreakpointSpec {
                client_id: 7,
                line: 1,
                column: None,
                condition: None,
                enabled: true,
                hit_condition: None,
                log_message: None,
            }],
        },
    );
    let worker = controller(&vm, move || {
        assert!(reply(&debug, configured).is_ok());
        let (pause, reason, location) = paused(&debug);
        assert_eq!(reason, PauseReason::Breakpoint);
        assert_eq!(location.unwrap().line, 1);
        resume(&debug, pause, DebugContinue::Resume);
    });
    drop(vm.execute_artifact(artifact).unwrap());
    worker.join().unwrap();
}

struct EvalEntered(std::sync::mpsc::Sender<()>);
impl HostFunction for EvalEntered {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.0.send(()).unwrap();
        call.import_data(JsData::Undefined)
    }
}
#[test]
fn stop_interrupts_an_evaluator_after_it_has_started_running() {
    let mut vm = development();
    let (started, entered) = std::sync::mpsc::channel();
    let _root = vm
        .bind_function(
            JsString::from_utf8("evalEntered"),
            0,
            std::rc::Rc::new(EvalEntered(started)),
        )
        .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let stop = vm.handle();
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let _evaluation = send(
            &debug,
            DebugRequest::Evaluate {
                frame: frame.id,
                expression: JsString::from_utf8("evalEntered(); while(true) {}"),
                allow_side_effects: true,
            },
        );
        entered
            .recv_timeout(Duration::from_secs(10))
            .expect("evaluator reached host checkpoint");
        stop.request_stop().unwrap();
    });
    let result = vm.evaluate(source(
        "debugger;throw 'application must not resume'",
        "file:///evaluator-stop.js",
    ));
    worker.join().unwrap();
    assert!(
        matches!(result, Err(VmError::Stopped)),
        "stop must unwind busy evaluator: {result:?}"
    );
}

#[test]
fn long_variable_and_property_names_remain_distinct() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let prefix = "x".repeat(300);
    let first = format!("{prefix}A");
    let second = format!("{prefix}B");
    let program = format!(
        "(function(){{let {first}=17;let {second}=29;let o={{{first}:41,{second}:53}};debugger;return {first}+{second};}})()"
    );
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let locals = variables(&debug, VariableParent::Scope(local_scope(&debug, frame.id)));
        assert_eq!(utf8(&find(&locals, &first).preview), "17");
        assert_eq!(utf8(&find(&locals, &second).preview), "29");
        let members = variables(
            &debug,
            VariableParent::Object(find(&locals, "o").object.unwrap()),
        );
        assert_eq!(utf8(&find(&members, &first).preview), "41");
        assert_eq!(utf8(&find(&members, &second).preview), "53");
        resume(&debug, pause, DebugContinue::Resume);
        debug
    });
    assert_eq!(number(&mut vm, &program), 46.0);
    worker.join().unwrap();
}

#[test]
fn symbol_properties_are_inspected_without_symbol_coercion_or_getters() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let locals = variables(&debug, VariableParent::Scope(local_scope(&debug, frame.id)));
        let members = variables(
            &debug,
            VariableParent::Object(find(&locals, "object").object.unwrap()),
        );
        assert_eq!(utf8(&find(&members, "a").preview), "1");
        assert_eq!(utf8(&find(&members, "[Symbol(s)]").preview), "2");
        assert_eq!(utf8(&find(&members, "[Symbol()]").preview), "3");
        assert!(find(&members, "[Symbol(accessor)]").accessor);
        resume(&debug, pause, DebugContinue::Resume);
        debug
    });
    assert_eq!(
        number(
            &mut vm,
            r#"
        globalThis.symbolHooks=0;
        Symbol.prototype.toString=function(){symbolHooks++;throw 1;};
        Object.defineProperty(Symbol.prototype,'description',{
            get(){symbolHooks++;throw 2;}
        });
        (function(){
            let object={a:1,[Symbol('s')]:2,[Symbol()]:3,
                get [Symbol('accessor')](){symbolHooks++;throw 3;}};
            debugger;
            return symbolHooks;
        })()
    "#
        ),
        0.0
    );
    worker.join().unwrap();
}

#[test]
fn dropping_detaching_handles_never_stops_application_execution() {
    // Dropping a handle issues a second detach while the owner may be closing
    // the native session. Exercise that ordering repeatedly without waiting for
    // SessionClosed to hide the concurrent signal-revocation window.
    for _ in 0..32 {
        let mut vm = development();
        let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
        let worker = controller(&vm, move || {
            paused(&debug);
            debug.detach().unwrap();
            drop(debug);
        });
        assert_eq!(number(&mut vm, "debugger; 5"), 5.0);
        worker.join().unwrap();
        assert_eq!(vm.debug_state(), DebugExecutionState::Detached);
        assert_eq!(number(&mut vm, "6"), 6.0);
    }
}

struct QueuePausedCallback;
impl HostFunction for QueuePausedCallback {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let job = CallbackJob {
            function: call.retain(call.arguments()[0])?,
            receiver: None,
            arguments: Vec::new(),
        };
        call.host().post(job).map_err(|_| {
            FailureReport::fault(FailureCode::Capacity, "paused callback admission failed")
        })?;
        call.import_data(JsData::Undefined)
    }
}

#[test]
fn d04_pause_and_debugger_evaluation_do_not_run_queued_jobs_or_microtasks() {
    let mut vm = development();
    let _post = vm
        .bind_function(
            JsString::from_utf8("post"),
            1,
            std::rc::Rc::new(QueuePausedCallback),
        )
        .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        for expression in [
            "ordinary * 100 + microtasks",
            // Explicit evaluation may enqueue native Promise jobs, but must not
            // checkpoint them while the application frame is suspended.
            "Promise.resolve().then(()=>microtasks++); ordinary * 100 + microtasks",
            "ordinary * 100 + microtasks",
        ] {
            let DebugReply::Evaluation { value, exception } = ask(
                &debug,
                DebugRequest::Evaluate {
                    frame: frame.id,
                    expression: JsString::from_utf8(expression),
                    allow_side_effects: true,
                },
            ) else {
                panic!("expected evaluation reply")
            };
            assert!(!exception);
            assert_eq!(utf8(&value.preview), "0");
        }
        resume(&debug, pause, DebugContinue::Resume);
        debug
    });
    drop(vm.evaluate(source(
        "globalThis.ordinary=0;globalThis.microtasks=0;post(()=>ordinary++);Promise.resolve().then(()=>microtasks++);debugger;",
        "file:///paused-jobs.js",
    )).unwrap());
    let _debug = worker.join().unwrap();
    vm.tick(false).unwrap();
    assert_eq!(number(&mut vm, "ordinary * 100 + microtasks"), 102.0);
}

#[test]
fn pause_columns_count_utf16_units_after_astral_text() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let prefix = "let astral='😀'; ";
    let column = prefix.encode_utf16().count() as u32;
    let worker = controller(&vm, move || {
        let (pause, reason, location) = paused(&debug);
        assert_eq!(reason, PauseReason::DebuggerStatement);
        let location = location.unwrap();
        assert_eq!(location.line, 0);
        assert_eq!(location.column, Some(column));
        resume(&debug, pause, DebugContinue::Resume);
        debug
    });
    drop(
        vm.evaluate(source(
            &format!("{prefix}debugger;40+2"),
            "file:///utf16-columns.js",
        ))
        .unwrap(),
    );
    worker.join().unwrap();
}

#[test]
fn stop_during_pause_skips_application_catch_and_finally() {
    let mut vm = development();
    let (touch, touched) = std::sync::mpsc::channel();
    let _probe = vm
        .bind_function(
            JsString::from_utf8("touched"),
            0,
            std::rc::Rc::new(EvalEntered(touch)),
        )
        .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let stop = vm.handle();
    let controller = controller(&vm, move || {
        paused(&debug);
        stop.request_stop().unwrap();
    });
    let result = vm.evaluate(source(
        "try { debugger } catch (e) { touched() } finally { touched() }",
        "file:///stop-catch.js",
    ));
    controller.join().unwrap();
    assert!(
        matches!(result, Err(VmError::Stopped)),
        "stop must end the program: {result:?}"
    );
    assert!(
        touched.try_recv().is_err(),
        "stop must not run the application's catch or finally"
    );
}

#[test]
fn stop_at_an_exception_pause_skips_the_application_catch() {
    let mut vm = development();
    let (touch, touched) = std::sync::mpsc::channel();
    let _probe = vm
        .bind_function(
            JsString::from_utf8("touched"),
            0,
            std::rc::Rc::new(EvalEntered(touch)),
        )
        .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let id = send(&debug, DebugRequest::ExceptionPolicy(ExceptionPolicy::All));
    let stop = vm.handle();
    let controller = controller(&vm, move || {
        assert!(reply(&debug, id).is_ok());
        let (_, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        stop.request_stop().unwrap();
    });
    let result = vm.evaluate(source(
        "try { throw 1 } catch (e) { touched() } finally { touched() }",
        "file:///stop-exception.js",
    ));
    controller.join().unwrap();
    assert!(
        matches!(result, Err(VmError::Stopped)),
        "stop at an exception pause must end the program: {result:?}"
    );
    assert!(
        touched.try_recv().is_err(),
        "stop at an exception pause must not run the application's catch or finally"
    );
}

#[test]
fn number_previews_use_javascript_number_formatting() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let controller = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let locals = variables(&debug, VariableParent::Scope(local_scope(&debug, frame.id)));
        let shown: Vec<String> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|name| utf8(&find(&locals, name).preview))
            .collect();
        resume(&debug, pause, DebugContinue::Resume);
        shown
    });
    drop(
        vm.evaluate(source(
            "(function () { const a = Infinity, b = -Infinity, c = 1e21, d = 1e-7, e = 123.5; debugger; return a + b + c + d + e })();",
            "file:///numbers.js",
        ))
        .unwrap(),
    );
    assert_eq!(
        controller.join().unwrap(),
        ["Infinity", "-Infinity", "1e+21", "1e-7", "123.5"]
    );
}

#[test]
fn long_function_name_does_not_end_the_program_at_a_pause() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let controller = controller(&vm, move || {
        let (pause, reason, _) = paused(&debug);
        resume(&debug, pause, DebugContinue::Resume);
        reason
    });
    let name = "f".repeat(70_000);
    let result = vm.evaluate(source(
        &format!("function {name}() {{ debugger; return 5 }} {name}()"),
        "file:///long-name.js",
    ));
    assert!(
        result.is_ok(),
        "a debugger-internal failure must not end the program: {result:?}"
    );
    assert_eq!(controller.join().unwrap(), PauseReason::DebuggerStatement);
}

/// Resumes every pause until `done` fires, then reports how many there were.
fn count_pauses(vm: &Vm, debug: VmDebugHandle) -> (Controller<usize>, std::sync::mpsc::Sender<()>) {
    let (done, finished) = std::sync::mpsc::channel();
    let controller = controller(vm, move || {
        let mut pauses = 0;
        while finished.try_recv().is_err() {
            match debug.try_recv() {
                Some(DebugUpdate::Paused { pause, .. }) => {
                    pauses += 1;
                    resume(&debug, pause, DebugContinue::Resume);
                }
                Some(_) => {}
                None => thread::yield_now(),
            }
        }
        pauses
    });
    (controller, done)
}

#[test]
fn uncaught_policy_does_not_pause_for_async_rejections_a_caller_catches() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    send(
        &debug,
        DebugRequest::ExceptionPolicy(ExceptionPolicy::Uncaught),
    );
    let (controller, done) = count_pauses(&vm, debug);
    drop(
        vm.evaluate(source(
            "globalThis.r = 0;\n\
             async function late() { await null; throw 7 }\n\
             async function early() { throw 8 }\n\
             (async () => {\n\
               try { await late() } catch (e) { r += e }\n\
               try { await early() } catch (e) { r += e }\n\
               try { await late().then(() => 0) } catch (e) { r += e }\n\
               await late().catch((e) => { r += e });\n\
             })();",
            "file:///async-caught.js",
        ))
        .unwrap(),
    );
    vm.tick(false).unwrap();
    assert_eq!(number(&mut vm, "r"), 29.0);
    done.send(()).unwrap();
    assert_eq!(
        controller.join().unwrap(),
        0,
        "every rejection reaches a catch, so the uncaught policy must not pause"
    );
}

#[test]
fn uncaught_policy_pauses_for_an_async_throw_the_callers_try_cannot_catch() {
    let mut vm = development();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    send(
        &debug,
        DebugRequest::ExceptionPolicy(ExceptionPolicy::Uncaught),
    );
    let (controller, done) = count_pauses(&vm, debug);
    // The throw rejects g's promise, which nothing handles; the caller's try never sees it.
    let _ = vm.evaluate(source(
        "async function g() { throw 9 } try { g() } catch (e) {}",
        "file:///async-uncaught.js",
    ));
    done.send(()).unwrap();
    assert_eq!(controller.join().unwrap(), 1);
}
