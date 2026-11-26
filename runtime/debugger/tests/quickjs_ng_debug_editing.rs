//! Exact identity editing through the production compiler and native VM.
//! Const/TDZ/accessors have no public write identity: assert non-issuance here;
//! DAP refusal attempts below prove no RHS execution for those displayed entries.
//! Native/unit tests separately defend retained metadata permission checks.
#![cfg(feature = "quickjs-ng")]
use coldmind_runtime::{debug::*, engine::*, engines::quickjs_ng::QuickJsNgEngine, vm::*};
use std::{
    cell::Cell,
    rc::{Rc, Weak},
    sync::mpsc,
    thread,
    time::Duration,
};
#[path = "support/debugger.rs"]
mod debugger;
use debugger::{ask, paused, reply, send};
use serde_json::{Value, json};

// Test-local forwarding keeps a Weak engine for a real host-driven native GC.
// No compiler or scope behavior is replaced by the wrapper.
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
    ScriptSource::new(JsString::from_utf8(text), "file:///editing.js".into())
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
fn eval(d: &VmDebugHandle, frame: FrameRef, text: &str) -> DebugValue {
    match ask(
        d,
        DebugRequest::Evaluate {
            frame,
            expression: JsString::from_utf8(text),
            allow_side_effects: true,
        },
    ) {
        DebugReply::Evaluation { value, exception } => {
            assert!(!exception, "{value:?}");
            value
        }
        other => panic!("{other:?}"),
    }
}
fn preview(d: &VmDebugHandle, frame: FrameRef, text: &str) -> String {
    eval(d, frame, text).preview.to_utf8().unwrap()
}
fn entries(d: &VmDebugHandle, parent: VariableParent) -> Vec<InspectionEntry> {
    let mut out = Vec::new();
    let mut cursor = None;
    loop {
        match ask(
            d,
            DebugRequest::Inspect(InspectionRequest {
                parent,
                page: Page {
                    start: 0,
                    count: 64,
                },
                cursor,
            }),
        ) {
            DebugReply::Inspection(page) => {
                out.extend(page.items);
                cursor = page.next;
                if page.complete {
                    return out;
                }
            }
            other => panic!("{other:?}"),
        }
    }
}
fn bindings(d: &VmDebugHandle, frame: FrameRef) -> Vec<InspectionEntry> {
    match ask(d, DebugRequest::Scopes { frame }) {
        DebugReply::Scopes(scopes) => scopes
            .into_iter()
            .flat_map(|s| entries(d, VariableParent::Scope(s.id)))
            .collect(),
        other => panic!("{other:?}"),
    }
}
fn binding(d: &VmDebugHandle, frame: FrameRef, name: &str, old: &str) -> WriteTarget {
    bindings(d, frame)
        .into_iter()
        .find(|e| {
            e.variable.name.to_utf8().unwrap() == name
                && e.variable.value.preview.to_utf8().unwrap() == old
        })
        .unwrap_or_else(|| panic!("missing binding {name}={old}"))
        .write
        .expect("mutable binding must issue exact target")
}
fn properties(d: &VmDebugHandle, frame: FrameRef, object: &str) -> Vec<InspectionEntry> {
    let object = eval(d, frame, object)
        .object
        .expect("supported own-property receiver");
    entries(d, VariableParent::Object(object))
}
fn property(d: &VmDebugHandle, frame: FrameRef, object: &str, name: &str) -> WriteTarget {
    properties(d, frame, object)
        .into_iter()
        .find(|e| e.variable.name.to_utf8().unwrap() == name)
        .unwrap()
        .write
        .expect("writable own data property target")
}
fn write_result(
    d: &VmDebugHandle,
    frame: FrameRef,
    target: WriteTarget,
    rhs: &str,
) -> Result<DebugReply, DebugError> {
    let id = send(
        d,
        DebugRequest::Write {
            target,
            frame,
            expression: JsString::from_utf8(rhs),
        },
    );
    reply(d, id)
}
fn write(d: &VmDebugHandle, frame: FrameRef, target: WriteTarget, rhs: &str) {
    assert!(matches!(
        write_result(d, frame, target, rhs),
        Ok(DebugReply::Write(_))
    ));
}
// Equivalent watchdog to support/debugger.rs, accepting a generic VM engine.
// Generalize existing support::controller after release if that is the smaller diff.
fn run(script: &str, check: impl FnOnce(&VmDebugHandle, PauseId, FrameRef) + Send + 'static) {
    run_config(script, VmConfig::default(), check);
}
fn run_config(
    script: &str,
    config: VmConfig,
    check: impl FnOnce(&VmDebugHandle, PauseId, FrameRef) + Send + 'static,
) {
    let engine = Rc::new(QuickJsNgEngine::new().unwrap());
    let collections = Rc::new(Cell::new(0));
    let mut vm = VirtualMachine::new(SharedEngine(engine.clone()), config).unwrap();
    vm.bind_function(
        JsString::from_utf8("forceGC"),
        0,
        Rc::new(ForceGc(Rc::downgrade(&engine), collections)),
    )
    .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let stop = vm.handle();
    let deadline_stop = vm.handle();
    let (cancel, canceled) = mpsc::channel();
    let watchdog = thread::spawn(move || {
        if canceled.recv_timeout(Duration::from_secs(20)).is_err() {
            let _ = deadline_stop.request_stop();
        }
    });
    let worker = thread::spawn(move || {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (pause, _, _) = paused(&debug);
            let frame = top(&debug, pause);
            check(&debug, pause, frame);
            debug
                .continue_execution(pause, DebugContinue::Resume)
                .unwrap();
        })) {
            Ok(()) => (),
            Err(panic) => {
                let _ = stop.request_stop();
                std::panic::resume_unwind(panic);
            }
        }
    });
    let result = vm.evaluate(source(script));
    let _ = cancel.send(());
    watchdog.join().unwrap();
    worker.join().unwrap();
    result.unwrap();
}

