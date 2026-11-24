//! Step policies and continuation behavior through the real compiler/native VM.
#![cfg(feature = "quickjs-ng")]
use coldmind_runtime::{debug::*, engine::*, engines::quickjs_ng::QuickJsNgEngine, vm::*};
use std::{rc::Rc, sync::mpsc, time::Duration};
#[path = "support/debugger.rs"]
mod debugger;
use debugger::*;

struct Gate {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}
impl HostFunction for Gate {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.entered.send(()).unwrap();
        self.release.recv_timeout(Duration::from_secs(10)).unwrap();
        call.import_data(JsData::Undefined)
    }
}

#[test]
fn user_pause_and_breakpoints_override_skip() {
    const URI: &str = "file:///library.js";
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    vm.bind_function(
        JsString::from_utf8("gate"),
        0,
        Rc::new(Gate {
            entered,
            release: released,
        }),
    )
    .unwrap();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let worker = controller(&vm, move || {
        let (entry, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Entry);
        assert!(matches!(
            ask(
                &debug,
                DebugRequest::StepPolicy(StepPolicy {
                    skip_sources: vec!["file:///**".into()],
                })
            ),
            DebugReply::StepPolicy
        ));
        ask(
            &debug,
            DebugRequest::SetBreakpoints {
                selector: ScriptSelector {
                    origin: URI.into(),
                    expected_digest: None,
                },
                breakpoints: vec![BreakpointSpec {
                    client_id: 1,
                    line: 1,
                    ..Default::default()
                }],
            },
        );
        debug
            .continue_execution(entry, DebugContinue::StepInto)
            .unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Breakpoint);
        debug
            .continue_execution(pause, DebugContinue::StepInto)
            .unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::DebuggerStatement);
        debug
            .continue_execution(pause, DebugContinue::StepInto)
            .unwrap();
        waiting.recv_timeout(Duration::from_secs(10)).unwrap();
        debug.request_pause().unwrap();
        release.send(()).unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Requested);
        ask(&debug, DebugRequest::StepPolicy(StepPolicy::default()));
        debug
            .continue_execution(pause, DebugContinue::StepInto)
            .unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(
            reason,
            PauseReason::Step,
            "clearing rules must invalidate cached skips"
        );
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    let result = vm.evaluate(ScriptSource::new(
        JsString::from_utf8(
            "let count=0;\ncount++;\ndebugger;\ngate();\ncount++;\nif(count!==2)throw 'counter';",
        ),
        URI.into(),
    ));
    let _debug = worker.join().unwrap();
    drop(result.unwrap());
}

