//! Immutable scheduling ancestry through production compilation and native jobs.
#![cfg(feature = "quickjs-ng")]
use coldmind_runtime::{debug::*, engine::*, engines::quickjs_ng::QuickJsNgEngine, vm::*};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    rc::Rc,
    thread,
    time::{Duration, Instant},
};
#[path = "support/debugger.rs"]
mod debugger;
use debugger::*;

fn history(debug: &VmDebugHandle, pause: PauseId) -> AsyncStackTrace {
    match ask(debug, DebugRequest::AsyncStack { pause }) {
        DebugReply::AsyncStack(trace) => trace,
        other => panic!("unexpected history reply: {other:?}"),
    }
}

#[test]
fn await_and_promise_ancestry_points_to_registration_site() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let trace = history(&debug, pause);
        assert!(!trace.truncated);
        assert_eq!(trace.entries.len(), 2);
        assert_eq!(trace.entries[0].record.label.to_utf8().unwrap(), "await");
        assert_eq!(trace.entries[0].record.locations[0].line, 2);
        assert_eq!(
            trace.entries[1].record.label.to_utf8().unwrap(),
            "Promise.then"
        );
        assert_eq!(trace.entries[1].record.locations[0].line, 0);
        assert!(trace.entries.iter().all(|entry| !entry.record.truncated));
        assert_eq!(trace.entries[0].record.parent, Some(trace.entries[1].id));
        assert!(trace.retained_records <= 1024);
        assert!(trace.retained_bytes <= 8 * 1024 * 1024);
        // Reading again does not consume or change the immutable history.
        let again = history(&debug, pause);
        assert_eq!(again.entries[0].id, trace.entries[0].id);
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    let result = vm.evaluate(ScriptSource::new(
        JsString::from_utf8(concat!(
            "Promise.resolve().then(function registered(){\n",
            "return (async function selected(){\n",
            "await null;\n",
            "debugger;\n",
            "})();\n",
            "});"
        )),
        "file:///async-history.js".into(),
    ));
    let tick = if result.is_ok() {
        Some(vm.tick(false))
    } else {
        None
    };
    let _debug = worker.join().unwrap();
    drop(result.unwrap());
    tick.unwrap().unwrap();
}

struct Queue(AsyncScheduleKind);
impl HostFunction for Queue {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let callback = call.arguments()[0];
        if matches!(self.0, AsyncScheduleKind::Microtask) {
            call.enqueue_microtask(callback)?;
        } else {
            let job = CallbackJob {
                function: call.retain(callback)?,
                receiver: None,
                arguments: Vec::new(),
            };
            match self.0 {
                AsyncScheduleKind::Timer => {
                    call.host().timer(Duration::ZERO, None, job).unwrap();
                }
                AsyncScheduleKind::Callback => {
                    call.host().post(job).unwrap();
                }
                _ => unreachable!(),
            }
        }
        call.import_data(JsData::Undefined)
    }
}

#[test]
fn microtask_callback_and_timer_ancestry_points_to_admission_site() {
    for (kind, label) in [
        (AsyncScheduleKind::Microtask, "microtask"),
        (AsyncScheduleKind::Callback, "callback"),
        (AsyncScheduleKind::Timer, "timer"),
    ] {
        let mut vm =
            VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
        vm.bind_function(JsString::from_utf8("queue"), 1, Rc::new(Queue(kind)))
            .unwrap();
        let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
        let worker = controller(&vm, move || {
            let (pause, _, _) = paused(&debug);
            let trace = history(&debug, pause);
            assert!(!trace.truncated);
            assert_eq!(trace.entries.len(), 1);
            assert_eq!(trace.entries[0].record.label.to_utf8().unwrap(), label);
            assert_eq!(trace.entries[0].record.locations[0].line, 3);
            debug
                .continue_execution(pause, DebugContinue::Resume)
                .unwrap();
            debug
        });
        let result = vm.evaluate(ScriptSource::new(
            JsString::from_utf8(concat!(
                "function callback(){\n",
                "debugger;\n",
                "}\n",
                "queue(callback);"
            )),
            "file:///scheduled-history.js".into(),
        ));
        let tick = if result.is_ok() {
            Some(vm.tick(false))
        } else {
            None
        };
        let _debug = worker.join().unwrap();
        drop(result.unwrap());
        tick.unwrap().unwrap();
    }
}

