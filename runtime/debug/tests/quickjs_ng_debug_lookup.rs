//! Non-executing lookup through the production compiler, native VM and DAP.
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

type Vm = VirtualMachine<QuickJsNgEngine>;
fn vm() -> Vm {
    VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap()
}
fn source(text: &str) -> ScriptSource {
    ScriptSource::new(JsString::from_utf8(text), "file:///lookup.js".into())
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
fn lookup(d: &VmDebugHandle, frame: FrameRef, text: &str) -> Result<DebugValue, DebugError> {
    let request = DebugRequest::LookupPath {
        frame,
        path: JsString::from_utf8(text),
    };
    // Parse/identity rejection can happen synchronously before queue admission.
    let id = match d.try_request(request) {
        Ok(
            DebugRequestReceipt::Accepted { id }
            | DebugRequestReceipt::AcceptedWakeFailed { id, .. },
        ) => id,
        Err(rejected) => match rejected.reason {
            RejectionReason::Debug(error) => return Err(error),
            other => panic!("{other:?}"),
        },
    };
    match reply(d, id)? {
        DebugReply::Lookup(value) => Ok(value),
        other => panic!("lookup must not return an Evaluation reply: {other:?}"),
    }
}
fn exact(d: &VmDebugHandle, frame: FrameRef, path: &str, expected: &str) {
    let value = lookup(d, frame, path).unwrap();
    assert!(value.unavailable.is_none(), "{path}: {value:?}");
    assert!(!value.accessor, "{path}: {value:?}");
    assert_eq!(value.preview.to_utf8().unwrap(), expected, "{path}");
}
fn refused(d: &VmDebugHandle, frame: FrameRef, path: &str) {
    match lookup(d, frame, path) {
        Ok(value) => assert!(value.unavailable.is_some(), "{path}: {value:?}"),
        Err(DebugError::Unsupported) => {}
        other => panic!("expected structured unavailable, {path}: {other:?}"),
    }
}
fn run(script: &str, check: impl FnOnce(&VmDebugHandle, FrameRef) + Send + 'static) {
    let mut vm = vm();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        check(&debug, frame);
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source(script)).unwrap();
    c.join().unwrap();
}

#[test]
fn lookup_reads_verified_binding_and_own_data_path() {
    run(
        r#"
        let shadow={answer:1}; let globalLexical={answer:9};
        globalThis.globalProperty={answer:10};
        function make(captured){return function inner(arg){
          let local={answer:3}; {let shadow={answer:42};
          let object={'quoted key':[{answer:43}], '\uD800':44};
          debugger; return shadow.answer+captured.answer+arg.answer+local.answer;
        }}} make({answer:4})({answer:5});
    "#,
        |d, f| {
            for (path, expected) in [
                ("shadow.answer", "42"),
                ("captured.answer", "4"),
                ("arg.answer", "5"),
                ("local.answer", "3"),
                ("globalLexical.answer", "9"),
                ("globalProperty.answer", "10"),
                ("object['quoted key'][0].answer", "43"),
                (r"object['\uD800']", "44"),
            ] {
                exact(d, f, path, expected);
            }
        },
    );
}

#[test]
fn lookup_refuses_executable_syntax_before_native_access() {
    run(
        r#"
        let syntaxCalls=0; function touch(){syntaxCalls++;return 'answer'}
        let object={answer:42,valueOf(){syntaxCalls++;throw 1}};
        debugger; if(syntaxCalls!==0||object.answer!==42)throw 'syntax executed';
    "#,
        |d, f| {
            // Native/compiler entry counter deltas MUST be 0 for every row; see header.
            for expression in [
                "touch()",
                "object.answer=9",
                "object.answer++",
                "object[touch()]",
                "object['ans'+'wer']",
                "object+1",
                "object?.answer",
                "(object).answer",
                "new touch",
                "delete object.answer",
                "object;touch()",
            ] {
                assert_eq!(
                    lookup(d, f, expression).unwrap_err(),
                    DebugError::InvalidRequest,
                    "{expression}"
                );
            }
            exact(d, f, "syntaxCalls", "0");
            exact(d, f, "object.answer", "42");
        },
    );
}

