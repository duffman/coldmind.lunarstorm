//! Exact executable UTF-16 positions through the production compiler, VM and DAP.
//! Public discovery ranges are inclusive; replies are complete, sorted and unique.
#![cfg(feature = "quickjs-ng")]
use coldmind_runtime::{debug::*, engine::*, engines::quickjs_ng::QuickJsNgEngine, vm::*};
use serde_json::{Value, json};
use std::{
    thread,
    time::{Duration, Instant},
};
#[path = "support/debugger.rs"]
mod debugger;
use debugger::*;

const ORIGIN: &str = "file:///g007-locations.js";
// Exact compiler evidence: one-based (1,8), (1,17), (2,8).
// No extra statement is appended: the complete marker list is already proved.
const PROGRAM: &str = "/*😀*/ let a=1; a++;\r\n/*😀*/ a++;";
const POSITIONS: [(u32, u32); 3] = [(0, 7), (0, 16), (1, 7)];
fn source() -> ScriptSource {
    ScriptSource::new(JsString::from_utf8(PROGRAM), ORIGIN.into())
}
fn selector() -> ScriptSelector {
    ScriptSelector {
        origin: ORIGIN.into(),
        expected_digest: None,
    }
}
fn position((line, column): (u32, u32)) -> SourcePosition {
    SourcePosition { line, column }
}
fn coordinates(location: &DebugLocation) -> (u32, u32) {
    (
        location.line,
        location
            .column
            .expect("verified executable metadata needs a column"),
    )
}
fn locations(debug: &VmDebugHandle, start: (u32, u32), end: (u32, u32)) -> Vec<DebugLocation> {
    match ask(
        debug,
        DebugRequest::BreakpointLocations {
            selector: selector(),
            start: position(start),
            end: position(end),
        },
    ) {
        DebugReply::Locations(found) => found,
        other => panic!("expected native executable locations, got {other:?}"),
    }
}
fn discover(debug: &VmDebugHandle) -> Vec<DebugLocation> {
    let found = locations(debug, (0, 0), (1, 11));
    assert_eq!(found.iter().map(coordinates).collect::<Vec<_>>(), POSITIONS);
    assert!(
        found
            .iter()
            .all(|p| p.script == found[0].script && p.revision == found[0].revision)
    );
    found
}
fn configured(debug: &VmDebugHandle, wanted: &[(u32, u32)]) -> Vec<BreakpointInfo> {
    match ask(
        debug,
        DebugRequest::SetBreakpoints {
            selector: selector(),
            breakpoints: wanted
                .iter()
                .enumerate()
                .map(|(index, &(line, column))| BreakpointSpec {
                    client_id: 700 + index as u64,
                    line,
                    column: Some(column),
                    condition: None,
                    enabled: true,
                    hit_condition: None,
                    log_message: None,
                })
                .collect(),
        },
    ) {
        DebugReply::Breakpoints(infos) => infos,
        other => panic!("{other:?}"),
    }
}
fn assert_location(actual: &DebugLocation, expected: &DebugLocation) {
    assert_eq!(actual.script, expected.script);
    assert_eq!(actual.revision, expected.revision);
    assert_eq!(coordinates(actual), coordinates(expected));
}
fn assert_exact_binding(info: &BreakpointInfo, id: u64, expected: &DebugLocation) {
    assert_eq!(info.client_id, id);
    let BreakpointState::Verified(actual) = &info.state else {
        panic!(
            "exact executable request was not verified: {:?}",
            info.state
        )
    };
    assert_eq!(
        actual.len(),
        1,
        "a column must not install every marker on its line"
    );
    assert_location(&actual[0], expected);
}
fn continue_to_hit(debug: &VmDebugHandle, pause: PauseId, expected: &DebugLocation) -> PauseId {
    debug
        .continue_execution(pause, DebugContinue::Resume)
        .unwrap();
    let (pause, reason, actual) = paused(debug);
    assert_eq!(reason, PauseReason::Breakpoint);
    assert_location(&actual.expect("breakpoint stop location"), expected);
    let DebugReply::Stack { frames, .. } = ask(
        debug,
        DebugRequest::Stack {
            pause,
            page: Page { start: 0, count: 1 },
        },
    ) else {
        panic!("stack reply")
    };
    assert_eq!(frames.len(), 1);
    assert_location(
        frames[0].generated.as_ref().expect("native frame position"),
        expected,
    );
    pause
}
fn run_native(serialized: bool, check: impl FnOnce(&VmDebugHandle, PauseId) + Send + 'static) {
    run_native_config(VmConfig::default(), serialized, check);
}
fn run_native_config(
    config: VmConfig,
    serialized: bool,
    check: impl FnOnce(&VmDebugHandle, PauseId) + Send + 'static,
) {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let artifact = if serialized {
        let compiled = vm
            .compile(source(), QuickJsNgEngine::debug_compile_options())
            .unwrap();
        assert!(!compiled.payload().is_empty());
        let bytes = compiled.payload().to_vec();
        let metadata = compiled.metadata().clone();
        let retained = compiled.source().clone();
        drop(compiled); // native execution receives only the independently owned artifact
        // SAFETY: bytes are an unmodified copy of this VM's production compiler output;
        // retained source and compatibility metadata are copied without alteration.
        Some(
            unsafe { CompiledArtifact::from_trusted_compiler_output(metadata, bytes, retained) }
                .unwrap(),
        )
    } else {
        None
    };
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let worker = controller(&vm, move || {
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Entry);
        check(&debug, pause);
        debug
    });
    let value = match artifact {
        Some(artifact) => vm.execute_artifact(artifact).unwrap(),
        None => vm.evaluate(source()).unwrap(),
    };
    let _debug = worker.join().unwrap();
    drop(value);
    // A wrongly installed trailing marker leaves the owner paused and triggers the
    // shared watchdog; do not silently consume unexpected additional breakpoint stops.
    assert_eq!(vm.debug_state(), DebugExecutionState::Running);
}