#[test]
fn history_bounds_truncate_without_changing_application_jobs() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let trace = history(&debug, pause);
        assert!(trace.truncated);
        assert!(trace.entries.len() <= 32);
        assert_eq!(trace.retained_records, 1024);
        assert!(trace.retained_bytes <= 8 * 1024 * 1024);
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    let result = vm.evaluate(ScriptSource::new(JsString::from_utf8(
        "async function chain(){for(let i=0;i<2048;i++)await null;debugger;globalThis.complete=true;}chain();"
    ), "file:///bounded-history.js".into()));
    let tick = if result.is_ok() {
        Some((|| -> Result<(), VmError> {
            while vm.tick(false)?.made_progress {}
            Ok(())
        })())
    } else {
        None
    };
    let _debug = worker.join().unwrap();
    drop(result.unwrap());
    tick.unwrap().unwrap();
    drop(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8("if(!complete)throw 'lost work';"),
            "file:///check-history.js".into(),
        ))
        .unwrap(),
    );
}

struct Context(bool);
impl HostFunction for Context {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        if self.0 {
            call.set_async_context(call.arguments()[0])?;
            call.import_data(JsData::Undefined)
        } else {
            call.async_context()
        }
    }
}
#[test]
fn application_async_context_is_unchanged() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    vm.bind_function(JsString::from_utf8("setContext"), 1, Rc::new(Context(true)))
        .unwrap();
    vm.bind_function(
        JsString::from_utf8("getContext"),
        0,
        Rc::new(Context(false)),
    )
    .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        assert_eq!(history(&debug, pause).entries.len(), 2);
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    let result = vm.evaluate(ScriptSource::new(
        JsString::from_utf8(concat!(
            "setContext('outer');Promise.resolve().then(()=>{",
            "if(getContext()!=='outer')throw 'outer';setContext('inner');",
            "Promise.resolve().then(()=>{if(getContext()!=='inner')throw 'inner';debugger;});});"
        )),
        "file:///application-context.js".into(),
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
            JsString::from_utf8("if(getContext()!=='outer')throw 'context restoration';"),
            "file:///check-context.js".into(),
        ))
        .unwrap(),
    );
}

// Real framed DAP over the production VM; notifications stay queued while replies correlate.
struct Client {
    adapter: DapAdapter,
    inbox: Vec<Value>,
    seq: u64,
}
impl Client {
    fn collect(&mut self, frames: Vec<Vec<u8>>) {
        self.inbox.extend(frames.into_iter().map(|wire| {
            let start = wire.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
            serde_json::from_slice(&wire[start..]).unwrap()
        }));
    }
    fn take(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(index) = self.inbox.iter().position(&predicate) {
                return self.inbox.remove(index);
            }
            assert!(Instant::now() < deadline, "DAP timeout: {:?}", self.inbox);
            let frames = self.adapter.poll().unwrap();
            self.collect(frames);
            thread::yield_now();
        }
    }
    fn request(&mut self, command: &str, arguments: Value) -> Value {
        self.seq += 1;
        let seq = self.seq;
        let body = serde_json::to_vec(
            &json!({"seq":seq,"type":"request","command":command,"arguments":arguments}),
        )
        .unwrap();
        let mut wire = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        wire.extend(body);
        let frames = self.adapter.receive(&wire).unwrap();
        self.collect(frames);
        self.take(|value| value["type"] == "response" && value["request_seq"] == seq)
    }
}