#[test]
fn getter_and_proxy_paths_remain_unavailable() {
    run(
        r#"
        let getters=0,proxies=0,formatted=0,prototypes=0,coercions=0;
        Error.prepareStackTrace=()=>{formatted++;throw 17};
        let object={ok:42,get bad(){getters++;throw 18},
          [Symbol.toPrimitive](){coercions++;throw 19}};
        let proxy=new Proxy({}, {
          get(){proxies++;throw 20}, getOwnPropertyDescriptor(){proxies++;throw 21},
          ownKeys(){proxies++;throw 22},getPrototypeOf(){proxies++;throw 23}
        });
        let inherited=Object.create({get bad(){prototypes++;throw 24}});
        debugger;
        if(getters||proxies||formatted||prototypes||coercions)throw 'lookup invoked hook';
    "#,
        |d, f| {
            for path in [
                "object.bad",
                "object.bad.deep",
                "proxy.key",
                "proxy[0]",
                "inherited.bad",
                "missingGlobal",
            ] {
                refused(d, f, path);
            }
            exact(d, f, "object.ok", "42");
            for counter in ["getters", "proxies", "formatted", "prototypes", "coercions"] {
                exact(d, f, counter, "0");
            }
        },
    );
}

#[test]
fn own_undefined_dense_holes_sparse_indices_and_literal_keys_are_exact() {
    run(
        r#"
        let prototypeCalls=0,formatted=0;
        Error.prepareStackTrace=()=>{formatted++;throw 1};
        let object={present:undefined,'4294967295':45,[Symbol('key')]:46};
        let dense=[41,42]; let sparse=[]; sparse[90000]=43;
        let hole=[1,,3];Object.setPrototypeOf(hole,{get 1(){prototypeCalls++;throw 2}});
        let inherited=Object.create({get absent(){prototypeCalls++;throw 3}});
        debugger;if(prototypeCalls||formatted)throw 'inherited lookup';
    "#,
        |d, f| {
            exact(d, f, "object.present", "undefined");
            refused(d, f, "object.absent"); // absent must not be successful undefined
            exact(d, f, "dense[0]", "41");
            exact(d, f, "dense['1']", "42");
            exact(d, f, "sparse[90000]", "43");
            refused(d, f, "sparse[1]");
            refused(d, f, "hole[1]");
            refused(d, f, "inherited.absent");
            exact(d, f, "object[4294967295]", "45");
            exact(d, f, "object['4294967295']", "45");
            refused(d, f, "object['Symbol(key)']"); // symbol description is not a string key
            assert_eq!(
                lookup(d, f, "object[Symbol('key')]").unwrap_err(),
                DebugError::InvalidRequest
            );
            exact(d, f, "prototypeCalls", "0");
            exact(d, f, "formatted", "0");
        },
    );
}

#[test]
fn unsupported_receivers_refuse_before_boxing_or_exotic_hooks() {
    run(
        r#"
        let prototypeCalls=0,formatted=0;
        Object.defineProperty(Number.prototype,'bad',{get(){prototypeCalls++;throw 1}});
        Error.prepareStackTrace=()=>{formatted++;throw 2};
        let number=7,string='abc',nil=null,undef=undefined,big=1n,symbol=Symbol();
        let typed=new Uint8Array([42]),shared=new SharedArrayBuffer(8),map=new Map();
        debugger;if(prototypeCalls||formatted)throw 'boxed or formatted';
    "#,
        |d, f| {
            for path in [
                "number.bad",
                "string[0]",
                "nil.bad",
                "undef.bad",
                "big.bad",
                "symbol.bad",
                "typed[0]",
                "typed.length",
                "shared.byteLength",
                "map.size",
            ] {
                refused(d, f, path);
            }
            exact(d, f, "prototypeCalls", "0");
            exact(d, f, "formatted", "0");
        },
    );
}

#[test]
fn first_tdz_binding_blocks_outer_and_global_fallback() {
    run(
        r#"
        globalThis.shadow=99;globalThis.cell=98;let formatted=0;
        Error.prepareStackTrace=()=>{formatted++;throw 1};
        function outer(){let shadow=42;{debugger;let shadow=43;}}
        outer();
    "#,
        |d, f| {
            refused(d, f, "shadow");
            exact(d, f, "formatted", "0");
        },
    );
    // Arrow captures a TDZ VARREF while its parent is still active.
    run(
        "globalThis.cell=98;function outer(){(()=>{debugger;})();let cell=42;}outer();",
        |d, f| refused(d, f, "cell"),
    );
}

