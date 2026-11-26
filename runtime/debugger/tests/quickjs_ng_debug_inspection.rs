//! Bounded native inspection acceptance, using the production compiler and VM.
#![cfg(feature = "quickjs-ng")]
use coldmind_runtime::{debug::*, engine::*, engines::quickjs_ng::QuickJsNgEngine, vm::*};
#[path = "support/debugger.rs"]
mod debugger;
use debugger::*;
fn inspect(d: &VmDebugHandle, parent: VariableParent, cursor: Option<CursorRef>) -> InspectionPage {
    inspect_at(d, parent, 0, cursor)
}
fn inspect_at(
    d: &VmDebugHandle,
    parent: VariableParent,
    start: usize,
    cursor: Option<CursorRef>,
) -> InspectionPage {
    match ask(
        d,
        DebugRequest::Inspect(InspectionRequest {
            parent,
            page: Page { start, count: 64 },
            cursor,
        }),
    ) {
        DebugReply::Inspection(page) => page,
        other => panic!("{other:?}"),
    }
}
fn run(script: &str, check: impl FnOnce(&VmDebugHandle, PauseId, FrameRef) + Send + 'static) {
    run_config(script, VmConfig::default(), check);
}
fn run_config(
    script: &str,
    config: VmConfig,
    check: impl FnOnce(&VmDebugHandle, PauseId, FrameRef) + Send + 'static,
) {
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let debug = vm.attach_debugger(DebugSessionOptions::default()).unwrap();
    let c = controller(&vm, move || {
        let (pause, _, _) = paused(&debug);
        let frame = match ask(
            &debug,
            DebugRequest::Stack {
                pause,
                page: Page { start: 0, count: 1 },
            },
        ) {
            DebugReply::Stack { frames, .. } => frames[0].id,
            x => panic!("{x:?}"),
        };
        check(&debug, pause, frame);
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
    });
    vm.evaluate(ScriptSource::new(
        JsString::from_utf8(script),
        "file:///inspection.js".into(),
    ))
    .unwrap();
    c.join().unwrap();
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
        DebugReply::Evaluation { value, .. } => value,
        x => panic!("{x:?}"),
    }
}
#[test]
fn huge_object_first_page_does_not_snapshot_all_keys() {
    run(
        "let object={}; for(let i=0;i<100000;i++) object['p'+i]=i; debugger;",
        |d, _, frame| {
            let object = eval(d, frame, "object").object.unwrap();
            let page = inspect(d, VariableParent::Object(object), None);
            assert_eq!(page.items.len(), 64);
            assert!(page.scanned_slots <= 4096);
            assert!(!page.complete);
            assert!(page.next.is_some());
            assert_eq!(page.total, None);
        },
    );
}
#[test]
fn sparse_page_progress_is_not_false_completion() {
    run(
        "let object={};for(let i=0;i<10000;i++)object['p'+i]=i;for(let i=0;i<4999;i++)delete object['p'+i];debugger;",
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let first = inspect(d, parent, None);
            assert_eq!(first.scanned_slots, 4096);
            assert!(first.items.is_empty());
            assert!(!first.complete);
            let mut cursor = first.next;
            let mut count = 0;
            let mut calls = 0;
            while cursor.is_some() {
                let page = inspect(d, parent, cursor);
                assert!(page.scanned_slots <= 4096);
                count += page.items.len();
                calls += 1;
                assert!(calls < 100);
                cursor = page.next;
                assert_eq!(page.complete, cursor.is_none());
            }
            assert_eq!(count, 5001);
        },
    );
}
#[test]
fn evaluation_invalidates_existing_cursor_even_when_it_mutates_then_throws() {
    run(
        "let object={};for(let i=0;i<1000;i++)object['p'+i]=i;debugger;",
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let cursor = inspect(d, parent, None).next.unwrap();
            eval(d, frame, "object.extra=42;throw 123");
            let id = send(
                d,
                DebugRequest::Inspect(InspectionRequest {
                    parent,
                    cursor: Some(cursor),
                    page: Page {
                        start: 0,
                        count: 64,
                    },
                }),
            );
            assert_eq!(reply(d, id).unwrap_err(), DebugError::StaleInspection);
            assert_eq!(inspect(d, parent, None).items.len(), 64);
        },
    );
}
#[test]
fn inspection_never_invokes_getters_or_proxy_traps_and_bounds_large_text() {
    run(
        "let calls=0;let object={get bad(){calls++;throw 1},huge:'x'.repeat(1000000)};let proxy=new Proxy({}, {ownKeys(){calls++;throw 2}});debugger;",
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let page = inspect(d, parent, None);
            let accessor = page
                .items
                .iter()
                .find(|v| v.variable.name.to_utf8().unwrap() == "bad")
                .unwrap();
            assert!(accessor.variable.value.accessor);
            let huge = page
                .items
                .iter()
                .find(|v| v.variable.name.to_utf8().unwrap() == "huge")
                .unwrap();
            assert_eq!(huge.variable.value.preview.as_units().len(), 259);
            assert!(eval(d, frame, "proxy").object.is_none());
            assert_eq!(eval(d, frame, "calls").preview.to_utf8().unwrap(), "0");
        },
    );
}
#[test]
fn frame_scope_pages_follow_lexical_depths_and_closures() {
    run(
        "function outer(captured){return function inner(argument){var local=3;let shadow=4;{let shadow=5;debugger;return shadow+captured+argument+local;}}}outer(7)(11);",
        |d, _, frame| {
            let scopes = match ask(d, DebugRequest::Scopes { frame }) {
                DebugReply::Scopes(scopes) => scopes,
                x => panic!("{x:?}"),
            };
            let mut values = Vec::new();
            for scope in scopes {
                let parent = VariableParent::Scope(scope.id);
                let mut cursor = None;
                loop {
                    let page = inspect(d, parent, cursor);
                    assert!(page.scanned_slots <= 4096);
                    values.extend(page.items.into_iter().map(|v| {
                        (
                            v.variable.name.to_utf8().unwrap(),
                            v.variable.value.preview.to_utf8().unwrap(),
                        )
                    }));
                    cursor = page.next;
                    if page.complete {
                        break;
                    }
                }
            }
            for pair in [
                ("captured", "7"),
                ("argument", "11"),
                ("local", "3"),
                ("shadow", "4"),
                ("shadow", "5"),
            ] {
                assert!(
                    values.contains(&(pair.0.into(), pair.1.into())),
                    "missing {pair:?} in {values:?}"
                );
            }
        },
    );
}
#[test]
fn dense_arrays_page_in_native_index_order() {
    run(
        "let array=[];for(let i=0;i<200;i++)array.push(i);debugger;",
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "array").object.unwrap());
            let first = inspect(d, parent, None);
            assert_eq!(first.items[0].variable.name.to_utf8().unwrap(), "0");
            assert_eq!(
                first.items[63].variable.value.preview.to_utf8().unwrap(),
                "63"
            );
            let second = inspect(d, parent, first.next);
            assert_eq!(second.items[0].variable.name.to_utf8().unwrap(), "64");
        },
    );
}
#[test]
fn configured_scan_item_and_reply_budgets_are_honored() {
    let mut config = VmConfig::default();
    config.debug_limits.inspection_page_items = 7;
    config.debug_limits.inspection_scan_slots = 3;
    config.debug_limits.reply_bytes = 8192;
    run_config(
        "let object={};for(let i=0;i<100;i++)object['p'+i]='x'.repeat(100000);debugger;",
        config,
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let first = inspect(d, parent, None);
            assert_eq!(first.items.len(), 3);
            assert_eq!(first.scanned_slots, 3);
            assert!(!first.complete);
            for _ in 0..40 {
                // Continuing advances the same cursor identity and record, without
                // one retained root/reference allocation for each emitted page.
                let mut page = inspect(d, parent, None);
                while let Some(cursor) = page.next {
                    page = inspect(d, parent, Some(cursor));
                }
                assert!(page.complete);
            }
        },
    );
}
#[test]
fn lazy_native_properties_are_reported_without_initialization() {
    run("debugger;", |d, _, frame| {
        let parent = VariableParent::Object(eval(d, frame, "Math").object.unwrap());
        let first = inspect(d, parent, None);
        let lazy = first
            .items
            .iter()
            .filter(|v| {
                v.variable.value.unavailable.as_deref() == Some("lazy property not initialized")
            })
            .count();
        assert!(lazy > 0, "Math must exercise native AUTOINIT properties");
        let again = inspect(d, parent, None);
        assert_eq!(
            again
                .items
                .iter()
                .filter(|v| v.variable.value.unavailable.as_deref()
                    == Some("lazy property not initialized"))
                .count(),
            lazy
        );
    });
}
#[test]
fn long_shared_prefix_property_names_remain_distinct_and_lossless() {
    run(
        "let object={};object['x'.repeat(300)+'a']=1;object['x'.repeat(300)+'b']=2;debugger;",
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let page = inspect(d, parent, None);
            assert_eq!(page.items.len(), 2);
            assert_eq!(page.items[0].variable.name.as_units().len(), 301);
            assert_ne!(page.items[0].variable.name, page.items[1].variable.name);
        },
    );
}
#[test]
fn completed_cursor_slots_are_reclaimed_without_reviving_old_tokens() {
    let mut config = VmConfig::default();
    config.debug_limits.inspection_objects = 8;
    run_config(
        "let object={};for(let i=0;i<130;i++)object['p'+i]=i;debugger;",
        config,
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let old = inspect(d, parent, None).next.unwrap();
            let mut page = inspect(d, parent, Some(old));
            while let Some(cursor) = page.next {
                page = inspect(d, parent, Some(cursor));
            }
            for _ in 0..40 {
                let mut page = inspect(d, parent, None);
                while let Some(cursor) = page.next {
                    page = inspect(d, parent, Some(cursor));
                }
            }
            let id = send(
                d,
                DebugRequest::Inspect(InspectionRequest {
                    parent,
                    cursor: Some(old),
                    page: Page {
                        start: 0,
                        count: 64,
                    },
                }),
            );
            assert_eq!(reply(d, id).unwrap_err(), DebugError::StaleInspection);
        },
    );
}
#[test]
fn reply_byte_boundary_returns_partial_page_and_retries_unemitted_entry() {
    let mut config = VmConfig::default();
    config.debug_limits.inspection_page_items = 2;
    config.debug_limits.reply_bytes = 8192;
    run_config(
        "let object={};object['x'.repeat(2000)+'a']=1;object['x'.repeat(2000)+'b']=2;debugger;",
        config,
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let first = inspect(d, parent, None);
            assert_eq!(first.items.len(), 1);
            assert!(!first.complete);
            let second = inspect(d, parent, first.next);
            assert_eq!(second.items.len(), 1);
            assert!(second.complete);
            assert_ne!(first.items[0].variable.name, second.items[0].variable.name);
        },
    );
}
#[test]
fn frame_scope_continuations_bound_actual_variable_slots() {
    let mut script = String::from("function many(){");
    for i in 0..300 {
        script.push_str(&format!("let v{i}={i};"));
    }
    script.push_str("debugger;return v299;}many();");
    let mut config = VmConfig::default();
    config.debug_limits.inspection_page_items = 3;
    config.debug_limits.inspection_scan_slots = 7;
    run_config(&script, config, |d, _, frame| {
        let DebugReply::Scopes(scopes) = ask(d, DebugRequest::Scopes { frame }) else {
            panic!()
        };
        let parent = VariableParent::Scope(
            scopes
                .into_iter()
                .find(|s| s.name.to_utf8().unwrap() == "Local")
                .unwrap()
                .id,
        );
        let mut cursor = None;
        let mut names = std::collections::BTreeSet::new();
        let mut ordered = Vec::new();
        loop {
            let page = inspect(d, parent, cursor);
            assert!(page.items.len() <= 3);
            assert!(page.scanned_slots <= 7);
            for entry in page.items {
                let name = entry.variable.name.to_utf8().unwrap();
                names.insert(name.clone());
                ordered.push(name);
            }
            cursor = page.next;
            if page.complete {
                break;
            }
        }
        let expected: std::collections::BTreeSet<_> = (0..300).map(|i| format!("v{i}")).collect();
        let actual = names
            .iter()
            .filter(|name| name.starts_with('v'))
            .cloned()
            .collect();
        assert_eq!(expected, actual);
        assert!(names.contains("v0") && names.contains("v299"));
        let mut page = inspect_at(d, parent, 150, None);
        assert!(page.items.is_empty() && !page.complete);
        let mut tail = Vec::new();
        loop {
            assert!(page.scanned_slots <= 7 && page.items.len() <= 3);
            tail.extend(
                page.items
                    .into_iter()
                    .map(|v| v.variable.name.to_utf8().unwrap()),
            );
            if page.complete {
                break;
            }
            page = inspect(d, parent, page.next);
        }
        assert_eq!(tail, ordered[150..]);
    });
}
#[test]
fn over_capacity_names_fail_explicitly_without_a_truncated_success() {
    run(
        "let object={};object['x'.repeat(70000)]=1;debugger;",
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let id = send(
                d,
                DebugRequest::Inspect(InspectionRequest {
                    parent,
                    cursor: None,
                    page: Page {
                        start: 0,
                        count: 64,
                    },
                }),
            );
            assert_eq!(reply(d, id).unwrap_err(), DebugError::Capacity);
            assert_eq!(eval(d, frame, "1+1").preview.to_utf8().unwrap(), "2");
        },
    );
}
#[test]
fn small_reply_with_default_item_limit_admits_a_bounded_partial_page() {
    let mut config = VmConfig::default();
    config.debug_limits.reply_bytes = 8192;
    run_config(
        "let object={};for(let i=0;i<100;i++)object['p'+i]='x'.repeat(1000);debugger;",
        config,
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let first = inspect(d, parent, None);
            assert!(!first.items.is_empty());
            assert!(first.items.len() < 64);
            assert!(!first.complete);
            let second = inspect(d, parent, first.next);
            assert_eq!(
                second.items[0].variable.name.to_utf8().unwrap(),
                format!("p{}", first.items.len())
            );
        },
    );
}
#[test]
fn initial_offset_advances_under_scan_budget_without_copying_skipped_entries() {
    let mut config = VmConfig::default();
    config.debug_limits.inspection_scan_slots = 7;
    config.debug_limits.inspection_page_items = 3;
    run_config(
        "let object={};for(let i=0;i<200;i++)object['p'+i]=i;debugger;",
        config,
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let mut page = inspect_at(d, parent, 150, None);
            assert_eq!(page.scanned_slots, 7);
            assert!(page.items.is_empty() && !page.complete && page.next.is_some());
            let mut values = Vec::new();
            let mut pages = 0;
            loop {
                assert!(page.scanned_slots <= 7 && page.items.len() <= 3);
                assert_eq!(page.total, None);
                values.extend(
                    page.items
                        .into_iter()
                        .map(|v| v.variable.value.preview.to_utf8().unwrap()),
                );
                pages += 1;
                assert!(pages < 100);
                if page.complete {
                    break;
                }
                page = inspect_at(d, parent, 0, page.next);
            }
            assert_eq!(
                values,
                (150..200).map(|i| i.to_string()).collect::<Vec<_>>()
            );
        },
    );
}
#[test]
fn initial_offset_beyond_end_completes_truthfully_and_continuation_requires_zero_start() {
    let mut config = VmConfig::default();
    config.debug_limits.inspection_scan_slots = 7;
    run_config(
        "let object={};for(let i=0;i<20;i++)object['p'+i]=i;debugger;",
        config,
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let mut page = inspect_at(d, parent, usize::MAX, None);
            let cursor = page.next.unwrap();
            let id = send(
                d,
                DebugRequest::Inspect(InspectionRequest {
                    parent,
                    cursor: Some(cursor),
                    page: Page {
                        start: 1,
                        count: 64,
                    },
                }),
            );
            assert_eq!(reply(d, id).unwrap_err(), DebugError::InvalidReference);
            let mut pages = 0;
            loop {
                assert!(page.items.is_empty() && page.scanned_slots <= 7);
                assert_eq!(page.total, None);
                if page.complete {
                    assert!(page.next.is_none());
                    break;
                }
                page = inspect(d, parent, page.next);
                pages += 1;
                assert!(pages < 10);
            }
        },
    );
}
#[test]
fn zero_native_inspection_limits_are_rejected_at_vm_configuration() {
    for scan in [true, false] {
        let mut config = VmConfig::default();
        if scan {
            config.debug_limits.inspection_scan_slots = 0;
        } else {
            config.debug_limits.inspection_page_items = 0;
        }
        assert!(matches!(
            VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config),
            Err(VmError::Configuration(_))
        ));
    }
}
#[test]
fn rope_string_previews_copy_only_the_bounded_prefix() {
    run(
        "let text='';for(let i=0;i<100;i++)text=text+('a'.repeat(300)+'Ω'.repeat(300));let object={text};debugger;",
        |d, _, frame| {
            let value = eval(d, frame, "text");
            let units = value.preview.as_units();
            assert_eq!(units.len(), 259);
            assert!(units[1..257].iter().all(|u| *u == u16::from(b'a')));
            assert_eq!(units[257], 0x2026);
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let page = inspect(d, parent, None);
            assert_eq!(page.items[0].variable.value.preview, value.preview);
        },
    );
}
#[test]
fn rich_previews_ignore_hostile_prototypes() {
    run(
        "let calls=0;let map=new Map([[1,2]]);Map.prototype.toString=function(){calls++;throw 1};Object.defineProperty(Map.prototype,'size',{get(){calls++;throw 1}});Object.defineProperty(Symbol.prototype,'description',{get(){calls++;throw 1}});Promise.prototype.toString=function(){calls++;throw 1};let object={map,set:new Set([3]),big:12345678901234567890n,symbol:Symbol('hello'),promise:Promise.resolve(42)};debugger;",
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let page = inspect(d, parent, None);
            for (name, kind) in [
                ("map", "Map"),
                ("set", "Set"),
                ("big", "bigint"),
                ("symbol", "symbol"),
                ("promise", "Promise"),
            ] {
                let entry = page
                    .items
                    .iter()
                    .find(|v| v.variable.name.to_utf8().unwrap() == name)
                    .unwrap();
                assert_eq!(entry.type_name.to_utf8().unwrap(), kind);
                assert!(entry.variable.value.preview.to_utf8().unwrap().len() > kind.len());
            }
            assert_eq!(eval(d, frame, "calls").preview.to_utf8().unwrap(), "0");
        },
    );
}
#[test]
fn map_set_pages_preserve_identity_order_and_bounds() {
    run(
        "let key={marker:1};let map=new Map();let set=new Set();for(let i=0;i<150;i++){map.set(i===0?key:i,i===0?key:i+1000);set.add(i)}debugger;",
        |d, _, frame| {
            let map = eval(d, frame, "map");
            assert_eq!(map.preview.to_utf8().unwrap(), "Map(150)");
            let parent = VariableParent::Object(map.object.unwrap());
            let mut cursor = None;
            let mut entries = Vec::new();
            loop {
                let page = inspect(d, parent, cursor);
                assert!(page.items.len() <= 64 && page.scanned_slots <= 4096);
                entries.extend(page.items);
                cursor = page.next;
                if page.complete {
                    break;
                }
            }
            assert_eq!(entries.len(), 300);
            assert_eq!(entries[0].variable.name.to_utf8().unwrap(), "[0].key");
            assert_eq!(entries[1].variable.name.to_utf8().unwrap(), "[0].value");
            assert_eq!(
                entries[0].variable.value.object,
                entries[1].variable.value.object
            );
            assert_eq!(
                entries[299].variable.value.preview.to_utf8().unwrap(),
                "1149"
            );
            let parent = VariableParent::Object(eval(d, frame, "set").object.unwrap());
            let first = inspect(d, parent, None);
            assert_eq!(first.items.len(), 64);
            let second = inspect(d, parent, first.next);
            assert_eq!(
                second.items[0].variable.value.preview.to_utf8().unwrap(),
                "64"
            );
            let old = second.next.unwrap();
            eval(d, frame, "map.clear();set.clear()");
            let id = send(
                d,
                DebugRequest::Inspect(InspectionRequest {
                    parent,
                    cursor: Some(old),
                    page: Page {
                        start: 0,
                        count: 64,
                    },
                }),
            );
            assert_eq!(reply(d, id).unwrap_err(), DebugError::StaleInspection);
        },
    );
}
#[test]
fn typed_array_detach_and_sparse_array_holes_are_explicit() {
    run(
        "let buffer=new ArrayBuffer(8);let view=new Uint8Array(buffer);view[0]=42;let moved=buffer.transfer();let sparse=[];sparse[99999]=7;let shared=new Uint8Array(new SharedArrayBuffer(8));debugger;",
        |d, _, frame| {
            let value = eval(d, frame, "view");
            assert!(value.preview.to_utf8().unwrap().contains("detached"));
            assert!(inspect(d, VariableParent::Object(value.object.unwrap()), None).complete);
            let value = eval(d, frame, "moved");
            let page = inspect(d, VariableParent::Object(value.object.unwrap()), None);
            assert_eq!(
                page.items[0].variable.value.preview.to_utf8().unwrap(),
                "42"
            );
            let value = eval(d, frame, "sparse");
            assert!(value.preview.to_utf8().unwrap().contains("holes omitted"));
            let page = inspect(d, VariableParent::Object(value.object.unwrap()), None);
            assert!(
                page.items
                    .iter()
                    .any(|e| e.variable.name.to_utf8().unwrap() == "99999")
            );
            let value = eval(d, frame, "shared");
            assert!(value.preview.to_utf8().unwrap().contains("contents opaque"));
            let page = inspect(d, VariableParent::Object(value.object.unwrap()), None);
            assert!(page.items.is_empty() && page.complete);
        },
    );
}
#[test]
fn promise_state_inspection_does_not_run_jobs() {
    run(
        "let jobs=0;let fulfilled=Promise.resolve({answer:42});let pending=new Promise(()=>{});fulfilled.then(()=>jobs++);debugger;",
        |d, _, frame| {
            for (name, state) in [("fulfilled", "1"), ("pending", "0")] {
                let value = eval(d, frame, name);
                assert!(
                    value
                        .preview
                        .to_utf8()
                        .unwrap()
                        .contains(if name == "pending" {
                            "pending"
                        } else {
                            "fulfilled"
                        })
                );
                let page = inspect(d, VariableParent::Object(value.object.unwrap()), None);
                assert_eq!(
                    page.items[0].variable.value.preview.to_utf8().unwrap(),
                    state
                );
                if name == "fulfilled" {
                    assert!(page.items[1].variable.value.object.is_some());
                }
            }
            assert_eq!(eval(d, frame, "jobs").preview.to_utf8().unwrap(), "0");
        },
    );
}
#[test]
fn bigint_symbol_error_function_previews_mark_truncation() {
    run(
        "let huge=1n<<1000000n;let long=Symbol('x'.repeat(100000));let absent=Symbol();let empty=Symbol('');function useful(a,b){}let error=new Error('message');let calls=0;Object.defineProperty(error,'stack',{get(){calls++;throw 1}});Error.prepareStackTrace=()=>{calls++;throw 2};debugger;",
        |d, _, frame| {
            let value = eval(d, frame, "huge");
            let text = value.preview.to_utf8().unwrap();
            assert!(text.contains("truncated") && text.contains("limbs"));
            assert!(text.len() < 300);
            let value = eval(d, frame, "long");
            assert!(
                value.preview.as_units().len() <= 512 && value.preview.as_units().contains(&0x2026)
            );
            assert_ne!(
                eval(d, frame, "absent").preview,
                eval(d, frame, "empty").preview
            );
            let value = eval(d, frame, "useful");
            let text = value.preview.to_utf8().unwrap();
            assert!(
                text.contains("useful")
                    && text.contains("arity=2")
                    && text.contains("inspection.js"),
                "{text}"
            );
            let value = eval(d, frame, "error");
            assert!(value.preview.to_utf8().unwrap().contains("native trace"));
            let page = inspect(d, VariableParent::Object(value.object.unwrap()), None);
            assert!(
                page.items
                    .iter()
                    .any(|e| e.variable.name.to_utf8().unwrap() == "message")
            );
            assert_eq!(eval(d, frame, "calls").preview.to_utf8().unwrap(), "0");
        },
    );
}