#[test]
fn dap_history_is_mapped_and_refuses_historical_evaluation() {
    for lines_one in [false, true] {
        for columns_one in [false, true] {
            let mut vm =
                VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
            let debug = vm
                .attach_debugger(DebugSessionOptions {
                    break_on_start: true,
                })
                .unwrap();
            let mut client = Client {
                adapter: DapAdapter::new(debug).unwrap(),
                inbox: Vec::new(),
                seq: 0,
            };
            let init = client.request(
                "initialize",
                json!({"linesStartAt1":lines_one,"columnsStartAt1":columns_one}),
            );
            assert_eq!(init["body"]["jetbunny"]["supportsAsyncStack"], true);
            let worker = controller(&vm, move || {
                assert_eq!(
                    client.request("configurationDone", json!({}))["success"],
                    true
                );
                client.take(|value| value["event"] == "stopped");
                let response = client.request("jetbunny/asyncStack", json!({"threadId":1}));
                assert_eq!(response["success"], true, "{response}");
                let entries = response["body"]["entries"].as_array().unwrap();
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0]["label"], "await");
                assert_eq!(entries[0]["readOnly"], true);
                assert_eq!(entries[0]["frames"][0]["line"], 6 + u32::from(lines_one));
                assert_eq!(entries[0]["frames"][0]["column"], u32::from(columns_one));
                assert_eq!(
                    entries[0]["frames"][0]["source"]["path"],
                    "/async-original.ts"
                );
                let id = entries[0]["id"].clone();
                assert!(id.as_str().unwrap().starts_with("history:"));
                let refused = client.request(
                    "evaluate",
                    json!({"frameId":id,"expression":"effects++","context":"repl"}),
                );
                assert_eq!(refused["success"], false);
                assert_eq!(
                    client.request("continue", json!({"threadId":1}))["success"],
                    true
                );
                client
            });
            let code = "Promise.resolve().then(async function work(){\nawait null;\ndebugger;\nif(effects)throw 'historical evaluation executed';});";
            let mut source = ScriptSource::new(
                JsString::from_utf8(code),
                "file:///async-generated.js".into(),
            );
            // Keep the setup separate so mapped scheduling coordinates remain exact.
            drop(
                vm.evaluate(ScriptSource::new(
                    JsString::from_utf8("globalThis.effects=0;"),
                    "file:///setup.js".into(),
                ))
                .unwrap(),
            );
            source.source_map = Some(SourceMapAttachment {
            json: serde_json::to_vec(&json!({"version":3,"sources":["async-original.ts"],"sourcesContent":[format!("{}{}", "\n".repeat(5), code)],"names":[],"mappings":"AAKA;AACA;AACA;AACA"})).unwrap(),
            originals:Vec::new(),preceding_maps:Vec::new(),
        });
            let result = vm.evaluate(source);
            let tick = if result.is_ok() {
                Some(vm.tick(false))
            } else {
                None
            };
            let _client = worker.join().unwrap();
            drop(result.unwrap());
            tick.unwrap().unwrap();
            drop(
                vm.evaluate(ScriptSource::new(
                    JsString::from_utf8("if(effects!==0)throw 'historical evaluation executed';"),
                    "file:///check-effects.js".into(),
                ))
                .unwrap(),
            );
        }
    }
}

#[test]
fn mapped_history_reply_capacity_keeps_pause_usable() {
    let mut config = VmConfig::default();
    config.debug_limits.reply_bytes = 1024;
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let id = send(&debug, DebugRequest::AsyncStack { pause });
        assert!(matches!(reply(&debug, id), Err(DebugError::Capacity)));
        assert_eq!(debug.state(), DebugExecutionState::Paused(pause));
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    let code = "Promise.resolve().then(async function work(){\nawait null;\ndebugger;\nglobalThis.complete=true;});";
    let mut source = ScriptSource::new(JsString::from_utf8(code), "file:///large-origin.js".into());
    source.source_map=Some(SourceMapAttachment {
        json:serde_json::to_vec(&json!({"version":3,"sources":[format!("{}.ts", "x".repeat(2048))],"sourcesContent":[code],"names":[],"mappings":"AAAA;AACA;AACA;AACA"})).unwrap(),
        originals:Vec::new(),preceding_maps:Vec::new(),
    });
    let result = vm.evaluate(source);
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
            JsString::from_utf8("if(!complete)throw 'lost pause';"),
            "file:///check-capacity.js".into(),
        ))
        .unwrap(),
    );
}