#[test]
fn global_fallback_reads_own_lexical_then_own_object_without_getters() {
    run(
        r#"
        globalThis.precedence=99;let precedence=42;var globalVar=43;
        let globalCalls=0;Object.defineProperty(globalThis,'hostileGlobal',{
          get(){globalCalls++;throw 1}});
        let oldPrototype=Object.getPrototypeOf(globalThis);
        Object.setPrototypeOf(globalThis,Object.create(oldPrototype,{
          inheritedGlobal:{get(){globalCalls++;throw 2}}
        }));
        (function(){debugger;})();if(globalCalls)throw 'global hook';
    "#,
        |d, f| {
            exact(d, f, "precedence", "42");
            exact(d, f, "globalVar", "43");
            refused(d, f, "hostileGlobal");
            refused(d, f, "inheritedGlobal");
            exact(d, f, "globalCalls", "0");
        },
    );
}

#[test]
fn this_reads_ordinary_unused_receiver_and_strict_primitive_without_synthesis() {
    // Neither ordinary function references this in source: debug retention is required.
    run(
        "(function(){'use strict';debugger;}).call({marker:42});",
        |d, f| exact(d, f, "this.marker", "42"),
    );
    run("(function(){'use strict';debugger;}).call(7);", |d, f| {
        exact(d, f, "this", "7");
        refused(d, f, "this.marker");
    });
    run("globalThis.marker=43;(function(){debugger;})();", |d, f| {
        exact(d, f, "this.marker", "43")
    });
}

#[test]
fn this_reads_returned_arrow_lexical_capture_and_ordinary_own_receiver() {
    run(
        "function make(){return ()=>{debugger;};}let arrow=make.call({marker:42});arrow.call({marker:99});",
        |d, f| exact(d, f, "this.marker", "42"),
    );
    run(
        "function make(){return function(){debugger;};}let ordinary=make.call({marker:99});ordinary.call({marker:43});",
        |d, f| exact(d, f, "this.marker", "43"),
    );
}

#[test]
fn derived_this_and_arrow_share_pre_super_tdz_then_initialized_receiver() {
    let mut vm = vm();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        // Ordinary derived frame, arrow before super, derived after super, returned arrow.
        for initialized in [false, false, true, true] {
            let (pause, _, _) = paused(&debug);
            let frame = top(&debug, pause);
            if initialized {
                exact(&debug, frame, "this.marker", "42");
            } else {
                refused(&debug, frame, "this");
                refused(&debug, frame, "this.marker");
            }
            debug
                .continue_execution(pause, DebugContinue::Resume)
                .unwrap();
        }
    });
    vm.evaluate(source(
        r#"
        class Base{constructor(){this.marker=42}}
        class Derived extends Base{constructor(){
          debugger;(()=>{debugger;})();super();debugger;
          return ()=>{debugger;};
        }}
        let arrow=new Derived();arrow.call({marker:99});
    "#,
    ))
    .unwrap();
    c.join().unwrap();
}

#[test]
fn hover_refuses_stale_frame_after_resume() {
    let mut vm = vm();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let stale = top(&debug, pause);
        exact(&debug, stale, "object.answer", "42");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        let (next, _, _) = paused(&debug);
        let fresh = top(&debug, next);
        // Private production-backend observer also proves zero FFI/compiler access.
        assert_eq!(
            lookup(&debug, stale, "object.answer").unwrap_err(),
            DebugError::StalePause
        );
        exact(&debug, fresh, "object.answer", "43");
        debug
            .continue_execution(next, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source(
        "let object={answer:42};debugger;object.answer=43;debugger;",
    ))
    .unwrap();
    c.join().unwrap();
}