#[test]
fn edit_shadowed_local_and_captured_binding_changes_only_target() {
    run(
        r#"
        function outer(captured) {
            return function inner(argument) {
                let shadow=4; let readOuter=()=>shadow;
                { let shadow=5; debugger;
                  if(shadow!==50 || readOuter()!==40 || captured!==70 || argument!==11) throw 'wrong binding'; }
                if(shadow!==40) throw 'outer shadow lost';
                return captured;
            };
        }
        if(outer(7)(11)!==70) throw 'capture lost';
    "#,
        |d, _, frame| {
            let inner = binding(d, frame, "shadow", "5");
            let outer = binding(d, frame, "shadow", "4");
            assert_ne!(inner, outer);
            write(d, frame, inner, "50");
            assert_eq!(
                preview(d, frame, "[shadow,readOuter(),captured,argument].join(',')"),
                "\"50,4,7,11\""
            );
            // Every write/evaluation invalidates issued targets: reacquire each time.
            write(d, frame, binding(d, frame, "shadow", "4"), "40");
            assert_eq!(
                preview(d, frame, "[shadow,readOuter(),captured].join(',')"),
                "\"50,40,7\""
            );
            write(d, frame, binding(d, frame, "captured", "7"), "70");
            assert_eq!(
                preview(d, frame, "[shadow,readOuter(),captured].join(',')"),
                "\"50,40,70\""
            );
        },
    );
}

#[test]
fn const_tdz_and_stale_write_fail_before_rhs_side_effect() {
    dap_refuses_readonly(
        "(function(){let rhs=0;const fixed=1;debugger;let later=3;if(rhs!==0)throw 'RHS ran';})();",
        &["fixed", "later"],
        false,
    );
    run(
        r#"
        (function(){ let rhs=0; const fixed=1; let editable=2;
          debugger; let later=3;
          if(rhs!==0 || fixed!==1 || editable!==2 || later!==3) throw 'readonly target changed';
        })();
    "#,
        |d, _, frame| {
            let entries = bindings(d, frame);
            for name in ["fixed", "later"] {
                let entry = entries
                    .iter()
                    .find(|e| e.variable.name.to_utf8().unwrap() == name)
                    .expect("read-only/TDZ binding remains visible");
                assert!(entry.write.is_none(), "{name} must not offer a write");
            }
            let old = binding(d, frame, "editable", "2");
            assert_eq!(preview(d, frame, "0"), "0");
            assert_eq!(
                write_result(d, frame, old, "(++rhs,99)").unwrap_err(),
                DebugError::StaleInspection
            );
            assert_eq!(preview(d, frame, "rhs"), "0");
        },
    );
}

#[test]
fn duplicate_symbol_labels_edit_distinct_properties() {
    run(
        r#"
        let a=Symbol('same'), b=Symbol('same'), c=Symbol.for('same');
        let object={[a]:1,[b]:2,[c]:3}; debugger;
        if(object[a]!==11 || object[b]!==22 || object[c]!==33) throw 'symbol identity lost';
    "#,
        |d, _, frame| {
            let items = properties(d, frame, "object");
            let labels: std::collections::HashSet<_> = items
                .iter()
                .map(|e| e.variable.name.to_utf8().unwrap())
                .collect();
            assert_eq!(items.len(), 3);
            assert_eq!(
                labels.len(),
                3,
                "display labels must disambiguate local/registered symbol descriptions"
            );
            let targets: Vec<_> = items.iter().map(|e| e.write.unwrap()).collect();
            assert_ne!(targets[0], targets[1]);
            assert_ne!(targets[0], targets[2]);
            assert_ne!(targets[1], targets[2]);
            for (before, after) in [(1, 11), (2, 22), (3, 33)] {
                let entry = properties(d, frame, "object")
                    .into_iter()
                    .find(|e| e.variable.value.preview.to_utf8().unwrap() == before.to_string())
                    .unwrap();
                write(d, frame, entry.write.unwrap(), &after.to_string());
            }
            assert_eq!(
                preview(d, frame, "[object[a],object[b],object[c]].join(',')"),
                "\"11,22,33\""
            );
        },
    );
}

#[test]
fn rhs_mutation_rechecks_property_descriptor() {
    for rhs in [
        "(rhs++,Object.defineProperty(object,'value',{writable:false}),99)",
        "(rhs++,Object.defineProperty(object,'value',{get(){getter++;return 8},set(v){setter++}}),99)",
        "(rhs++,delete object.value,99)",
        "(rhs++,delete object.value,Object.defineProperty(object,'value',{value:8,writable:false}),99)",
    ] {
        let rhs = rhs.to_owned();
        run(
            "let rhs=0,getter=0,setter=0;let object={value:1};debugger;if(rhs!==1||getter!==0||setter!==0)throw 'unexpected invocation';",
            move |d, _, frame| {
                let target = property(d, frame, "object", "value");
                assert!(write_result(d, frame, target, &rhs).is_err());
                assert_eq!(
                    preview(d, frame, "[rhs,getter,setter].join(',')"),
                    "\"1,0,0\""
                );
                assert_eq!(
                    preview(
                        d,
                        frame,
                        "Object.getOwnPropertyDescriptor(object,'value')?.value===99"
                    ),
                    "false"
                );
            },
        );
    }
    // Root ruling: exact receiver+atom survives deletion/recreation when current
    // descriptor is again a supported writable own DATA property.
    run(
        "let object={value:1};let replacement={marker:42};debugger;if(object.value!==replacement)throw 'RHS identity lost';",
        |d, _, frame| {
            let target = property(d, frame, "object", "value");
            write(
                d,
                frame,
                target,
                "(delete object.value,object.value=8,replacement)",
            );
            assert_eq!(preview(d, frame, "object.value===replacement"), "true");
        },
    );
}