struct Bind(Rc<RefCell<Option<CompletionTicket>>>);
impl HostFunction for Bind {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let reservation = call.host().reserve_operation(128)?;
        let ticket =
            call.host()
                .bind_callback(reservation, call.retain(call.arguments()[0])?, None)?;
        *self.0.borrow_mut() = Some(ticket);
        call.import_data(JsData::Undefined)
    }
}
#[test]
fn reserved_callback_ancestry_points_to_binding_site() {
    let ticket = Rc::new(RefCell::new(None));
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    vm.bind_function(
        JsString::from_utf8("begin"),
        1,
        Rc::new(Bind(ticket.clone())),
    )
    .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let trace = history(&debug, pause);
        assert_eq!(trace.entries.len(), 1);
        assert_eq!(trace.entries[0].record.label.to_utf8().unwrap(), "callback");
        assert_eq!(trace.entries[0].record.locations[0].line, 3);
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    let result=vm.evaluate(ScriptSource::new(JsString::from_utf8("function callback(error,value){\ndebugger;globalThis.answer=value;\n}\nbegin(callback);"),"file:///reserved-history.js".into()));
    assert!(matches!(
        ticket
            .borrow_mut()
            .take()
            .unwrap()
            .complete(CompletionPayload::Success(JsData::Number(42.0))),
        PublishOutcome::Published
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
            JsString::from_utf8("if(answer!==42)throw 'completion changed';"),
            "file:///check-reserved-history.js".into(),
        ))
        .unwrap(),
    );
}

fn rejection_updates(debug: &VmDebugHandle) -> Vec<PromiseRejectionInfo> {
    let mut updates = Vec::new();
    while let Some(update) = debug.try_recv() {
        if let DebugUpdate::PromiseRejection(info) = update {
            updates.push(info);
        }
    }
    updates
}
#[test]
fn later_handler_changes_rejection_state_once() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    drop(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8("globalThis.pending=Promise.reject('original');"),
            "file:///rejected.js".into(),
        ))
        .unwrap(),
    );
    // evaluate may itself drain a checkpoint; preserve its qualified event.
    let mut unhandled = rejection_updates(&debug);
    vm.tick(false).unwrap();
    unhandled.extend(rejection_updates(&debug));
    assert_eq!(unhandled.len(), 1);
    assert_eq!(unhandled[0].state, PromiseRejectionState::Unhandled);
    drop(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8("pending.catch(()=>{});"),
            "file:///handled-later.js".into(),
        ))
        .unwrap(),
    );
    vm.tick(false).unwrap();
    let handled = rejection_updates(&debug);
    assert_eq!(handled.len(), 1);
    assert_eq!(handled[0].id, unhandled[0].id);
    assert_eq!(handled[0].state, PromiseRejectionState::HandledLater);
    drop(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8("pending.catch(()=>{});"),
            "file:///handled-again.js".into(),
        ))
        .unwrap(),
    );
    vm.tick(false).unwrap();
    assert!(rejection_updates(&debug).is_empty());
}

#[test]
fn same_checkpoint_handler_suppresses_unhandled_with_small_job_budget() {
    let mut config = VmConfig::default();
    config.scheduler.microtask_hint = std::num::NonZeroUsize::new(1).unwrap();
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    drop(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8(concat!(
                "let rejected=Promise.reject('handled');let later=Promise.resolve();",
                "for(let i=0;i<8;i++)later=later.then(()=>{});",
                "later.then(()=>rejected.catch(()=>{globalThis.handled=true;}));"
            )),
            "file:///same-checkpoint.js".into(),
        ))
        .unwrap(),
    );
    assert!(rejection_updates(&debug).is_empty());
    while vm.tick(false).unwrap().made_progress {
        assert!(
            rejection_updates(&debug).is_empty(),
            "partial checkpoint must not qualify pending rejections"
        );
    }
    assert!(rejection_updates(&debug).is_empty());
    drop(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8("if(!handled)throw 'handler did not run';"),
            "file:///check-handling.js".into(),
        ))
        .unwrap(),
    );
}