#[test]
fn lookup_preserves_pending_application_exception_identity_and_trace() {
    let mut vm = vm();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let policy = send(&debug, DebugRequest::ExceptionPolicy(ExceptionPolicy::All));
    let c = controller(&vm, move || {
        reply(&debug, policy).unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        let frame = top(&debug, pause);
        exact(&debug, frame, "original.marker", "42");
        exact(&debug, frame, "data.marker", "42");
        refused(&debug, frame, "hostile.bad");
        refused(&debug, frame, "missingGlobal");
        exact(&debug, frame, "formatted", "0");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(source(
        r#"
        let original=new Error('application'),data={marker:42};original.marker=42;
        let trace=original.stack,formatted=0;
        Error.prepareStackTrace=()=>{formatted++;throw 17};
        let hostile={get bad(){throw 'getter'}};
        try{throw original}catch(error){
          if(error!==original||error.stack!==trace||formatted)throw 'exception changed';
        }
    "#,
    ))
    .unwrap();
    c.join().unwrap();
}

// Actual DAP transport + native VM; fixture metadata is not native acceptance.
struct Client {
    adapter: DapAdapter,
    seq: u64,
    inbox: Vec<Value>,
}
impl Client {
    fn decode(wire: &[u8]) -> Value {
        let start = wire.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
        serde_json::from_slice(&wire[start..]).unwrap()
    }
    fn pump(&mut self) {
        self.inbox
            .extend(self.adapter.poll().unwrap().iter().map(|f| Self::decode(f)));
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
        self.inbox.extend(
            self.adapter
                .receive(&wire)
                .unwrap()
                .iter()
                .map(|f| Self::decode(f)),
        );
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(i) = self.inbox.iter().position(|f| f["request_seq"] == seq) {
                return self.inbox.remove(i);
            }
            assert!(Instant::now() < until, "missing response {seq}");
            self.pump();
            thread::yield_now();
        }
    }
    fn stopped(&mut self) {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(i) = self.inbox.iter().position(|f| f["event"] == "stopped") {
                self.inbox.remove(i);
                return;
            }
            assert!(Instant::now() < until);
            self.pump();
            thread::yield_now();
        }
    }
}
#[test]
fn automatic_watch_never_falls_back_to_evaluate() {
    let mut vm = vm();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    assert!(
        !debug
            .descriptor()
            .capabilities
            .contains(DebugCapability::ReadOnlyEvaluation)
    );
    let adapter = DapAdapter::new(debug).unwrap();
    let c = controller(&vm, move || {
        let mut dap = Client {
            adapter,
            seq: 0,
            inbox: Vec::new(),
        };
        let init = dap.request("initialize", json!({}));
        // Only enable SafeLookup and this DAP flag after the native suite is green.
        assert_eq!(init["body"]["supportsEvaluateForHovers"], true);
        assert_eq!(dap.request("configurationDone", json!({}))["success"], true);
        dap.stopped();
        let stack = dap.request("stackTrace", json!({"threadId":1}));
        let frame = stack["body"]["stackFrames"][0]["id"].clone();
        for context in ["hover", "watch"] {
            let result = dap.request(
                "evaluate",
                json!({"frameId":frame,"expression":"object.answer","context":context}),
            );
            assert_eq!(result["success"], true, "{result}");
            assert_eq!(result["body"]["result"], "42");
            for expression in [
                "touch()",
                "object.answer=99",
                "object[touch()]",
                "object+1",
                "object.bad",
            ] {
                let denied = dap.request(
                    "evaluate",
                    json!({"frameId":frame,"expression":expression,"context":context}),
                );
                assert_eq!(denied["success"], false, "{denied}");
                if expression != "object.bad" {
                    assert!(
                        denied["message"]
                            .as_str()
                            .unwrap()
                            .contains("literal property paths"),
                        "{denied}"
                    );
                }
            }
        }
        let count = dap.request(
            "evaluate",
            json!({"frameId":frame,"expression":"calls","context":"watch"}),
        );
        assert_eq!(count["body"]["result"], "0");
        // Explicit REPL is the consent boundary and retains evaluator budgets.
        let repl = dap.request(
            "evaluate",
            json!({"frameId":frame,"expression":"touch()","context":"repl"}),
        );
        assert_eq!(repl["success"], true);
        assert_eq!(repl["body"]["result"], "1");
        let count = dap.request(
            "evaluate",
            json!({"frameId":frame,"expression":"calls","context":"hover"}),
        );
        assert_eq!(count["body"]["result"], "1");
        assert_eq!(
            dap.request("continue", json!({"threadId":1}))["success"],
            true
        );
    });
    vm.evaluate(source("let calls=0;function touch(){return ++calls}let object={answer:42,get bad(){calls++;return 9}};debugger;if(calls!==1||object.answer!==42)throw 'automatic evaluator fallback';")).unwrap();
    c.join().unwrap();
}