#[test]
fn accessor_write_requires_explicit_evaluation() {
    dap_refuses_readonly(
        "let rhs=0,setter=0;let object={set value(v){setter++}};debugger;if(rhs!==0||setter!==0)throw 'implicit setter';",
        &["value"],
        true,
    );
    run(
        "let setter=0,getter=0,rhs=0;let object={get value(){getter++;return 1},set value(v){setter++}};debugger;if(setter!==1||getter!==0||rhs!==0)throw 'implicit accessor';",
        |d, _, frame| {
            let entries = properties(d, frame, "object");
            let entry = entries
                .iter()
                .find(|e| e.variable.name.to_utf8().unwrap() == "value")
                .unwrap();
            assert!(entry.variable.value.accessor);
            assert!(
                entry.write.is_none(),
                "accessor never issues direct write identity"
            );
            assert_eq!(
                preview(d, frame, "[getter,setter,rhs].join(',')"),
                "\"0,0,0\""
            );
            // Setter invocation remains available via explicitly side-effectful eval.
            eval(d, frame, "object.value=7");
            assert_eq!(
                preview(d, frame, "[getter,setter,rhs].join(',')"),
                "\"0,1,0\""
            );
        },
    );
}

#[test]
fn edited_binding_survives_gc_and_resume() {
    run(
        r#"
        function make(){let captured={marker:1};return function(){
          let local={marker:2}; debugger;
          forceGC();
          if(local.marker!==42 || captured.marker!==70 || local.same!==local) throw 'edited roots lost';
          return captured;
        }}
        let closure=make(); forceGC(); let result=closure(); forceGC();
        if(result.marker!==70) throw 'detached capture lost';
    "#,
        |d, _, frame| {
            let local = bindings(d, frame)
                .into_iter()
                .find(|e| e.variable.name.to_utf8().unwrap() == "local")
                .unwrap()
                .write
                .unwrap();
            // Cycle and nontrivial native identity cannot be reconstructed from preview.
            write(
                d,
                frame,
                local,
                "(()=>{let value={marker:42};value.same=value;forceGC();return value})()",
            );
            let captured = bindings(d, frame)
                .into_iter()
                .find(|e| e.variable.name.to_utf8().unwrap() == "captured")
                .unwrap()
                .write
                .unwrap();
            // make has returned: this captured JSVarRef is detached from its stack slot.
            write(d, frame, captured, "(forceGC(),{marker:70})");
            assert_eq!(
                preview(
                    d,
                    frame,
                    "(forceGC(),local.same===local&&captured.marker===70)"
                ),
                "true"
            );
        },
    );
}