#[test]
fn rejection_original_location_and_reason_survive_historical_pause() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    vm.bind_function(JsString::from_utf8("rawEvents"), 0, Rc::new(DrainRaw))
        .unwrap();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let worker = controller(&vm, move || {
        let (entry, _, _) = paused(&debug);
        assert!(matches!(
            ask(&debug, DebugRequest::UnhandledRejections(true)),
            DebugReply::UnhandledRejections
        ));
        debug
            .continue_execution(entry, DebugContinue::Resume)
            .unwrap();
        let (pause, reason, location) = paused(&debug);
        assert_eq!(reason, PauseReason::UnhandledRejection);
        assert_eq!(
            location.unwrap().line,
            2,
            "preserve throw site, not the async call site"
        );
        assert!(
            matches!(ask(&debug,DebugRequest::Stack {pause,page:Page {start:0,count:64}}),DebugReply::Stack {frames,total:0} if frames.is_empty())
        );
        assert!(matches!(
            debug.continue_execution(pause, DebugContinue::StepInto),
            Err(DebugError::Unsupported)
        ));
        let DebugReply::ExceptionInfo(info) = ask(&debug, DebugRequest::ExceptionInfo { pause })
        else {
            panic!("exception info")
        };
        assert!(!info.caught);
        assert_eq!(info.location.unwrap().line, 2);
        let object = info
            .value
            .object
            .expect("original object retained for safe inspection");
        let DebugReply::Inspection(page) = ask(
            &debug,
            DebugRequest::Inspect(InspectionRequest {
                parent: VariableParent::Object(object),
                cursor: None,
                page: Page {
                    start: 0,
                    count: 16,
                },
            }),
        ) else {
            panic!("reason properties")
        };
        let tag = page
            .items
            .iter()
            .find(|item| item.variable.name.to_utf8().unwrap() == "tag")
            .unwrap();
        assert_eq!(tag.variable.value.preview.to_utf8().unwrap(), "\"kept\"");
        assert!(
            page.items
                .iter()
                .find(|item| item.variable.name.to_utf8().unwrap() == "message")
                .unwrap()
                .variable
                .value
                .accessor
        );
        assert!(!history(&debug, pause).entries.is_empty());
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    let result = vm.evaluate(ScriptSource::new(
        JsString::from_utf8(concat!(
            "globalThis.effects=0;\n",
            "async function reject(){\n",
            "throw {tag:'kept',get message(){effects++;throw 'getter';}};\n",
            "}\n",
            "reject();rawEvents();for(let i=0;i<10000;i++)new Array(32);void 0;"
        )),
        "file:///rejection-origin.js".into(),
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
            JsString::from_utf8("if(effects)throw 'getter executed';"),
            "file:///check-rejection.js".into(),
        ))
        .unwrap(),
    );
}

#[test]
fn rejection_propagation_preserves_nested_finally_and_await_origins() {
    let cases = [
        "function inner(){\nthrow 'origin';\n}\nPromise.resolve().then(()=>inner()).then(()=>{}).then(()=>{});",
        "let pending;\npending=Promise.reject('origin');\nfor(let i=0;i<2048;i++)pending=pending.then();",
        "async function reject(){\ntry {throw 'origin';}finally{\ntry{throw 'caught';}catch(e){}\n}\n}\nreject();",
        "async function producer(){\nthrow 'origin';\n}\nasync function consumer(){\nawait producer();\n}\nconsumer();",
        "async function producer(){\nthrow 'origin';\n}\nasync function* consumer(){\nawait producer();\n}\nconsumer().next();",
    ];
    for code in cases {
        let mut vm =
            VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
        let debug = vm
            .attach_debugger(DebugSessionOptions {
                break_on_start: true,
            })
            .unwrap();
        let worker = controller(&vm, move || {
            let (entry, _, _) = paused(&debug);
            ask(&debug, DebugRequest::UnhandledRejections(true));
            debug
                .continue_execution(entry, DebugContinue::Resume)
                .unwrap();
            let (pause, reason, location) = paused(&debug);
            assert_eq!(reason, PauseReason::UnhandledRejection);
            assert_eq!(
                location.unwrap().line,
                1,
                "original throw must survive propagation: {code}"
            );
            debug
                .continue_execution(pause, DebugContinue::Resume)
                .unwrap();
            debug
        });
        let result = vm.evaluate(ScriptSource::new(
            JsString::from_utf8(code),
            "file:///propagation.js".into(),
        ));
        let tick = if result.is_ok() {
            Some(vm.tick(false))
        } else {
            None
        };
        let _debug = worker.join().unwrap();
        drop(result.unwrap());
        tick.unwrap().unwrap();
    }
}