#[test]
fn lookup_retains_inspection_cursor_and_edit_target_identities() {
    run(
        "let object={};for(let i=0;i<130;i++)object['p'+i]=i;debugger;",
        |d, f| {
            let object = lookup(d, f, "object").unwrap().object.unwrap();
            let parent = VariableParent::Object(object);
            let first = match ask(
                d,
                DebugRequest::Inspect(InspectionRequest {
                    parent,
                    page: Page { start: 0, count: 1 },
                    cursor: None,
                }),
            ) {
                DebugReply::Inspection(page) => page,
                other => panic!("{other:?}"),
            };
            let target = first.items[0]
                .write
                .expect("existing own data edit identity");
            let cursor = first.next.expect("nonfinal page");
            exact(d, f, "object.p42", "42");
            refused(d, f, "object.missing");
            let next = match ask(
                d,
                DebugRequest::Inspect(InspectionRequest {
                    parent,
                    page: Page { start: 0, count: 1 },
                    cursor: Some(cursor),
                }),
            ) {
                DebugReply::Inspection(page) => page,
                other => panic!("{other:?}"),
            };
            assert_eq!(next.items[0].variable.name.to_utf8().unwrap(), "p1");
            // Existing exact edit handle still works after both successful/refused lookup.
            assert!(matches!(
                ask(
                    d,
                    DebugRequest::Write {
                        target,
                        frame: f,
                        expression: JsString::from_utf8("99")
                    }
                ),
                DebugReply::Write(_)
            ));
            exact(d, f, "object.p0", "99");
        },
    );
}

#[test]
fn safe_lookup_capability_has_current_native_build_identity() {
    let engine = QuickJsNgEngine::new().unwrap();
    let identity = &engine.descriptor().build_fingerprint;
    assert!(
        identity.contains("safe-lookup-v1") && identity.contains(";abi=14;"),
        "{identity}"
    );
    println!("G006_NATIVE_BUILD={identity}");
    let mut vm = VirtualMachine::new(engine, VmConfig::default()).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    assert!(
        debug
            .descriptor()
            .capabilities
            .contains(DebugCapability::SafeLookup)
    );
    assert!(
        !debug
            .descriptor()
            .capabilities
            .contains(DebugCapability::ReadOnlyEvaluation)
    );
}

#[test]
fn parameter_initializer_scope_does_not_expose_function_body_locals() {
    run(
        "let hidden=42;function pause(){debugger;}function f(a=pause()){var hidden=99;}f();",
        |d, _top| {
            let caller = match ask(
                d,
                DebugRequest::Stack {
                    pause: match d.state() {
                        DebugExecutionState::Paused(pause) => pause,
                        state => panic!("{state:?}"),
                    },
                    page: Page { start: 1, count: 1 },
                },
            ) {
                DebugReply::Stack { frames, .. } => frames[0].id,
                other => panic!("{other:?}"),
            };
            exact(d, caller, "hidden", "42");
        },
    );
}

fn dynamic_environment_probe(script: &str) {
    run(script, |d, f| {
        let value = lookup(d, f, "x").unwrap();
        assert!(
            value.unavailable.is_some() || value.preview.to_utf8().unwrap() == "42",
            "dynamic environment fell through: {value:?}"
        );
    });
}
#[test]
fn with_dynamic_environment_never_falls_through_to_outer_binding() {
    dynamic_environment_probe("let x=1;with({x:42}){debugger;}");
}
#[test]
fn eval_var_dynamic_environment_never_falls_through_to_outer_binding() {
    dynamic_environment_probe("let x=1;function f(){eval('var x=42');debugger;}f();");
}

#[test]
fn dynamic_environment_refusal_never_runs_proxy_getter_or_unscopables_hooks() {
    run(
        r#"
        let calls=0,x=1;
        let object={get x(){calls++;throw 1},get [Symbol.unscopables](){calls++;throw 2}};
        with(object){debugger;}
        if(calls)throw 'dynamic getter or unscopables ran';
    "#,
        |d, f| refused(d, f, "x"),
    );
    run(
        r#"
        let calls=0,x=1;
        let proxy=new Proxy({x:42},{has(){calls++;throw 1},get(){calls++;throw 2},
          getOwnPropertyDescriptor(){calls++;throw 3},getPrototypeOf(){calls++;throw 4}});
        with(proxy){let inner=43;debugger;}
        if(calls)throw 'dynamic proxy ran';
    "#,
        |d, f| {
            exact(d, f, "inner", "43");
            refused(d, f, "x");
        },
    );
    // Captured dynamic environments cannot override an ordinary receiver.
    run(
        r#"
        let calls=0,x=1;let object={get x(){calls++;throw 1}};
        with(object){(function(){debugger;}).call({marker:44});}
        if(calls)throw 'captured dynamic getter ran';
    "#,
        |d, f| {
            exact(d, f, "this.marker", "44");
            refused(d, f, "x");
        },
    );
}