// Every DAP refusal is an actual setVariable attempt against a fetched view.
// Keep events in the inbox while awaiting correlated terminal responses.
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
        let until = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(i) = self.inbox.iter().position(|f| f["request_seq"] == seq) {
                return self.inbox.remove(i);
            }
            assert!(std::time::Instant::now() < until, "missing response {seq}");
            self.pump();
            thread::yield_now();
        }
    }
    fn stopped(&mut self) {
        let until = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(i) = self.inbox.iter().position(|f| f["event"] == "stopped") {
                self.inbox.remove(i);
                return;
            }
            assert!(std::time::Instant::now() < until, "missing stop");
            self.pump();
            thread::yield_now();
        }
    }
}
fn dap_refuses_readonly(script: &str, names: &[&str], object: bool) {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let adapter = DapAdapter::new(debug).unwrap();
    let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    let worker = debugger::controller(&vm, move || {
        let mut dap = DapClient {
            adapter,
            seq: 0,
            inbox: Vec::new(),
        };
        let initialized = dap.request("initialize", json!({}));
        assert_eq!(
            initialized["body"]["supportsSetVariable"], true,
            "broad editing capability must be implemented"
        );
        assert_eq!(dap.request("configurationDone", json!({}))["success"], true);
        dap.stopped(); // configuration releases entry gate; actual debugger statement
        let stack = dap.request("stackTrace", json!({"threadId":1}));
        let frame = stack["body"]["stackFrames"][0]["id"].clone();
        for name in names {
            let parents: Vec<Value> = if object {
                let value = dap.request(
                    "evaluate",
                    json!({"frameId":frame,"expression":"object","context":"repl"}),
                );
                assert_eq!(value["success"], true);
                vec![value["body"]["variablesReference"].clone()]
            } else {
                let scopes = dap.request("scopes", json!({"frameId":frame}));
                assert_eq!(scopes["success"], true);
                scopes["body"]["scopes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s["variablesReference"].clone())
                    .collect()
            };
            let mut found = None;
            for parent in parents {
                let page = dap.request(
                    "variables",
                    json!({"variablesReference":parent,"start":0,"count":64}),
                );
                assert_eq!(page["success"], true);
                if page["body"]["variables"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v["name"] == name)
                {
                    found = Some(parent);
                    break;
                }
            }
            let parent = found.expect("read-only displayed member must exist");
            let edited = dap.request(
                "setVariable",
                json!({"variablesReference":parent,"name":name,"value":"(++rhs,99)"}),
            );
            assert_eq!(
                edited["success"], false,
                "read-only edit must fail: {edited}"
            );
            let counter=dap.request("evaluate",json!({"frameId":frame,"expression":if object {"[rhs,setter].join(',')"} else {"rhs"},"context":"repl"}));
            assert_eq!(counter["success"], true);
            assert_eq!(
                counter["body"]["result"],
                if object { "\"0,0\"" } else { "0" }
            );
        }
        assert_eq!(
            dap.request("continue", json!({"threadId":1}))["success"],
            true
        );
    });
    let result = vm.evaluate(source(script));
    worker.join().unwrap();
    result.unwrap();
}

fn dap_shadow_binding(dap: &mut DapClient, frame: &Value, value: &str) -> (Value, Value) {
    let scopes = dap.request("scopes", json!({"frameId": frame}));
    assert_eq!(scopes["success"], true);
    let mut selected = None;
    for scope in scopes["body"]["scopes"].as_array().unwrap() {
        let parent = &scope["variablesReference"];
        // Fetch out of order, one row at a time: selection must survive paging.
        for start in [1, 0, 2, 3, 4, 5, 6, 7] {
            let page = dap.request(
                "variables",
                json!({"variablesReference": parent, "start": start, "count": 1}),
            );
            assert_eq!(page["success"], true, "{page}");
            for variable in page["body"]["variables"].as_array().unwrap() {
                if variable["name"] == "x" && variable["value"] == value {
                    selected = Some((parent.clone(), variable["name"].clone()));
                }
            }
        }
    }
    selected.unwrap_or_else(|| panic!("displayed x={value} not found"))
}

fn dap_same_frame_shadowing(declaration: &str) {
    let mutable = declaration == "let";
    let script = format!(
        "globalThis.rhs=0; function f(){{var x=1; {{{declaration} x=2; debugger; \
         if(x!=={}||rhs!==0)throw 'inner binding changed';}} \
         if(x!==10)throw 'outer binding changed';}} f();",
        if mutable { 20 } else { 2 },
    );
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let adapter = DapAdapter::new(debug).unwrap();
    let worker = debugger::controller(&vm, move || {
        let mut dap = DapClient {
            adapter,
            seq: 0,
            inbox: vec![],
        };
        assert_eq!(dap.request("initialize", json!({}))["success"], true);
        assert_eq!(dap.request("configurationDone", json!({}))["success"], true);
        dap.stopped();
        let stack = dap.request("stackTrace", json!({"threadId": 1}));
        let frame = stack["body"]["stackFrames"][0]["id"].clone();
        if !mutable {
            let (parent, name) = dap_shadow_binding(&mut dap, &frame, "2");
            let response = dap.request(
                "setVariable",
                json!({
                    "variablesReference": parent, "name": name, "value": "(++rhs,99)"
                }),
            );
            assert_eq!(response["success"], false, "const row edited: {response}");
            let rhs = dap.request(
                "evaluate",
                json!({
                    "frameId": frame, "expression": "rhs", "context": "hover"
                }),
            );
            assert_eq!(rhs["body"]["result"], "0");
        }
        let (parent, name) = dap_shadow_binding(&mut dap, &frame, "1");
        let edited = dap.request(
            "setVariable",
            json!({
                "variablesReference": parent, "name": name, "value": "10"
            }),
        );
        assert_eq!(edited["success"], true, "{edited}");
        let inner = dap.request(
            "evaluate",
            json!({
                "frameId": frame, "expression": "x", "context": "hover"
            }),
        );
        assert_eq!(
            inner["body"]["result"], "2",
            "outer edit changed inner binding"
        );
        if mutable {
            let (parent, name) = dap_shadow_binding(&mut dap, &frame, "2");
            let edited = dap.request(
                "setVariable",
                json!({
                    "variablesReference": parent, "name": name, "value": "20"
                }),
            );
            assert_eq!(edited["success"], true, "{edited}");
        }
        assert_eq!(
            dap.request("continue", json!({"threadId": 1}))["success"],
            true
        );
    });
    let result = vm.evaluate(source(&script));
    worker.join().unwrap();
    result.unwrap();
}

#[test]
fn dap_same_frame_mutable_shadowing_edits_the_selected_row() {
    dap_same_frame_shadowing("let");
}

#[test]
fn dap_same_frame_const_shadowing_refuses_before_rhs() {
    dap_same_frame_shadowing("const");
}

struct Entered(mpsc::Sender<()>);
impl HostFunction for Entered {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.0.send(()).unwrap();
        call.import_data(JsData::Undefined)
    }
}
#[test]
fn write_rhs_cancel_deadline_and_success_keep_one_terminal_and_commit_honestly() {
    for cancel in [true, false] {
        let mut config = VmConfig::default();
        config.debug_limits.evaluation_deadline_ms = if cancel { 10_000 } else { 250 };
        let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
        let (tx, rx) = mpsc::channel();
        vm.bind_function(JsString::from_utf8("entered"), 0, Rc::new(Entered(tx)))
            .unwrap();
        let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
        let worker = debugger::controller(&vm, move || {
            let (pause, _, _) = paused(&debug);
            let frame = top(&debug, pause);
            let target = binding(&debug, frame, "local", "1");
            let id = send(
                &debug,
                DebugRequest::Write {
                    target,
                    frame,
                    expression: JsString::from_utf8(
                        "rhs++;entered();try{while(true){}}finally{cleanup++;while(true){}}",
                    ),
                },
            );
            rx.recv_timeout(Duration::from_secs(10)).unwrap();
            if cancel {
                assert!(matches!(
                    debug.cancel_evaluation(id).unwrap(),
                    DebugControlReceipt::Accepted
                ));
            }
            let expected = if cancel {
                DebugError::EvaluationCancelled {
                    effects_may_remain: true,
                }
            } else {
                DebugError::EvaluationDeadlineExceeded {
                    effects_may_remain: true,
                }
            };
            assert_eq!(reply(&debug, id).unwrap_err(), expected);
            assert_eq!(debug.state(), DebugExecutionState::Paused(pause));
            assert_eq!(
                preview(&debug, frame, "[local,rhs,cleanup].join(',')"),
                "\"1,1,0\""
            );
            assert_eq!(
                debug.cancel_evaluation(id).unwrap_err(),
                DebugError::InvalidRequest
            );
            let target = binding(&debug, frame, "local", "1");
            let committed = send(
                &debug,
                DebugRequest::Write {
                    target,
                    frame,
                    expression: JsString::from_utf8("42"),
                },
            );
            assert!(matches!(
                reply(&debug, committed).unwrap(),
                DebugReply::Write(_)
            ));
            assert_eq!(
                debug.cancel_evaluation(committed).unwrap_err(),
                DebugError::InvalidRequest
            );
            assert_eq!(preview(&debug, frame, "local"), "42");
            debug
                .continue_execution(pause, DebugContinue::Resume)
                .unwrap();
        });
        let result=vm.evaluate(source("(function(){let local=1,rhs=0,cleanup=0;debugger;if(local!==42||rhs!==1||cleanup!==0)throw 'wrong commit';})();"));
        worker.join().unwrap();
        result.unwrap();
    }
}
#[test]
fn write_rhs_stop_wins_and_does_not_resume_application() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let (tx, rx) = mpsc::channel();
    vm.bind_function(JsString::from_utf8("entered"), 0, Rc::new(Entered(tx)))
        .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let stop = vm.handle();
    let worker = debugger::controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let target = binding(&debug, frame, "local", "1");
        let id = send(
            &debug,
            DebugRequest::Write {
                target,
                frame,
                expression: JsString::from_utf8("entered();while(true){}"),
            },
        );
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        stop.request_stop().unwrap();
        assert_eq!(reply(&debug, id).unwrap_err(), DebugError::Closed);
    });
    let result = vm.evaluate(source(
        "(function(){let local=1;debugger;throw 'application resumed';})();",
    ));
    worker.join().unwrap();
    assert!(matches!(result, Err(VmError::Stopped)));
}
#[test]
fn property_target_retains_receiver_and_exact_symbol_atom_through_rhs_gc() {
    run(
        "let symbol=Symbol('duplicate'),other=Symbol('duplicate');let object={[symbol]:1,[other]:2};let original=object;debugger;if(original[symbol].marker!==42||original[other]!==2||object.changed!==true)throw 'receiver or key changed';",
        |d, _, frame| {
            let target = properties(d, frame, "object")
                .into_iter()
                .find(|e| e.variable.value.preview.to_utf8().unwrap() == "1")
                .unwrap()
                .write
                .unwrap();
            write(
                d,
                frame,
                target,
                "(object={changed:true},forceGC(),{marker:42})",
            );
            assert_eq!(preview(d, frame, "original[symbol].marker"), "42");
        },
    );
}
#[test]
fn writable_target_rejected_after_rhs_throw_and_stale_across_resume() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = debugger::controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let target = binding(&debug, frame, "local", "1");
        assert_eq!(
            write_result(&debug, frame, target, "rhs++;throw 123").unwrap_err(),
            DebugError::InvalidRequest
        );
        assert_eq!(preview(&debug, frame, "[local,rhs].join(',')"), "\"1,1\"");
        let target = binding(&debug, frame, "local", "1");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        let (next, _, _) = paused(&debug);
        let next_frame = top(&debug, next);
        assert!(matches!(
            write_result(&debug, next_frame, target, "++rhs").unwrap_err(),
            DebugError::StalePause
        ));
        assert_eq!(preview(&debug, next_frame, "rhs"), "1");
        debug
            .continue_execution(next, DebugContinue::Resume)
            .unwrap();
    });
    let result=vm.evaluate(source("(function(){let local=1,rhs=0;debugger;debugger;if(local!==1||rhs!==1)throw 'stale write';})();"));
    worker.join().unwrap();
    result.unwrap();
}
#[test]
fn synthetic_views_never_issue_property_writes() {
    for object in [
        "new Map([[1,2]])",
        "new Set([1])",
        "Promise.resolve(1)",
        "new Uint8Array([1,2])",
        "new ArrayBuffer(4)",
    ] {
        run(&format!("let object={object};debugger;"), |d, _, frame| {
            let entries = properties(d, frame, "object");
            assert!(!entries.is_empty());
            assert!(entries.iter().all(|entry| entry.write.is_none()));
        });
    }
}
#[test]
fn repeated_identical_inspection_reuses_issued_target_metadata() {
    run(
        "(function(){let local=1;let object={value:2};debugger;if(local!==42)throw 'target not retained';})();",
        |d, _, frame| {
            let object = eval(d, frame, "object").object.unwrap();
            let before = binding(d, frame, "local", "1");
            let initial = entries(d, VariableParent::Object(object))[0].write.unwrap();
            for _ in 0..1500 {
                assert_eq!(binding(d, frame, "local", "1"), before);
                assert_eq!(
                    entries(d, VariableParent::Object(object))[0].write.unwrap(),
                    initial
                );
            }
            write(d, frame, before, "42");
        },
    );
}