#[test]
fn rejection_tracking_is_bounded_and_context_scoped() {
    let mut identities = Vec::new();
    for _ in 0..2 {
        let mut config = VmConfig::default();
        config.debug_limits.promise_rejections = 2;
        let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
        let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
        drop(
            vm.evaluate(ScriptSource::new(
                JsString::from_utf8("for(let i=0;i<8;i++)Promise.reject(i);"),
                "file:///same-rejections.js".into(),
            ))
            .unwrap(),
        );
        let updates = rejection_updates(&debug);
        assert_eq!(updates.len(), 2);
        identities.push(updates[0].id);
        let request = send(
            &debug,
            DebugRequest::PromiseRejections {
                page: Page {
                    start: 0,
                    count: 64,
                },
            },
        );
        vm.tick(false).unwrap();
        let DebugReply::PromiseRejections {
            records,
            total,
            dropped,
            retained_bytes,
        } = reply(&debug, request).unwrap()
        else {
            panic!("records")
        };
        assert_eq!(records.len(), 2);
        assert_eq!(total, 2);
        assert_eq!(dropped, 6);
        assert!(retained_bytes < 4096);
        assert!(
            records
                .iter()
                .all(|r| r.state == PromiseRejectionState::Unhandled)
        );
    }
    assert_ne!(
        identities[0], identities[1],
        "identical counters in separate heaps cannot alias"
    );
}

#[test]
fn dap_unhandled_filter_opens_read_only_historical_pause() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let mut client = Client {
        adapter: DapAdapter::new(debug).unwrap(),
        inbox: Vec::new(),
        seq: 0,
    };
    let init = client.request("initialize", json!({}));
    assert!(
        init["body"]["exceptionBreakpointFilters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["filter"] == "unhandledRejection")
    );
    let worker = controller(&vm, move || {
        assert_eq!(
            client.request(
                "setExceptionBreakpoints",
                json!({"filters":["unhandledRejection"]})
            )["success"],
            true
        );
        assert_eq!(
            client.request("configurationDone", json!({}))["success"],
            true
        );
        let stop = client.take(|v| v["event"] == "stopped");
        assert_eq!(stop["body"]["reason"], "exception");
        let info = client.request("exceptionInfo", json!({"threadId":1}));
        assert_eq!(info["success"], true);
        assert_eq!(info["body"]["breakMode"], "unhandled");
        assert!(
            info["body"]["description"]
                .as_str()
                .unwrap()
                .contains("boom")
        );
        let stack = client.request("stackTrace", json!({"threadId":1}));
        assert!(stack["body"]["stackFrames"].as_array().unwrap().is_empty());
        let records = client.request("jetbunny/promiseRejections", json!({}));
        assert_eq!(records["success"], true);
        assert_eq!(records["body"]["records"][0]["state"], "unhandled");
        assert_eq!(
            client.request(
                "evaluate",
                json!({"expression":"effects++","context":"repl"})
            )["success"],
            false
        );
        assert_eq!(
            client.request("continue", json!({"threadId":1}))["success"],
            true
        );
        client
    });
    drop(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8("globalThis.effects=0;"),
            "file:///setup.js".into(),
        ))
        .unwrap(),
    );
    let result = vm.evaluate(ScriptSource::new(
        JsString::from_utf8("Promise.resolve().then(()=>{throw new Error('boom');});"),
        "file:///dap-rejection.js".into(),
    ));
    let tick = if result.is_ok() {
        Some(vm.tick(false))
    } else {
        None
    };
    let _client = worker.join().unwrap();
    drop(result.unwrap());
    tick.unwrap().unwrap();
    drop(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8("if(effects)throw 'historical evaluation';"),
            "file:///check-dap-rejection.js".into(),
        ))
        .unwrap(),
    );
}