#[test]
fn multiple_statements_same_line_have_verified_distinct_locations() {
    run_native(false, |debug, entry| {
        let found = discover(debug);
        // Install ONLY the second statement: accidentally installing the first
        // marker on this line must produce an observable wrong first stop.
        let infos = configured(debug, &[POSITIONS[1], POSITIONS[2]]);
        assert_eq!(infos.len(), 2);
        assert_exact_binding(&infos[0], 700, &found[1]);
        assert_exact_binding(&infos[1], 701, &found[2]);
        let pause = continue_to_hit(debug, entry, &found[1]);
        let pause = continue_to_hit(debug, pause, &found[2]);
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
}
#[test]
fn astral_and_crlf_columns_roundtrip() {
    run_native(false, |debug, entry| {
        let found = discover(debug);
        // Full discovery + both same-line markers + next CRLF line all remain exact.
        assert_eq!(
            locations(debug, (0, 0), (0, 16))
                .iter()
                .map(coordinates)
                .collect::<Vec<_>>(),
            &POSITIONS[..2]
        );
        assert_eq!(
            locations(debug, (0, 8), (0, 16))
                .iter()
                .map(coordinates)
                .collect::<Vec<_>>(),
            vec![POSITIONS[1]]
        );
        assert!(
            locations(debug, (0, 0), (0, 6)).is_empty(),
            "comment is not executable"
        );
        let infos = configured(debug, &POSITIONS);
        assert_eq!(infos.len(), 3);
        for (index, info) in infos.iter().enumerate() {
            assert_exact_binding(info, 700 + index as u64, &found[index]);
        }
        // Entry is already suspended at the first marker. Installing a breakpoint
        // there does not replay that same hook after resume.
        let DebugReply::Stack { frames, .. } = ask(
            debug,
            DebugRequest::Stack {
                pause: entry,
                page: Page { start: 0, count: 1 },
            },
        ) else {
            panic!("stack reply")
        };
        assert_location(frames[0].generated.as_ref().unwrap(), &found[0]);
        let mut pause = entry;
        for expected in &found[1..] {
            pause = continue_to_hit(debug, pause, expected);
        }
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
}
#[test]
fn non_executable_requested_location_is_not_falsely_verified() {
    run_native(false, |debug, entry| {
        let found = discover(debug);
        // The selected policy relocates forward to the next real marker.
        // Column 8 lies inside `let`, after the actual column-7 marker.
        let infos = configured(debug, &[(0, 8)]);
        assert_eq!(infos.len(), 1);
        assert_exact_binding(&infos[0], 700, &found[1]);
        let pause = continue_to_hit(debug, entry, &found[1]);
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
}
#[test]
fn debug_artifact_retains_executable_columns() {
    run_native(true, |debug, entry| {
        let found = discover(debug);
        let infos = configured(debug, &[POSITIONS[1], POSITIONS[2]]);
        assert_eq!(infos.len(), 2);
        assert_exact_binding(&infos[0], 700, &found[1]);
        assert_exact_binding(&infos[1], 701, &found[2]);
        let pause = continue_to_hit(debug, entry, &found[1]);
        let pause = continue_to_hit(debug, pause, &found[2]);
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
}

#[test]
fn location_discovery_rejects_reversed_ranges_without_losing_pause() {
    run_native(false, |debug, entry| {
        assert!(matches!(
            debug.try_request(DebugRequest::BreakpointLocations {
                selector: selector(),
                start: position((1, 7)),
                end: position((0, 16)),
            }),
            Err(Rejected {
                reason: RejectionReason::Debug(DebugError::InvalidSource),
                ..
            })
        ));
        discover(debug);
        debug
            .continue_execution(entry, DebugContinue::Resume)
            .unwrap();
    });
}
#[test]
fn location_discovery_capacity_returns_error_without_partial_success() {
    for work_limited in [true, false] {
        let mut config = VmConfig::default();
        if work_limited {
            config.debug_limits.location_scan_slots = 1;
        } else {
            config.debug_limits.reply_bytes =
                std::mem::size_of::<DebugReply>() + 2 * std::mem::size_of::<DebugLocation>();
        }
        run_native_config(config, false, |debug, entry| {
            let id = send(
                debug,
                DebugRequest::BreakpointLocations {
                    selector: selector(),
                    start: position((0, 0)),
                    end: position((1, 11)),
                },
            );
            assert!(matches!(reply(debug, id), Err(DebugError::Capacity)));
            // The failed query cannot invalidate the pause or install a breakpoint.
            debug
                .continue_execution(entry, DebugContinue::Resume)
                .unwrap();
        });
    }
}

// Adapted from the actual native DAP client in quickjs_ng_debug_editing.rs.
// Preserves notifications while waiting for correlated replies; no fixture backend.
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
fn dap_case(lines_one: bool, columns_one: bool, defaults: bool) {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let adapter = DapAdapter::new(debug).unwrap();
    let mut dap = DapClient {
        adapter,
        seq: 0,
        inbox: vec![],
    };
    let line_base = u32::from(lines_one);
    let column_base = u32::from(columns_one);
    let init = dap.request(
        "initialize",
        if defaults {
            json!({})
        } else {
            json!({"linesStartAt1":lines_one,"columnsStartAt1":columns_one})
        },
    );
    assert_eq!(init["body"]["supportsBreakpointLocationsRequest"], true);
    assert_eq!(init["body"]["jetbunny"]["supportsStepPolicy"], true);
    let policy = dap.submit("jetbunny/setStepPolicy", json!({"skipSources":[ORIGIN]}));
    // Queue configuration before owner entry, ensuring a genuine pending reply
    // followed by later verified breakpoint events with actual bound positions.
    let initial_seq = dap.submit("setBreakpoints", json!({"source":{"path":ORIGIN},"breakpoints":[{"line":line_base,"column":16+column_base},{"line":1+line_base,"column":7+column_base}]}));
    let worker = controller(&vm, move || {
        dap.response("jetbunny/setStepPolicy", policy);
        let initial = dap.response("setBreakpoints", initial_seq);
        assert!(
            initial["body"]["breakpoints"]
                .as_array()
                .unwrap()
                .iter()
                .all(|b| b["verified"] == false)
        );
        dap.request("configurationDone", json!({}));
        dap.stopped("breakpoint");
        dap.stack_position((line_base, 16 + column_base));
        for (breakpoint, &(line, column)) in initial["body"]["breakpoints"]
            .as_array()
            .unwrap()
            .iter()
            .zip(&POSITIONS[1..])
        {
            if breakpoint["verified"] == false {
                let event = dap.take("verified breakpoint event", |event| {
                    event["event"] == "breakpoint"
                        && event["body"]["breakpoint"]["id"] == breakpoint["id"]
                        && event["body"]["breakpoint"]["verified"] == true
                });
                assert_eq!(event["body"]["breakpoint"]["line"], line + line_base);
                assert_eq!(event["body"]["breakpoint"]["column"], column + column_base);
            }
        }
        let stack = dap.request("stackTrace", json!({"threadId":1,"levels":1}));
        let src = stack["body"]["stackFrames"][0]["source"].clone();
        assert!(src["sourceReference"].as_u64().unwrap() > 0);
        let retained = dap.request(
            "source",
            json!({"source":src.clone(),"sourceReference":src["sourceReference"]}),
        );
        assert_eq!(
            retained["body"]["content"], PROGRAM,
            "astral and CRLF must survive source retrieval"
        );
        let discovery = dap.request("breakpointLocations", json!({"source":src.clone(),"line":line_base,"column":column_base,"endLine":1+line_base,"endColumn":11+column_base}));
        let actual: Vec<_> = discovery["body"]["breakpoints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["line"].as_u64().unwrap(), p["column"].as_u64().unwrap()))
            .collect();
        let expected: Vec<_> = POSITIONS
            .iter()
            .map(|&(l, c)| ((l + line_base) as u64, (c + column_base) as u64))
            .collect();
        assert_eq!(actual, expected);
        for location in discovery["body"]["breakpoints"].as_array().unwrap() {
            assert!(location.get("verified").is_none());
            assert!(location.get("id").is_none());
        }
        // Omitted column/endLine/endColumn selects all executable locations on
        // that line, independently of the SourceBreakpoint optional-column rule.
        let line_only = dap.request(
            "breakpointLocations",
            json!({"source":{"sourceReference":src["sourceReference"]},"line":line_base}),
        );
        assert_eq!(
            line_only["body"]["breakpoints"],
            json!([
                {"line":line_base,"column":7+column_base},
                {"line":line_base,"column":16+column_base}
            ])
        );
        let single = dap.request("breakpointLocations", json!({"source":src.clone(),"line":line_base,"column":16+column_base,"endColumn":16+column_base}));
        assert_eq!(
            single["body"]["breakpoints"],
            json!([{"line":line_base,"column":16+column_base}])
        );
        // Non-executable inside-token request must expose actual relocation in
        // the response, not echo the client's requested column as verified.
        let nonexec = dap.request(
            "setBreakpoints",
            json!({"source":src.clone(),"breakpoints":[{"line":line_base,"column":8+column_base}]}),
        );
        let nonexec = &nonexec["body"]["breakpoints"][0];
        assert_eq!(nonexec["verified"], true);
        assert_eq!(nonexec["line"], line_base);
        assert_eq!(nonexec["column"], 16 + column_base);
        assert_eq!(nonexec["source"]["sourceReference"], src["sourceReference"]);
        // Replace nonexec configuration before resuming, then check exact hits.
        let installed = dap.request("setBreakpoints", json!({"source":src,"breakpoints":[{"line":1+line_base,"column":7+column_base},{"line":line_base,"column":16+column_base}]}));
        let breakpoints = installed["body"]["breakpoints"].as_array().unwrap();
        assert_eq!(breakpoints.len(), 2);
        for (actual, &(line, column)) in breakpoints.iter().zip(&[POSITIONS[2], POSITIONS[1]]) {
            assert_eq!(actual["verified"], true);
            assert_eq!(actual["line"], line + line_base);
            assert_eq!(actual["column"], column + column_base);
        }
        for &(line, column) in &POSITIONS[2..] {
            dap.request("continue", json!({"threadId":1}));
            dap.stopped("breakpoint");
            dap.stack_position((line + line_base, column + column_base));
        }
        dap.request("continue", json!({"threadId":1}));
        dap
    });
    let result = vm.evaluate(source());
    let _dap = worker.join().unwrap();
    drop(result.unwrap());
    assert_eq!(vm.debug_state(), DebugExecutionState::Running);
}
#[test]
fn dap_native_locations_and_column_hits_use_one_based_utf16() {
    dap_case(true, true, true);
}
#[test]
fn dap_native_locations_and_column_hits_use_zero_based_utf16() {
    dap_case(false, false, false);
}

#[test]
fn dap_native_locations_independently_convert_one_based_lines() {
    dap_case(true, false, false);
}
#[test]
fn dap_native_locations_independently_convert_one_based_columns() {
    dap_case(false, true, false);
}

fn map_segments(original: &[(u32, u32)]) -> String {
    fn vlq(value: i64) -> String {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut value = ((value.unsigned_abs()) << 1) | u64::from(value < 0);
        let mut text = String::new();
        loop {
            let mut digit = (value & 31) as usize;
            value >>= 5;
            if value != 0 {
                digit |= 32;
            }
            text.push(alphabet[digit] as char);
            if value == 0 {
                return text;
            }
        }
    }
    let (mut line, mut column, mut original_line, mut original_column) = (0, 0, 0, 0);
    let mut result = String::new();
    for (&(generated_line, generated_column), &(next_line, next_column)) in
        POSITIONS.iter().zip(original)
    {
        if generated_line != line {
            result.push_str(&";".repeat((generated_line - line) as usize));
            line = generated_line;
            column = 0;
        } else if !result.is_empty() {
            result.push(',');
        }
        for value in [
            i64::from(generated_column) - i64::from(column),
            0,
            i64::from(next_line) - i64::from(original_line),
            i64::from(next_column) - i64::from(original_column),
        ] {
            result.push_str(&vlq(value));
        }
        column = generated_column;
        original_line = next_line;
        original_column = next_column;
    }
    result
}

#[test]
fn typescript_chained_indexed_maps_and_missing_originals_are_truthful() {
    const ORIGINAL: &str = "/*😀*/ let a: number=1; a++;\r\n/*😀*/ a++;";
    const ORIGINAL_POSITIONS: [(u32, u32); 3] = [(0, 7), (0, 24), (1, 7)];
    for content_available in [true, false] {
        for one_based in [true, false] {
            let base = u32::from(one_based);
            let mut vm =
                VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
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
            dap.request(
                "initialize",
                json!({"linesStartAt1":one_based,"columnsStartAt1":one_based}),
            );
            let setup = dap.submit(
                "setBreakpoints",
                json!({
                    "source":{"path":"file:///original.ts"},
                    "breakpoints":[{"line":base,"column":7+base}]
                }),
            );
            let worker = controller(&vm, move || {
                dap.response("setBreakpoints", setup);
                dap.request("configurationDone", json!({}));
                dap.stopped("breakpoint");
                let stack = dap.request("stackTrace", json!({"threadId":1,"levels":1}));
                let frame = &stack["body"]["stackFrames"][0];
                assert_eq!(frame["line"], base);
                assert_eq!(frame["column"], 7 + base);
                let source = frame["source"].clone();
                assert!(source["path"].as_str().unwrap().ends_with("original.ts"));
                assert_eq!(
                    source["sourceReference"].as_u64().unwrap_or(0) > 0,
                    content_available
                );
                let found = dap.request(
                    "breakpointLocations",
                    json!({
                        "source":{"path":"file:///original.ts"},
                        "line":base,"column":base,"endLine":1+base,"endColumn":100+base
                    }),
                );
                assert_eq!(
                    found["body"]["breakpoints"],
                    json!([
                        {"line":base,"column":7+base},
                        {"line":base,"column":24+base},
                        {"line":1+base,"column":7+base}
                    ])
                );
                let seq = dap.submit("source", json!({"source":source}));
                let response =
                    dap.take("original content", |message| message["request_seq"] == seq);
                assert_eq!(response["success"], content_available);
                if content_available {
                    assert_eq!(response["body"]["content"], ORIGINAL);
                }
                dap.request("continue", json!({"threadId":1}));
                dap
            });
            let mut script = source();
            script.source_map = Some(SourceMapAttachment {
                json: serde_json::to_vec(&json!({"version":3,"sections":[{
                    "offset":{"line":0,"column":0},"map":{
                        "version":3,"sources":["middle.js"],"sourcesContent":[PROGRAM],
                        "names":[],"mappings":map_segments(&POSITIONS)
                    }
                }]}))
                .unwrap(),
                originals: vec![],
                preceding_maps: vec![
                    serde_json::to_vec(&json!({
                        "version":3,"file":"middle.js","sources":["original.ts"],
                        "sourcesContent":[if content_available {Some(ORIGINAL)} else {None}],
                        "names":[],"mappings":map_segments(&ORIGINAL_POSITIONS)
                    }))
                    .unwrap(),
                ],
            });
            let result = vm.evaluate(script);
            let _dap = worker.join().unwrap();
            drop(result.unwrap());
        }
    }
}

#[test]
fn same_uri_replacement_keeps_old_digest_pinned() {
    const URI: &str = "file:///revision.js";
    const ORIGINAL: &str = "file:///revision.ts";
    let old = "globalThis.version=1;\nglobalThis.version+=10;";
    let new = "globalThis.version=2;\nglobalThis.version+=20;";
    let old_original = "globalThis.version=1 as number;\nglobalThis.version+=10;";
    let new_original = "globalThis.version=2 as number;\nglobalThis.version+=20;";
    let old_digest = SourceDigest::of_utf16(&JsString::from_utf8(old));
    let old_original_digest = SourceDigest::of_utf16(&JsString::from_utf8(old_original));
    let mapped = |text: &str, original: &str| {
        let mut source = ScriptSource::new(JsString::from_utf8(text), URI.into());
        source.source_map = Some(SourceMapAttachment {
            json: serde_json::to_vec(&json!({
                "version":3,"sources":["revision.ts"],"sourcesContent":[original],
                "names":[],"mappings":"AAAA;AACA"
            }))
            .unwrap(),
            originals: vec![],
            preceding_maps: vec![],
        });
        source
    };
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), VmConfig::default()).unwrap();
    drop(vm.evaluate(mapped(old, old_original)).unwrap());
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let worker = controller(&vm, move || {
        let (entry, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::Entry);
        let DebugReply::Sources { scripts, .. } = ask(
            &debug,
            DebugRequest::Sources {
                page: Page {
                    start: 0,
                    count: 16,
                },
            },
        ) else {
            panic!("sources");
        };
        let revisions: Vec<_> = scripts
            .iter()
            .filter(|source| source.origin == URI)
            .collect();
        assert_eq!(revisions.len(), 2);
        let before = revisions
            .iter()
            .find(|source| source.registration.digest == old_digest)
            .unwrap()
            .registration;
        let after = revisions
            .iter()
            .find(|source| source.registration.digest != old_digest)
            .unwrap()
            .registration;
        assert_ne!(before.id, after.id);
        assert!(before.revision < after.revision);
        let DebugReply::Source { text, .. } = ask(
            &debug,
            DebugRequest::Source {
                script: before.id,
                revision: before.revision,
                page: Page {
                    start: 0,
                    count: 64,
                },
            },
        ) else {
            panic!("retained old source");
        };
        assert_eq!(text.to_utf8().unwrap(), old);
        let DebugReply::Source { text, .. } = ask(
            &debug,
            DebugRequest::OriginalSource {
                selector: ScriptSelector {
                    origin: ORIGINAL.into(),
                    expected_digest: Some(old_original_digest),
                },
                page: Page {
                    start: 0,
                    count: 64,
                },
            },
        ) else {
            panic!("retained old original");
        };
        assert_eq!(text.to_utf8().unwrap(), old_original);
        for (digest, expected, id) in [(Some(old_original_digest), before, 41), (None, after, 42)] {
            let selector = ScriptSelector {
                origin: ORIGINAL.into(),
                expected_digest: digest,
            };
            let DebugReply::OriginalLocations(locations) = ask(
                &debug,
                DebugRequest::BreakpointLocations {
                    selector: selector.clone(),
                    start: position((0, 0)),
                    end: position((1, 100)),
                },
            ) else {
                panic!("original discovery");
            };
            assert_eq!(locations.len(), 2);
            assert!(locations.iter().all(|at| at.digest
                == Some(if digest.is_some() {
                    old_original_digest
                } else {
                    SourceDigest::of_utf16(&JsString::from_utf8(new_original))
                })));
            let DebugReply::Breakpoints(infos) = ask(
                &debug,
                DebugRequest::SetBreakpoints {
                    selector,
                    breakpoints: vec![BreakpointSpec {
                        client_id: id,
                        line: 1,
                        ..Default::default()
                    }],
                },
            ) else {
                panic!("breakpoints");
            };
            let BreakpointState::Verified(locations) = &infos[0].state else {
                panic!("unverified");
            };
            assert!(
                locations
                    .iter()
                    .all(|at| at.script == expected.id && at.revision == expected.revision)
            );
        }
        debug
            .continue_execution(entry, DebugContinue::Resume)
            .unwrap();
        let (pause, reason, location) = paused(&debug);
        assert_eq!(reason, PauseReason::Breakpoint);
        let location = location.unwrap();
        assert_eq!(
            (location.script, location.revision),
            (after.id, after.revision)
        );
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    let result = vm.evaluate(mapped(new, new_original));
    worker.join().unwrap();
    drop(result.unwrap());
}

#[test]
fn original_location_reply_charges_uri_storage_and_keeps_pause_usable() {
    let origin = format!("file:///{}.ts", "x".repeat(1024));
    let selected = origin.clone();
    let mut config = VmConfig::default();
    config.debug_limits.reply_bytes = 512;
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let worker = controller(&vm, move || {
        let (pause, reason, _) = paused(&debug);
        assert_eq!(reason, PauseReason::DebuggerStatement);
        let id = send(
            &debug,
            DebugRequest::BreakpointLocations {
                selector: ScriptSelector {
                    origin: selected,
                    expected_digest: None,
                },
                start: position((0, 0)),
                end: position((1, 100)),
            },
        );
        assert!(matches!(reply(&debug, id), Err(DebugError::Capacity)));
        assert_eq!(debug.state(), DebugExecutionState::Paused(pause));
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    let mut script = ScriptSource::new(
        JsString::from_utf8(&format!("debugger;\n{PROGRAM}")),
        ORIGIN.into(),
    );
    script.source_map = Some(SourceMapAttachment {
        json: serde_json::to_vec(&json!({"version":3,"sections":[{
            "offset":{"line":1,"column":0},"map":{
                "version":3,"sources":[origin],"sourcesContent":[PROGRAM],
                "names":[],"mappings":map_segments(&POSITIONS)
            }
        }]}))
        .unwrap(),
        originals: vec![],
        preceding_maps: vec![],
    });
    let result = vm.evaluate(script);
    worker.join().unwrap();
    drop(result.unwrap());
}