fn run_dap(script: &str, check: impl FnOnce(&mut DapClient, Value) + Send + 'static) {
    run_dap_config(script, VmConfig::default(), check);
}
fn run_dap_config(
    script: &str,
    config: VmConfig,
    check: impl FnOnce(&mut DapClient, Value) + Send + 'static,
) {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let adapter = DapAdapter::new(debug).unwrap();
    let worker = debugger::controller(&vm, move || {
        let mut dap = DapClient {
            adapter,
            seq: 0,
            inbox: vec![],
        };
        assert_eq!(
            dap.request("initialize", json!({"supportsInvalidatedEvent":true}))["success"],
            true
        );
        assert_eq!(dap.request("configurationDone", json!({}))["success"], true);
        dap.stopped();
        let stack = dap.request("stackTrace", json!({"threadId":1}));
        let frame = stack["body"]["stackFrames"][0]["id"].clone();
        check(&mut dap, frame);
        assert_eq!(
            dap.request("continue", json!({"threadId":1}))["success"],
            true
        );
    });
    let result = vm.evaluate(source(script));
    worker.join().unwrap();
    result.unwrap();
}
#[test]
fn dap_symbol_view_edits_selected_frame_and_emits_variables_invalidation() {
    run_dap(
        "(function(){let rhsLocal=40;let a=Symbol('same'),b=Symbol('same');let object={[a]:1,[b]:2};debugger;if(object[a]!==41||object[b]!==42)throw 'DAP selected wrong slot';})();",
        |dap, frame| {
            let object = dap.request(
                "evaluate",
                json!({"frameId":frame,"expression":"object","context":"repl"}),
            );
            let parent = object["body"]["variablesReference"].clone();
            for (before, add, expected) in [("1", 1, "41"), ("2", 2, "42")] {
                let page =
                    dap.request("variables", json!({"variablesReference":parent,"count":64}));
                assert_eq!(page["success"], true);
                let rows = page["body"]["variables"].as_array().unwrap();
                assert_ne!(rows[0]["name"], rows[1]["name"]);
                let name = rows.iter().find(|row| row["value"] == before).unwrap()["name"].clone();
                let result=dap.request("setVariable",json!({"variablesReference":parent,"name":name,"value":format!("rhsLocal+{add}")}));
                assert_eq!(result["success"], true, "{result}");
                assert_eq!(result["body"]["value"], expected);
                assert!(dap.inbox.iter().any(|event| event["event"] == "invalidated"
                    && event["body"]["areas"] == json!(["variables"])));
                dap.inbox.retain(|event| event["event"] != "invalidated");
            }
        },
    );
}
#[test]
fn dap_replacement_pages_keep_view_bounded_and_refresh_old_write_ids() {
    run_dap(
        "(function(){let object={value:1};debugger;if(object.value!==42)throw 'replacement view failed';})();",
        |dap, frame| {
            let object = dap.request(
                "evaluate",
                json!({"frameId":frame,"expression":"object","context":"repl"}),
            );
            let parent = object["body"]["variablesReference"].clone();
            for _ in 0..1500 {
                assert_eq!(
                    dap.request("variables", json!({"variablesReference":parent,"count":64}))["success"],
                    true
                );
            }
            assert_eq!(
                dap.request(
                    "setVariable",
                    json!({"variablesReference":parent,"name":"value","value":"42"})
                )["success"],
                true
            );
            // No replacement view has been fetched since invalidation.
            assert_eq!(
                dap.request(
                    "setVariable",
                    json!({"variablesReference":parent,"name":"value","value":"99"})
                )["success"],
                false
            );
        },
    );
}