#[test]
fn skip_rules_follow_original_sources_within_one_bundle() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let worker = controller(&vm, move || {
        let (entry, _, _) = paused(&debug);
        ask(
            &debug,
            DebugRequest::StepPolicy(StepPolicy {
                skip_sources: vec!["**/vendor.ts".into()],
            }),
        );
        debug
            .continue_execution(entry, DebugContinue::StepInto)
            .unwrap();
        let (pause, reason, location) = paused(&debug);
        assert_eq!(reason, PauseReason::Step);
        assert_eq!(
            location.unwrap().line,
            2,
            "vendor marker must be skipped, app marker must remain visible"
        );
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    let mut source = ScriptSource::new(
        JsString::from_utf8("let a=0;\na++;\na++;"),
        "file:///bundle.js".into(),
    );
    source.source_map = Some(SourceMapAttachment {
        json: serde_json::to_vec(&serde_json::json!({"version":3,"sections":[
            {"offset":{"line":0,"column":0},"map":{"version":3,"sources":["app.ts"],"sourcesContent":["let a: number=0;\na++;"],"mappings":"AAAA"}},
            {"offset":{"line":1,"column":0},"map":{"version":3,"sources":["vendor.ts"],"sourcesContent":["a++;"],"mappings":"AAAA"}},
            {"offset":{"line":2,"column":0},"map":{"version":3,"sources":["app.ts"],"sourcesContent":["let a: number=0;\na++;"],"mappings":"AACA"}}
        ]})).unwrap(), originals: vec![], preceding_maps: vec![],
    });
    let result = vm.evaluate(source);
    let _debug = worker.join().unwrap();
    drop(result.unwrap());
}

fn top(debug: &VmDebugHandle, pause: PauseId) -> DebugFrame {
    let DebugReply::Stack { mut frames, .. } = ask(
        debug,
        DebugRequest::Stack {
            pause,
            page: Page { start: 0, count: 1 },
        },
    ) else {
        panic!("stack");
    };
    frames.remove(0)
}
fn lookup(debug: &VmDebugHandle, frame: FrameRef, path: &str) -> String {
    let DebugReply::Lookup(value) = ask(
        debug,
        DebugRequest::LookupPath {
            frame,
            path: JsString::from_utf8(path),
        },
    ) else {
        panic!("lookup");
    };
    value.preview.to_utf8().unwrap()
}

#[test]
fn step_over_await_follows_selected_continuation() {
    for action in [DebugContinue::StepOver, DebugContinue::StepInto] {
        let mut vm =
            VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
        let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
        let worker = controller(&vm, move || {
            let (pause, _, _) = paused(&debug);
            debug.continue_execution(pause, action).unwrap();
            let (pause, reason, location) = paused(&debug);
            assert_eq!(reason, PauseReason::Step);
            assert_eq!(location.unwrap().line, 5);
            debug.continue_execution(pause, action).unwrap();
            let (pause, reason, location) = paused(&debug);
            assert_eq!(reason, PauseReason::Step);
            assert_eq!(
                location.unwrap().line,
                6,
                "unrelated jobs must not complete selected await step"
            );
            let frame = top(&debug, pause);
            assert_eq!(frame.name.to_utf8().unwrap(), "selected");
            assert_eq!(lookup(&debug, frame.id, "stage"), "1");
            debug
                .continue_execution(pause, DebugContinue::Resume)
                .unwrap();
            debug
        });
        let result = vm.evaluate(ScriptSource::new(
            JsString::from_utf8(concat!(
                "globalThis.stage=0;\n",
                "let release;\n",
                "let pending=new Promise(resolve=>release=resolve);\n",
                "async function selected(){\n",
                "debugger;\n",
                "await pending;\n",
                "stage=2;\n",
                "}\n",
                "selected();\n",
                "Promise.resolve().then(()=>{stage=1;release();});"
            )),
            "file:///await-step.js".into(),
        ));
        let tick = if result.is_ok() {
            Some(vm.tick(false))
        } else {
            None
        };
        let _debug = worker.join().unwrap();
        drop(result.unwrap());
        tick.unwrap().unwrap();
        drop(
            vm.evaluate(ScriptSource::new(
                JsString::from_utf8("if(stage!==2)throw 'selected did not resume';"),
                "file:///await-check.js".into(),
            ))
            .unwrap(),
        );
    }
}

#[test]
fn step_out_async_follows_logical_caller() {
    for generator in [false, true] {
        let mut vm =
            VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
        let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
        let worker = controller(&vm, move || {
            let (pause, _, _) = paused(&debug);
            debug
                .continue_execution(pause, DebugContinue::StepOut)
                .unwrap();
            let (pause, reason, location) = paused(&debug);
            assert_eq!(reason, PauseReason::Step);
            assert_eq!(location.unwrap().line, 9);
            let frame = top(&debug, pause);
            assert_eq!(frame.name.to_utf8().unwrap(), "parent");
            assert_eq!(lookup(&debug, frame.id, "stage"), "1");
            assert_eq!(lookup(&debug, frame.id, "result"), "3");
            debug
                .continue_execution(pause, DebugContinue::Resume)
                .unwrap();
            debug
        });
        let result = vm.evaluate(ScriptSource::new(
            JsString::from_utf8(
                &concat!(
                    "globalThis.stage=0;\n",
                    "async function child(){\n",
                    "debugger;\n",
                    "await null;\n",
                    "stage=1;\n",
                    "return 3;\n",
                    "}\n",
                    "async function parent(){\n",
                    "const result=await child();\n",
                    "globalThis.answer=result;\n",
                    "}\n",
                    "parent();"
                )
                .replace(
                    "async function parent()",
                    if generator {
                        "async function* parent()"
                    } else {
                        "async function parent()"
                    },
                )
                .replace(
                    "parent();",
                    if generator {
                        "parent().next();"
                    } else {
                        "parent();"
                    },
                ),
            ),
            "file:///async-out.js".into(),
        ));
        let tick = if result.is_ok() {
            Some(vm.tick(false))
        } else {
            None
        };
        let _debug = worker.join().unwrap();
        drop(result.unwrap());
        tick.unwrap().unwrap();
        drop(
            vm.evaluate(ScriptSource::new(
                JsString::from_utf8("if(answer!==3)throw 'parent did not resume';"),
                "file:///async-out-check.js".into(),
            ))
            .unwrap(),
        );
    }
}

#[test]
fn yield_steps_back_to_live_next_caller() {
    const URI: &str = "file:///generator-step.js";
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        debug
            .continue_execution(pause, DebugContinue::StepOver)
            .unwrap();
        let (pause, _, location) = paused(&debug);
        assert_eq!(location.unwrap().line, 2);
        debug
            .continue_execution(pause, DebugContinue::StepOver)
            .unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Step);
        assert_eq!(top(&debug, pause).name.to_utf8().unwrap(), "first");
        ask(
            &debug,
            DebugRequest::SetBreakpoints {
                selector: ScriptSelector {
                    origin: URI.into(),
                    expected_digest: None,
                },
                breakpoints: vec![BreakpointSpec {
                    client_id: 1,
                    line: 11,
                    ..Default::default()
                }],
            },
        );
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Breakpoint);
        ask(
            &debug,
            DebugRequest::SetBreakpoints {
                selector: ScriptSelector {
                    origin: URI.into(),
                    expected_digest: None,
                },
                breakpoints: vec![],
            },
        );
        debug
            .continue_execution(pause, DebugContinue::StepInto)
            .unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Step);
        assert_eq!(top(&debug, pause).name.to_utf8().unwrap(), "gen");
        debug
            .continue_execution(pause, DebugContinue::StepOut)
            .unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Step);
        assert_eq!(top(&debug, pause).name.to_utf8().unwrap(), "second");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    let result = vm.evaluate(ScriptSource::new(
        JsString::from_utf8(concat!(
            "function* gen(){\n",
            "debugger;\n",
            "yield 1;\n",
            "globalThis.resumed=1;\n",
            "return 2;\n",
            "}\n",
            "function first(){\n",
            "const value=iterator.next();\n",
            "globalThis.firstValue=value.value;\n",
            "}\n",
            "function second(){\n",
            "const value=iterator.next();\n",
            "globalThis.secondValue=value.value;\n",
            "}\n",
            "let iterator=gen();\n",
            "first();\n",
            "second();\n",
            "if(firstValue!==1||secondValue!==2||resumed!==1)throw 'generator result';"
        )),
        URI.into(),
    ));
    let _debug = worker.join().unwrap();
    drop(result.unwrap());
}