#[test]
fn parameter_initializer_keeps_receiver_and_function_self_visible() {
    run(
        "function pause(){debugger;}let fn=function named(a=pause()){var hidden=99;};fn.call({marker:42});",
        |d, _| {
            let pause = match d.state() {
                DebugExecutionState::Paused(pause) => pause,
                state => panic!("{state:?}"),
            };
            let caller = match ask(
                d,
                DebugRequest::Stack {
                    pause,
                    page: Page { start: 1, count: 1 },
                },
            ) {
                DebugReply::Stack { frames, .. } => frames[0].id,
                other => panic!("{other:?}"),
            };
            exact(d, caller, "this.marker", "42");
            assert!(lookup(d, caller, "named").unwrap().unavailable.is_none());
        },
    );
}

#[test]
fn parameter_initializer_inspection_never_issues_inactive_body_edit_target() {
    run(
        "let hidden=42;function pause(){debugger;}function f(a=pause()){var hidden=99;}f();",
        |d, _| {
            let pause = match d.state() {
                DebugExecutionState::Paused(pause) => pause,
                state => panic!("{state:?}"),
            };
            let caller = match ask(
                d,
                DebugRequest::Stack {
                    pause,
                    page: Page { start: 1, count: 1 },
                },
            ) {
                DebugReply::Stack { frames, .. } => frames[0].id,
                other => panic!("{other:?}"),
            };
            let scopes = match ask(d, DebugRequest::Scopes { frame: caller }) {
                DebugReply::Scopes(scopes) => scopes,
                other => panic!("{other:?}"),
            };
            for scope in scopes {
                let mut cursor = None;
                loop {
                    let page = match ask(
                        d,
                        DebugRequest::Inspect(InspectionRequest {
                            parent: VariableParent::Scope(scope.id),
                            page: Page {
                                start: 0,
                                count: 64,
                            },
                            cursor,
                        }),
                    ) {
                        DebugReply::Inspection(page) => page,
                        other => panic!("{other:?}"),
                    };
                    for entry in &page.items {
                        assert!(
                            !(entry.variable.name.to_utf8().unwrap() == "hidden"
                                && entry.variable.value.preview.to_utf8().unwrap() == "undefined"),
                            "inactive body variable leaked into parameter inspection: {entry:?}"
                        );
                    }
                    if page.complete {
                        break;
                    }
                    cursor = page.next;
                }
            }
        },
    );
}

#[test]
fn parameter_initializer_arguments_never_falls_through_to_global_collision() {
    run(
        "globalThis.arguments=99;function pause(){debugger;}function f(a=pause()){return arguments;}f();",
        |d, _| {
            let pause = match d.state() {
                DebugExecutionState::Paused(pause) => pause,
                state => panic!("{state:?}"),
            };
            let caller = match ask(
                d,
                DebugRequest::Stack {
                    pause,
                    page: Page { start: 1, count: 1 },
                },
            ) {
                DebugReply::Stack { frames, .. } => frames[0].id,
                other => panic!("{other:?}"),
            };
            let value = lookup(d, caller, "arguments").unwrap();
            assert!(
                value.unavailable.is_some() || value.object.is_some(),
                "materialized arguments lost: {value:?}"
            );
        },
    );
}