#[test]
fn write_target_quota_preserves_reads_and_release_refresh_reclaims_exact_ids() {
    let mut config = VmConfig::default();
    config.debug_limits.write_targets = 2;
    config.debug_limits.inspection_objects = 8;
    run_config(
        "let rhs=0;let object={a:1,b:2,c:3};debugger;if(object.c!==42||rhs!==0)throw 'quota retry failed';",
        config,
        |d, _, frame| {
            let object = eval(d, frame, "object").object.unwrap();
            let first = entries(d, VariableParent::Object(object));
            assert_eq!(first.len(), 3);
            assert!(first[0].write.is_some());
            assert!(first[1].write.is_some());
            assert_eq!(first[2].write_unavailable, Some(WriteUnavailable::Capacity));
            assert!(first[2].variable.value.unavailable.is_none());
            let a = first[0].write.unwrap();
            let b = first[1].write.unwrap();
            assert_eq!(entries(d, VariableParent::Object(object))[0].write, Some(a));
            assert!(matches!(
                ask(d, DebugRequest::ReleaseWriteTargets(vec![a, a])),
                DebugReply::WriteTargetsReleased
            ));
            let replacement = entries(d, VariableParent::Object(object))[0].write.unwrap();
            assert_ne!(replacement, a);
            // Stale + live + duplicate members are idempotent and cannot revoke replacement.
            assert!(matches!(
                ask(d, DebugRequest::ReleaseWriteTargets(vec![a, b])),
                DebugReply::WriteTargetsReleased
            ));
            assert!(matches!(
                ask(d, DebugRequest::ReleaseWriteTargets(vec![b, b])),
                DebugReply::WriteTargetsReleased
            ));
            assert_eq!(
                entries(d, VariableParent::Object(object))[0].write,
                Some(replacement)
            );
            let refreshed = entries(d, VariableParent::Object(object));
            let targets: Vec<_> = refreshed.iter().filter_map(|e| e.write).collect();
            assert!(matches!(
                ask(d, DebugRequest::ReleaseWriteTargets(targets)),
                DebugReply::WriteTargetsReleased
            ));
            let page = match ask(
                d,
                DebugRequest::Inspect(InspectionRequest {
                    parent: VariableParent::Object(object),
                    page: Page { start: 2, count: 1 },
                    cursor: None,
                }),
            ) {
                DebugReply::Inspection(page) => page,
                other => panic!("{other:?}"),
            };
            assert_eq!(page.items[0].variable.name.to_utf8().unwrap(), "c");
            assert_eq!(
                write_result(d, frame, a, "++rhs").unwrap_err(),
                DebugError::StaleInspection
            );
            write(d, frame, page.items[0].write.unwrap(), "42");
        },
    );
}
#[test]
fn release_batch_rejects_foreign_pause_without_partially_releasing_live_target() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = debugger::controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let old = binding(&debug, frame, "local", "1");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let live = binding(&debug, frame, "local", "1");
        let id = send(&debug, DebugRequest::ReleaseWriteTargets(vec![live, old]));
        assert_eq!(reply(&debug, id).unwrap_err(), DebugError::StalePause);
        write(&debug, frame, live, "42");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    let result = vm.evaluate(source(
        "(function(){let local=1;debugger;debugger;if(local!==42)throw 'partial release';})();",
    ));
    worker.join().unwrap();
    result.unwrap();
}
#[test]
fn dap_small_target_quota_evicts_abandoned_pages_and_edits_current_page() {
    let mut config = VmConfig::default();
    config.debug_limits.write_targets = 2;
    run_dap_config(
        "let object={a:1,b:2,c:3,d:4,e:5};debugger;if(object.e!==42)throw 'evicted target not reclaimed';",
        config,
        |dap, frame| {
            let object = dap.request(
                "evaluate",
                json!({"frameId":frame,"expression":"object","context":"repl"}),
            );
            let parent = object["body"]["variablesReference"].clone();
            for start in 0..5 {
                assert_eq!(
                    dap.request(
                        "variables",
                        json!({"variablesReference":parent,"start":start,"count":1})
                    )["success"],
                    true
                );
            }
            assert_eq!(
                dap.request(
                    "setVariable",
                    json!({"variablesReference":parent,"name":"e","value":"42"})
                )["success"],
                true
            );
        },
    );
}
#[test]
fn array_special_length_is_readonly_but_ordinary_dense_elements_are_editable() {
    run(
        "let array=[1,2,3];debugger;if(array[1]!==42||array.length!==3)throw 'array invariant';",
        |d, _, frame| {
            let entries = properties(d, frame, "array");
            assert!(
                entries
                    .iter()
                    .find(|entry| entry.variable.name.to_utf8().unwrap() == "length")
                    .unwrap()
                    .write
                    .is_none()
            );
            let target = entries
                .iter()
                .find(|entry| entry.variable.name.to_utf8().unwrap() == "1")
                .unwrap()
                .write
                .unwrap();
            write(d, frame, target, "42");
        },
    );
}