#[test]
fn dap_rich_pages_checkpoint_and_reject_unbounded_offsets() {
    use serde_json::{Value, json};
    use std::time::{Duration, Instant};
    fn decode(frame: &[u8]) -> Value {
        let start = frame.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
        serde_json::from_slice(&frame[start..]).unwrap()
    }
    fn send_dap(dap: &mut DapAdapter, seq: u64, command: &str, args: Value) -> Vec<Value> {
        let body = serde_json::to_vec(
            &json!({"seq":seq,"type":"request","command":command,"arguments":args}),
        )
        .unwrap();
        let mut wire = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        wire.extend(body);
        dap.receive(&wire)
            .unwrap()
            .iter()
            .map(|f| decode(f))
            .collect()
    }
    fn response(dap: &mut DapAdapter, seq: u64, mut frames: Vec<Value>) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(index) = frames.iter().position(|f| f["request_seq"] == seq) {
                return frames.remove(index);
            }
            assert!(Instant::now() < deadline, "DAP response {seq} missing");
            frames.extend(dap.poll().unwrap().iter().map(|f| decode(f)));
            std::thread::yield_now();
        }
    }
    let mut config = VmConfig::default();
    config.debug_limits.inspection_objects = 8;
    config.debug_limits.requests = 1;
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let debug = vm
        .attach_debugger(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let mut dap = DapAdapter::new(debug).unwrap();
    let (ready, configured) = std::sync::mpsc::sync_channel(1);
    let worker = controller(&vm, move || {
        send_dap(&mut dap, 1, "initialize", json!({}));
        let mut frames = send_dap(&mut dap, 2, "configurationDone", json!({}));
        ready.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !frames.iter().any(|f| f["event"] == "stopped") {
            assert!(Instant::now() < deadline);
            frames.extend(dap.poll().unwrap().iter().map(|f| decode(f)));
            std::thread::yield_now();
        }
        let frames = send_dap(&mut dap, 3, "stackTrace", json!({"threadId":1}));
        let stack = response(&mut dap, 3, frames);
        let frame = stack["body"]["stackFrames"][0]["id"].clone();
        let frames = send_dap(
            &mut dap,
            4,
            "evaluate",
            json!({"frameId":frame,"expression":"map","context":"repl"}),
        );
        let evaluated = response(&mut dap, 4, frames);
        assert_eq!(evaluated["success"], true);
        let reference = evaluated["body"]["variablesReference"].clone();
        for (seq, start, first) in [(5, 0, "[0].key"), (6, 64, "[32].key")] {
            let frames = send_dap(
                &mut dap,
                seq,
                "variables",
                json!({"variablesReference":reference,"start":start,"count":64}),
            );
            let page = response(&mut dap, seq, frames);
            assert_eq!(page["success"], true, "{page}");
            assert_eq!(page["body"]["variables"].as_array().unwrap().len(), 64);
            assert_eq!(page["body"]["variables"][0]["name"], first);
            assert_eq!(page["body"]["variables"][0]["type"], "number");
        }
        // Omitted/zero count requests all variables: never silently report an
        // incomplete native page as a successful complete unpaged response.
        for seq in [7, 8] {
            let mut arguments = json!({"variablesReference":reference});
            if seq == 8 {
                arguments["count"] = json!(0);
            }
            let frames = send_dap(&mut dap, seq, "variables", arguments);
            let page = response(&mut dap, seq, frames);
            assert_eq!(page["success"], false, "{page}");
            assert!(page["message"].as_str().unwrap().contains("capacity"));
        }
        let frames = send_dap(
            &mut dap,
            9,
            "variables",
            json!({"variablesReference":reference,"start":100000,"count":64}),
        );
        let failure = response(&mut dap, 9, frames);
        assert_eq!(failure["success"], false);
        assert!(
            failure["message"].as_str().unwrap().contains("capacity"),
            "{failure}"
        );
        // Exceed the configured cursor budget while repeatedly abandoning pages.
        // Cursor release must work with exactly one outstanding request permit.
        for seq in 10..42 {
            let start = if seq % 3 == 0 { 100000 } else { 0 };
            let frames = send_dap(
                &mut dap,
                seq,
                "variables",
                json!({"variablesReference":reference,"start":start,"count":64}),
            );
            let page = response(&mut dap, seq, frames);
            assert_eq!(
                page["success"],
                start == 0,
                "cursor leaked at {seq}: {page}"
            );
        }
        let frames = send_dap(
            &mut dap,
            43,
            "variables",
            json!({"variablesReference":reference,"start":64,"count":64}),
        );
        let page = response(&mut dap, 43, frames);
        assert_eq!(page["success"], true, "{page}");
        send_dap(&mut dap, 44, "continue", json!({"threadId":1}));
    });
    configured.recv_timeout(Duration::from_secs(10)).unwrap();
    vm.evaluate(ScriptSource::new(
        JsString::from_utf8("let map=new Map();for(let i=0;i<10000;i++)map.set(i,i+100);debugger;"),
        "file:///dap-rich.js".into(),
    ))
    .unwrap();
    worker.join().unwrap();
}
#[test]
fn rich_symbol_keys_opaque_values_and_resized_views_are_truthful() {
    run(
        "let calls=0;Object.defineProperty(Symbol.prototype,'description',{get(){calls++;throw 1}});let symbols={[Symbol()]:1,[Symbol('')]:2,[Symbol.for('global')]:3,global:4};let object={weak:new WeakMap(),proxy:new Proxy({}, {get(){calls++;throw 1},ownKeys(){calls++;throw 1}})};let buffer=new ArrayBuffer(16,{maxByteLength:32});let fixed=new Uint16Array(buffer,8,4);buffer.resize(4);debugger;",
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "symbols").object.unwrap());
            let page = inspect(d, parent, None);
            let names: Vec<_> = page
                .items
                .iter()
                .map(|e| e.variable.name.to_utf8().unwrap())
                .collect();
            assert!(
                names
                    .iter()
                    .any(|name| name.starts_with("[Symbol(<absent>)] #"))
            );
            assert!(
                names
                    .iter()
                    .any(|name| name.starts_with("[Symbol(\"\")] #"))
            );
            assert!(
                names
                    .iter()
                    .any(|name| name.starts_with("[Symbol(\"global\")] #"))
            );
            assert!(names.contains(&"global".into()));
            assert_eq!(names.len(), 4);
            let parent = VariableParent::Object(eval(d, frame, "object").object.unwrap());
            let page = inspect(d, parent, None);
            for entry in page.items {
                assert!(entry.variable.value.object.is_none());
                assert!(
                    entry
                        .variable
                        .value
                        .unavailable
                        .as_deref()
                        .unwrap()
                        .contains("opaque")
                );
            }
            let value = eval(d, frame, "fixed");
            let text = value.preview.to_utf8().unwrap();
            assert!(
                text.contains("out of bounds") && text.contains("byteLength=0"),
                "{text}"
            );
            let page = inspect(d, VariableParent::Object(value.object.unwrap()), None);
            assert!(page.complete && page.items.is_empty());
            assert_eq!(eval(d, frame, "calls").preview.to_utf8().unwrap(), "0");
        },
    );
}