struct DrainRaw;
impl HostFunction for DrainRaw {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let mut count = 0;
        while call.take_rejection()?.is_some() {
            count += 1;
        }
        call.import_data(JsData::Number(count as f64))
    }
}
struct SetDrain;
impl HostFunction for SetDrain {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        call.host()
            .set_tick_drain(call.retain(call.arguments()[0])?)?;
        call.import_data(JsData::Undefined)
    }
}
#[test]
fn debugger_does_not_consume_application_raw_rejections() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    vm.bind_function(JsString::from_utf8("rawEvents"), 0, Rc::new(DrainRaw))
        .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    drop(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8(
                "Promise.reject('reason');if(rawEvents()!==1)throw 'raw tracker consumed';",
            ),
            "file:///raw-tracker.js".into(),
        ))
        .unwrap(),
    );
    assert_eq!(rejection_updates(&debug).len(), 1);
}
#[test]
fn next_tick_handler_after_native_drain_suppresses_unhandled() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    vm.bind_function(JsString::from_utf8("setDrain"), 1, Rc::new(SetDrain))
        .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    drop(vm.evaluate(ScriptSource::new(JsString::from_utf8(concat!(
        "let ready=false;let handled=false;let p=Promise.reject('tick');",
        "setDrain(()=>{if(!ready)return false;ready=false;p.catch(()=>{handled=true;});return true;});",
        "Promise.resolve().then(()=>{ready=true;});"
    )),"file:///tick-handler.js".into())).unwrap());
    while vm.tick(false).unwrap().made_progress {}
    assert!(
        rejection_updates(&debug).is_empty(),
        "native empty alone is not a completed checkpoint"
    );
    drop(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8("if(!handled)throw 'tick handler did not run';"),
            "file:///check-tick.js".into(),
        ))
        .unwrap(),
    );
}
#[test]
fn uncaught_policy_waits_for_same_checkpoint_async_handler() {
    let mut config = VmConfig::default();
    config.scheduler.microtask_hint = std::num::NonZeroUsize::new(1).unwrap();
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    let worker = controller(&vm, move || {
        let (entry, _, _) = paused(&debug);
        ask(
            &debug,
            DebugRequest::ExceptionPolicy(ExceptionPolicy::Uncaught),
        );
        debug
            .continue_execution(entry, DebugContinue::Resume)
            .unwrap();
        let mut stops = 0;
        let mut notices = 0;
        loop {
            while let Some(update) = debug.try_recv() {
                match update {
                    DebugUpdate::Paused { pause, .. } => {
                        stops += 1;
                        debug
                            .continue_execution(pause, DebugContinue::Resume)
                            .unwrap();
                    }
                    DebugUpdate::PromiseRejection(_) => notices += 1,
                    _ => {}
                }
            }
            if finished.try_recv().is_ok() {
                break;
            }
            thread::yield_now();
        }
        assert_eq!(
            stops, 0,
            "async throws cannot be classified uncaught before checkpoint completion"
        );
        assert_eq!(notices, 0);
        debug
    });
    let result = vm.evaluate(ScriptSource::new(
        JsString::from_utf8(concat!(
            "let p=(async()=>{throw 'late handler';})();let later=Promise.resolve();",
            "for(let i=0;i<8;i++)later=later.then(()=>{});later.then(()=>p.catch(()=>{}));"
        )),
        "file:///uncaught-timing.js".into(),
    ));
    let tick = if result.is_ok() {
        Some((|| -> Result<(), VmError> {
            while vm.tick(false)?.made_progress {}
            Ok(())
        })())
    } else {
        None
    };
    done.send(()).unwrap();
    let _debug = worker.join().unwrap();
    drop(result.unwrap());
    tick.unwrap().unwrap();
}

struct RawOverflow;
impl HostFunction for RawOverflow {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let error = call.take_rejection().unwrap_err();
        let failure = call.detach(error);
        call.import_data(JsData::Bool(failure.code == FailureCode::Capacity))
    }
}
#[test]
fn raw_rejection_overflow_is_explicit_without_stopping_application() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    vm.bind_function(JsString::from_utf8("rawOverflow"), 0, Rc::new(RawOverflow))
        .unwrap();
    let _debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    drop(vm.evaluate(ScriptSource::new(JsString::from_utf8("for(let i=0;i<1025;i++)Promise.reject(i);if(!rawOverflow())throw 'missing capacity';globalThis.complete=true;"),"file:///raw-overflow.js".into())).unwrap());
    drop(
        vm.evaluate(ScriptSource::new(
            JsString::from_utf8("if(!complete)throw 'application stopped';"),
            "file:///check-raw-overflow.js".into(),
        ))
        .unwrap(),
    );
}