#[test]
fn saturated_target_quota_does_not_relabel_const_or_tdz_as_temporary_capacity() {
    let mut config = VmConfig::default();
    config.debug_limits.write_targets = 1;
    run_config(
        "(function(){let object={value:1};const fixed=2;debugger;let later=3;})();",
        config,
        |d, _, frame| {
            let object = eval(d, frame, "object").object.unwrap();
            assert!(
                entries(d, VariableParent::Object(object))[0]
                    .write
                    .is_some()
            );
            let locals = bindings(d, frame);
            for name in ["fixed", "later"] {
                let entry = locals
                    .iter()
                    .find(|entry| entry.variable.name.to_utf8().unwrap() == name)
                    .unwrap();
                assert!(entry.write.is_none());
                assert!(
                    entry.write_unavailable.is_none(),
                    "intrinsic read-only {name}"
                );
            }
            assert_eq!(
                locals
                    .iter()
                    .find(|entry| entry.variable.name.to_utf8().unwrap() == "object")
                    .unwrap()
                    .write_unavailable,
                Some(WriteUnavailable::Capacity)
            );
        },
    );
}
#[test]
fn dap_shared_receiver_keeps_frame_context_and_cross_view_alias_ownership() {
    run_dap(
        "function outer(shared){let rhsLocal=10;function inner(shared){let rhsLocal=20;debugger;}inner(shared);if(shared.value!==21)throw 'wrong frame or alias';}outer({value:1,other:2});",
        |dap, _| {
            let stack = dap.request("stackTrace", json!({"threadId":1}));
            let frames = stack["body"]["stackFrames"].as_array().unwrap();
            let inner = frames[0]["id"].clone();
            let outer = frames[1]["id"].clone();
            let mut parents = vec![];
            for frame in [&inner, &outer] {
                let scopes = dap.request("scopes", json!({"frameId":frame}));
                let mut found = None;
                for scope in scopes["body"]["scopes"].as_array().unwrap() {
                    let vars = dap.request(
                        "variables",
                        json!({"variablesReference":scope["variablesReference"],"count":64}),
                    );
                    for row in vars["body"]["variables"].as_array().unwrap() {
                        if row["name"] == "shared" {
                            found = Some(row["variablesReference"].clone());
                        }
                    }
                }
                parents.push(found.unwrap());
            }
            assert_ne!(
                parents[0], parents[1],
                "same receiver requires frame-contextual DAP views"
            );
            for parent in &parents {
                assert_eq!(
                    dap.request(
                        "variables",
                        json!({"variablesReference":parent,"start":0,"count":1})
                    )["success"],
                    true
                );
            }
            // A disjoint page must keep earlier rows editable in BOTH alias views.
            assert_eq!(
                dap.request(
                    "variables",
                    json!({"variablesReference":parents[1],"start":1,"count":1})
                )["success"],
                true
            );
            let result = dap.request(
                "setVariable",
                json!({"variablesReference":parents[0],"name":"value","value":"rhsLocal+1"}),
            );
            assert_eq!(result["success"], true, "{result}");
            assert_eq!(result["body"]["value"], "21");
        },
    );
}