#[test]
fn parameter_arguments_metadata_excludes_body_collision_and_preserves_mutability() {
    run(
        "globalThis.arguments=99;globalThis.writeCalls=0;function pause(){debugger;}function f(a=pause()){var arguments=77;return arguments;}if(f()!==77||writeCalls)throw 'body collision changed';",
        |d, _| {
            let pause = match d.state() {
                DebugExecutionState::Paused(pause) => pause,
                state => panic!("{state:?}"),
            };
            let caller = match ask(
                d,
                DebugRequest::Stack {
                    pause,
                    page: Page { start: 1, count: 1 },
                },
            ) {
                DebugReply::Stack { frames, .. } => frames[0].id,
                other => panic!("{other:?}"),
            };
            let scopes = match ask(d, DebugRequest::Scopes { frame: caller }) {
                DebugReply::Scopes(scopes) => scopes,
                other => panic!("{other:?}"),
            };
            let mut targets = vec![];
            for scope in scopes {
                let mut cursor = None;
                loop {
                    let page = match ask(
                        d,
                        DebugRequest::Inspect(InspectionRequest {
                            parent: VariableParent::Scope(scope.id),
                            page: Page {
                                start: 0,
                                count: 64,
                            },
                            cursor,
                        }),
                    ) {
                        DebugReply::Inspection(page) => page,
                        other => panic!("{other:?}"),
                    };
                    for entry in page.items {
                        if entry.variable.name.to_utf8().unwrap() == "arguments" {
                            assert_ne!(
                                entry.variable.value.preview.to_utf8().unwrap(),
                                "undefined",
                                "inactive body arguments leaked"
                            );
                            targets.push(entry.write.expect("materialized arguments is mutable"));
                        }
                    }
                    if page.complete {
                        break;
                    }
                    cursor = page.next;
                }
            }
            assert_eq!(
                targets.len(),
                1,
                "only actual parameter-environment arguments may be issued"
            );
            let id = send(
                d,
                DebugRequest::Write {
                    target: targets[0],
                    frame: caller,
                    expression: JsString::from_utf8("writeCalls++;({marker:42})"),
                },
            );
            // Split-parameter evaluator entry remains explicitly unsupported.
            assert_eq!(reply(d, id).unwrap_err(), DebugError::InvalidRequest);
            exact(d, caller, "writeCalls", "0");
        },
    );
}
#[test]
fn parameter_arguments_reassigned_before_pause_retains_its_binding_proof() {
    run(
        "globalThis.arguments=99;function pause(){debugger;}function f(a=(arguments=17),b=pause()){return arguments;}if(f()!==17)throw 'arguments mutation lost';",
        |d, _| {
            let pause = match d.state() {
                DebugExecutionState::Paused(pause) => pause,
                state => panic!("{state:?}"),
            };
            let caller = match ask(
                d,
                DebugRequest::Stack {
                    pause,
                    page: Page { start: 1, count: 1 },
                },
            ) {
                DebugReply::Stack { frames, .. } => frames[0].id,
                other => panic!("{other:?}"),
            };
            exact(d, caller, "arguments", "17");
        },
    );
}

#[test]
fn body_active_arguments_declaration_shadows_materialized_parameter_binding() {
    run(
        "function f(a=1){var arguments=77;debugger;return arguments;}if(f()!==77)throw 'body args changed';",
        |d, f| {
            exact(d, f, "arguments", "77");
            let scopes = match ask(d, DebugRequest::Scopes { frame: f }) {
                DebugReply::Scopes(scopes) => scopes,
                other => panic!("{other:?}"),
            };
            let mut names = 0;
            for scope in scopes {
                let page = match ask(
                    d,
                    DebugRequest::Inspect(InspectionRequest {
                        parent: VariableParent::Scope(scope.id),
                        page: Page {
                            start: 0,
                            count: 64,
                        },
                        cursor: None,
                    }),
                ) {
                    DebugReply::Inspection(page) => page,
                    other => panic!("{other:?}"),
                };
                assert!(page.complete);
                for entry in page.items {
                    if entry.variable.name.to_utf8().unwrap() == "arguments" {
                        names += 1;
                        assert_eq!(entry.variable.value.preview.to_utf8().unwrap(), "77");
                        assert!(entry.write.is_some());
                    }
                }
            }
            assert_eq!(
                names, 1,
                "active body binding must be the only arguments local view"
            );
        },
    );
}
#[test]
fn unused_parameter_arguments_binding_never_selects_unproved_global() {
    run(
        "globalThis.arguments=99;function pause(){debugger;}function f(a=pause()){return 0;}f();",
        |d, _| {
            let pause = match d.state() {
                DebugExecutionState::Paused(pause) => pause,
                state => panic!("{state:?}"),
            };
            let caller = match ask(
                d,
                DebugRequest::Stack {
                    pause,
                    page: Page { start: 1, count: 1 },
                },
            ) {
                DebugReply::Stack { frames, .. } => frames[0].id,
                other => panic!("{other:?}"),
            };
            let value = lookup(d, caller, "arguments").unwrap();
            assert!(
                value.unavailable.is_some() || value.object.is_some(),
                "unproved implicit arguments: {value:?}"
            );
        },
    );
}