#[test]
fn odd_length_resizable_views_and_hostile_native_names_remain_bounded() {
    run(
        "let buffer=new ArrayBuffer(8,{maxByteLength:16});let typed=new Uint16Array(buffer);let view=new DataView(buffer);buffer.resize(5);let calls=0;Object.defineProperty(Math.max,'name',{value:'x'.repeat(1000)});Object.defineProperty(Math.max,'length',{get(){calls++;throw 1}});debugger;",
        |d, _, frame| {
            let value = eval(d, frame, "typed");
            assert!(value.preview.to_utf8().unwrap().contains("byteLength=4"));
            let page = inspect(d, VariableParent::Object(value.object.unwrap()), None);
            assert_eq!(page.items.len(), 4);
            let value = eval(d, frame, "view");
            assert!(value.preview.to_utf8().unwrap().contains("byteLength=5"));
            let page = inspect(d, VariableParent::Object(value.object.unwrap()), None);
            assert_eq!(page.items.len(), 5);
            let value = eval(d, frame, "Math.max");
            let text = value.preview.to_utf8().unwrap();
            assert!(
                value.preview.as_units().contains(&0x2026) && text.contains("native arity=2"),
                "{text}"
            );
            assert_eq!(eval(d, frame, "calls").preview.to_utf8().unwrap(), "0");
        },
    );
}
#[test]
fn explicitly_released_cursors_reclaim_slots_and_reject_reuse() {
    let mut config = VmConfig::default();
    config.debug_limits.inspection_objects = 4;
    run_config(
        "let map=new Map();for(let i=0;i<100;i++)map.set(i,i);debugger;",
        config,
        |d, _, frame| {
            let parent = VariableParent::Object(eval(d, frame, "map").object.unwrap());
            for _ in 0..20 {
                let cursor = inspect(d, parent, None).next.unwrap();
                assert!(matches!(
                    ask(d, DebugRequest::ReleaseCursor(cursor)),
                    DebugReply::CursorReleased
                ));
                let id = send(d, DebugRequest::ReleaseCursor(cursor));
                assert_eq!(reply(d, id).unwrap_err(), DebugError::StaleInspection);
            }
            assert_eq!(inspect(d, parent, None).items.len(), 64);
        },
    );
}