#[test]
fn failed_native_page_rolls_back_new_targets_and_preserves_published_ids() {
    let mut config = VmConfig::default();
    config.debug_limits.write_targets = 1;
    run_config(
        "(function(){let first={good:1};first['x'.repeat(70000)]=2;let second={value:3};debugger;if(second.value!==42)throw 'rollback';})();",
        config,
        |d, _, frame| {
            let vars = bindings(d, frame);
            let first = vars
                .iter()
                .find(|e| e.variable.name.to_utf8().unwrap() == "first")
                .unwrap()
                .variable
                .value
                .object
                .unwrap();
            let second = vars
                .iter()
                .find(|e| e.variable.name.to_utf8().unwrap() == "second")
                .unwrap()
                .variable
                .value
                .object
                .unwrap();
            let targets = vars.iter().filter_map(|e| e.write).collect();
            ask(d, DebugRequest::ReleaseWriteTargets(targets));
            let id = send(
                d,
                DebugRequest::Inspect(InspectionRequest {
                    parent: VariableParent::Object(first),
                    page: Page {
                        start: 0,
                        count: 64,
                    },
                    cursor: None,
                }),
            );
            assert_eq!(reply(d, id).unwrap_err(), DebugError::Capacity);
            let target = entries(d, VariableParent::Object(second))[0]
                .write
                .expect("failed page must reclaim its new target quota");
            write(d, frame, target, "42");
        },
    );
    run(
        "let object={good:1};object['x'.repeat(70000)]=2;debugger;if(object.good!==42)throw 'published target rolled back';",
        |d, _, frame| {
            let object = eval(d, frame, "object").object.unwrap();
            let first = match ask(
                d,
                DebugRequest::Inspect(InspectionRequest {
                    parent: VariableParent::Object(object),
                    page: Page { start: 0, count: 1 },
                    cursor: None,
                }),
            ) {
                DebugReply::Inspection(page) => page,
                other => panic!("{other:?}"),
            };
            let target = first.items[0].write.unwrap();
            let id = send(
                d,
                DebugRequest::Inspect(InspectionRequest {
                    parent: VariableParent::Object(object),
                    page: Page {
                        start: 0,
                        count: 64,
                    },
                    cursor: None,
                }),
            );
            assert_eq!(reply(d, id).unwrap_err(), DebugError::Capacity);
            write(d, frame, target, "42");
        },
    );
}
#[test]
fn write_rhs_detach_cancels_without_commit_and_resumes_restored_application() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let (tx, rx) = mpsc::channel();
    vm.bind_function(JsString::from_utf8("entered"), 0, Rc::new(Entered(tx)))
        .unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = debugger::controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = top(&debug, pause);
        let target = binding(&debug, frame, "local", "1");
        let id = send(
            &debug,
            DebugRequest::Write {
                target,
                frame,
                expression: JsString::from_utf8("entered();while(true){}"),
            },
        );
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        debug.detach().unwrap();
        let mut terminal = 0;
        loop {
            match debugger::update(&debug) {
                DebugUpdate::Reply { id: actual, result } if actual == id => {
                    terminal += 1;
                    assert_eq!(
                        result.unwrap_err(),
                        DebugError::EvaluationCancelled {
                            effects_may_remain: true
                        }
                    );
                }
                DebugUpdate::SessionClosed { .. } => break,
                _ => (),
            }
        }
        while let Some(update) = debug.try_recv() {
            if let DebugUpdate::Reply { id: actual, result } = update {
                assert_eq!(actual, id);
                terminal += 1;
                assert_eq!(
                    result.unwrap_err(),
                    DebugError::EvaluationCancelled {
                        effects_may_remain: true
                    }
                );
            }
        }
        assert_eq!(terminal, 1);
    });
    let result=vm.evaluate(source("globalThis.resumed=0;(function(){let local=1;debugger;if(local!==1)throw 'detached write committed';resumed=42;})();"));
    worker.join().unwrap();
    result.unwrap();
    assert_eq!(vm.debug_state(), DebugExecutionState::Detached);
    vm.evaluate(source("if(resumed!==42)throw 'not resumed';"))
        .unwrap();
}
#[test]
fn original_application_exception_survives_failed_and_successful_binding_edits() {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let policy = send(
        &debug,
        DebugRequest::ExceptionPolicy(ExceptionPolicy::Uncaught),
    );
    let worker = debugger::controller(&vm, move || {
        reply(&debug, policy).unwrap();
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Exception);
        let frame = top(&debug, pause);
        let target = binding(&debug, frame, "local", "1");
        assert_eq!(
            write_result(&debug, frame, target, "throw 99").unwrap_err(),
            DebugError::InvalidRequest
        );
        let target = binding(&debug, frame, "local", "1");
        write(&debug, frame, target, "42");
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    let result = vm.evaluate(source(
        "function application(){let local=1;throw 73;}application();",
    ));
    worker.join().unwrap();
    let Err(VmError::Execution(report)) = result else {
        panic!("{result:?}")
    };
    assert_eq!(report.thrown, Some(JsData::Number(73.0)));
}
#[test]
fn zero_or_overflowing_edit_target_quotas_fail_vm_configuration() {
    for quota in [0, usize::MAX] {
        let mut config = VmConfig::default();
        config.debug_limits.write_targets = quota;
        assert!(matches!(
            VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config),
            Err(VmError::Configuration(_))
        ));
    }
}

#[test]
fn dap_many_capacity_pages_bound_names_keep_editable_rows_and_refresh_current_target() {
    let mut config = VmConfig::default();
    config.debug_limits.write_targets = 1;
    config.debug_limits.inspection_objects = 8;
    config.debug_limits.inspection_page_items = 2;
    run_dap_config(
        "let object={};for(let i=0;i<400;i++)object['p'+i]=i;debugger;if(object.p399!==42)throw 'capacity-row refresh failed';",
        config,
        |dap, frame| {
            let object = dap.request(
                "evaluate",
                json!({"frameId":frame,"expression":"object","context":"repl"}),
            );
            let parent = object["body"]["variablesReference"].clone();
            for start in (0..400).step_by(2) {
                let page = dap.request(
                    "variables",
                    json!({"variablesReference":parent,"start":start,"count":2}),
                );
                assert_eq!(page["success"], true, "start {start}: {page}");
                assert_eq!(page["body"]["variables"].as_array().unwrap().len(), 2);
            }
            // Capacity-name eviction must preserve the original editable row.
            assert_eq!(
                dap.request(
                    "setVariable",
                    json!({"variablesReference":parent,"name":"p0","value":"0"})
                )["success"],
                true
            );
            // Refresh the requested original position after edit invalidation.
            let page = dap.request(
                "variables",
                json!({"variablesReference":parent,"start":399,"count":1}),
            );
            assert_eq!(page["success"], true, "{page}");
            assert_eq!(page["body"]["variables"][0]["name"], "p399");
            assert_eq!(
                dap.request(
                    "setVariable",
                    json!({"variablesReference":parent,"name":"p399","value":"42"})
                )["success"],
                true
            );
        },
    );
}