#[test]
fn returned_parameter_arrow_preserves_captured_arguments_owner() {
    run(
        "function outer(a=()=>{debugger;return arguments;}){var arguments=77;return a;}let value=outer(undefined,42)();if(value[1]!==42)throw 'parameter capture changed';",
        |d, f| {
            let value = lookup(d, f, "arguments").unwrap();
            assert!(
                value.unavailable.is_some() || value.object.is_some(),
                "capture incorrectly chose body arguments: {value:?}"
            );
        },
    );
}

#[test]
fn split_body_var_shadows_outer_parameter_and_keeps_explicit_outer_view() {
    run(
        "function f(x=1){var x=77;debugger;return x;}if(f()!==77)throw 'body var changed';",
        |d, f| {
            exact(d, f, "x", "77");
            let scopes = match ask(d, DebugRequest::Scopes { frame: f }) {
                DebugReply::Scopes(scopes) => scopes,
                other => panic!("{other:?}"),
            };
            let mut values = vec![];
            for scope in scopes {
                let page = match ask(
                    d,
                    DebugRequest::Inspect(InspectionRequest {
                        parent: VariableParent::Scope(scope.id),
                        page: Page {
                            start: 0,
                            count: 64,
                        },
                        cursor: None,
                    }),
                ) {
                    DebugReply::Inspection(page) => page,
                    other => panic!("{other:?}"),
                };
                assert!(page.complete);
                assert!(page.scanned_slots <= 4096);
                for entry in page.items {
                    if entry.variable.name.to_utf8().unwrap() == "x" {
                        values.push((
                            scope.id,
                            entry.variable.value.preview.to_utf8().unwrap(),
                            entry.write,
                        ));
                    }
                }
            }
            assert!(
                values
                    .iter()
                    .any(|(_, v, target)| v == "77" && target.is_some())
            );
            assert!(
                values
                    .iter()
                    .any(|(_, v, target)| v == "1" && target.is_some())
            );
            assert_ne!(
                values[0].0, values[1].0,
                "body and outer parameter must retain distinct scope identities"
            );
        },
    );
}
#[test]
fn parameter_created_generic_arrow_excludes_inactive_body_capture() {
    run(
        "let hidden=42;function f(a=()=>{debugger;return hidden}){var hidden=77;return a;}if(f()()!==42)throw 'parameter lexical binding changed';",
        |d, f| exact(d, f, "hidden", "42"),
    );
}
#[test]
fn parameter_and_body_arrows_preserve_their_actual_owner_cells() {
    run(
        "function f(x=42,a=()=>{debugger;return x}){var x=77;return a;}if(f()()!==42)throw 'parameter x changed';",
        |d, f| exact(d, f, "x", "42"),
    );
    run(
        "function f(x=1){var x=77;return()=>{debugger;return x};}if(f()()!==77)throw 'body x changed';",
        |d, f| exact(d, f, "x", "77"),
    );
}
#[test]
fn computed_class_key_in_parameter_never_proves_body_scope() {
    for source in [
        "let hidden=42;function pause(){debugger;return 'key'}function f(a=class C{[pause()](){}}){var hidden=77;return 0;}f();",
        "let hidden=42;function pause(){debugger;return 'key'}function f(a=class{[pause()](){}}){var hidden=77;return 0;}f();",
    ] {
        run(source, |d, _| {
            let pause = match d.state() {
                DebugExecutionState::Paused(p) => p,
                state => panic!("{state:?}"),
            };
            let caller = match ask(
                d,
                DebugRequest::Stack {
                    pause,
                    page: Page { start: 1, count: 1 },
                },
            ) {
                DebugReply::Stack { frames, .. } => frames[0].id,
                other => panic!("{other:?}"),
            };
            let value = lookup(d, caller, "hidden").unwrap();
            assert!(
                value.unavailable.is_some() || value.preview.to_utf8().unwrap() == "42",
                "nested parameter lexical context was misclassified as body: {value:?}"
            );
        });
    }
}