#[test]
fn step_into_from_async_enters_child_before_suspension() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        debug
            .continue_execution(pause, DebugContinue::StepOver)
            .unwrap();
        let (pause, _, location) = paused(&debug);
        assert_eq!(location.unwrap().line, 6);
        debug
            .continue_execution(pause, DebugContinue::StepInto)
            .unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Step);
        assert_eq!(top(&debug, pause).name.to_utf8().unwrap(), "child");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    let result = vm.evaluate(ScriptSource::new(
        JsString::from_utf8(concat!(
            "function child(){\n",
            "globalThis.answer=42;\n",
            "}\n",
            "async function parent(){\n",
            "await null;\n",
            "debugger;\n",
            "child();\n",
            "}\n",
            "parent();"
        )),
        "file:///async-into.js".into(),
    ));
    let tick = if result.is_ok() {
        Some(vm.tick(false))
    } else {
        None
    };
    let _debug = worker.join().unwrap();
    drop(result.unwrap());
    tick.unwrap().unwrap();
    drop(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8("if(answer!==42)throw 'answer';"),
            "file:///check.js".into(),
        ))
        .unwrap(),
    );
}

#[test]
fn async_generator_yield_and_return_follow_awaiting_next_caller() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        debug
            .continue_execution(pause, DebugContinue::StepOver)
            .unwrap();
        let (pause, _, location) = paused(&debug);
        assert_eq!(location.unwrap().line, 2);
        debug
            .continue_execution(pause, DebugContinue::StepOver)
            .unwrap();
        let (pause, _, location) = paused(&debug);
        assert_eq!(location.unwrap().line, 3);
        debug
            .continue_execution(pause, DebugContinue::StepOver)
            .unwrap();
        let (pause, reason, location) = paused(&debug);
        assert_eq!(reason, PauseReason::Step);
        assert_eq!(location.unwrap().line, 10);
        assert_eq!(top(&debug, pause).name.to_utf8().unwrap(), "parent");
        debug
            .continue_execution(pause, DebugContinue::StepOver)
            .unwrap();
        let (pause, _, location) = paused(&debug);
        assert_eq!(location.unwrap().line, 11);
        debug
            .continue_execution(pause, DebugContinue::StepInto)
            .unwrap();
        let (pause, _, _) = paused(&debug);
        assert_eq!(top(&debug, pause).name.to_utf8().unwrap(), "gen");
        debug
            .continue_execution(pause, DebugContinue::StepOut)
            .unwrap();
        let (pause, reason, location) = paused(&debug);
        assert_eq!(reason, PauseReason::Step);
        assert_eq!(location.unwrap().line, 12);
        assert_eq!(top(&debug, pause).name.to_utf8().unwrap(), "parent");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    let result = vm.evaluate(ScriptSource::new(
        JsString::from_utf8(concat!(
            "async function* gen(){\n",
            "debugger;\n",
            "await null;\n",
            "yield 1;\n",
            "await null;\n",
            "return 2;\n",
            "}\n",
            "async function parent(){\n",
            "const iterator=gen();\n",
            "const first=await iterator.next();\n",
            "globalThis.firstValue=first.value;\n",
            "const second=await iterator.next();\n",
            "globalThis.secondValue=second.value;\n",
            "}\n",
            "parent();"
        )),
        "file:///async-generator.js".into(),
    ));
    let tick = if result.is_ok() {
        Some(vm.tick(false))
    } else {
        None
    };
    let _debug = worker.join().unwrap();
    drop(result.unwrap());
    tick.unwrap().unwrap();
    drop(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8("if(firstValue!==1||secondValue!==2)throw 'generator results';"),
            "file:///check.js".into(),
        ))
        .unwrap(),
    );
}
