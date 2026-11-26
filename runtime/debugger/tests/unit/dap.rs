//! Private framing/identity/bounds tests use the shared metadata backend.
use super::*;
#[path = "../support/dap_fixture.rs"]
mod fixture;
use crate::engine::*;
use fixture::*;
use std::{num::NonZeroU64, rc::Rc};

#[test]
fn an_infallible_json_sink_has_no_dap_failure_channel() {
    let value = obj(vec![
        ("ok", boolean(true)),
        ("items", array(vec![number(7), text("a\n")])),
    ]);
    let mut bytes = Vec::new();
    let result: Result<(), std::convert::Infallible> = write_value(&value, &mut |chunk| {
        bytes.extend_from_slice(chunk);
        Ok::<(), std::convert::Infallible>(())
    });
    assert_eq!(result, Ok(()));
    assert_eq!(bytes, br#"{"ok":true,"items":[7,"a\n"]}"#);
}

#[test]
fn framing_bounds_ascii_validation_and_allocation_errors_are_closed_and_typed() {
    assert!(matches!(DapFramer::new(0, 1), Err(DapError::Capacity)));
    assert!(matches!(DapFramer::new(1, 0), Err(DapError::Capacity)));
    for header in [
        b"Content-Length: 999999999999999999999999999999\r\n\r\n".as_slice(),
        b"Content-Length: 1\r\n\xff: bad\r\n\r\n",
        b"InvalidField\r\n\r\n",
    ] {
        let mut f = DapFramer::new(1024, 1024).unwrap();
        assert!(f.push(header).is_err());
        assert!(matches!(f.push(b"{}"), Err(DapError::Protocol(_))));
    }
    let mut f = DapFramer::new(1, 1).unwrap();
    assert!(matches!(f.push(&vec![b'x'; 8193]), Err(DapError::Capacity)));
    let mut f = DapFramer::new(1, 1).unwrap();
    assert_eq!(
        f.push(b"Content-Length: 0\r\n\r\n").unwrap(),
        vec![Vec::<u8>::new()]
    );
    let mut f = DapFramer::new(4096, 4096).unwrap();
    f.buffer = Vec::with_capacity(64);
    let (result, fired) =
        crate::vm::test_allocator::fail_nth(0, || f.push(b"Content-Length: 4096\r\n\r\n"));
    assert!(fired);
    assert!(matches!(result, Err(DapError::Capacity)));
    let f = DapFramer::new(8, 8).unwrap();
    let (result, fired) = crate::vm::test_allocator::fail_nth(1, || f.encode(b"{}"));
    assert!(fired);
    assert!(matches!(result, Err(DapError::Capacity)));
}
#[test]
fn manual_json_preserves_every_escape_and_surrogate_pair_and_output_bound() {
    let units = vec![
        0x22, 0x5c, 8, 12, 10, 13, 9, 0, 1, 31, 0xd83d, 0xde80, 0xd800, 0x61, 0xdc00,
    ];
    let value = Value::Text(JsString::from_units(units.clone()));
    let bytes = frame_value(&value, 256).unwrap();
    let body = bytes.as_slice();
    let offset = body.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let raw = std::str::from_utf8(&body[offset..]).unwrap();
    let decoded: super::super::source_map::LosslessString = serde_json::from_str(raw).unwrap();
    assert_eq!(decoded.0.as_units(), units);
    assert!(matches!(frame_value(&value, 1), Err(DapError::Capacity)));
    let owned = text("abc");
    let (result, fired) = crate::vm::test_allocator::fail_nth(1, || frame_value(&owned, 256));
    assert!(fired);
    assert!(matches!(result, Err(DapError::Capacity)));
}
#[test]
fn coordinate_uri_and_diagnostic_helpers_keep_typed_failures() {
    assert!(from_client(0, true).is_err());
    assert_eq!(from_client(0, false).unwrap(), 0);
    assert_eq!(from_client(2, true).unwrap(), 1);
    assert!(to_client(u32::MAX, true).is_err());
    assert_eq!(to_client(0, false).unwrap(), 0);
    #[cfg(not(windows))]
    assert_eq!(
        canonical_uri("/tmp/a b.js").unwrap(),
        "file:///tmp/a%20b.js"
    );
    #[cfg(windows)]
    assert_eq!(
        canonical_uri(r"C:\tmp\a b.js").unwrap(),
        "file:///C:/tmp/a%20b.js"
    );
    assert!(canonical_uri("relative.js").is_err());
    for error in [
        DapError::Protocol("bad header".into()),
        DapError::Capacity,
        DapError::Debug(DebugError::Closed),
    ] {
        assert!(!error.to_string().is_empty());
    }
    assert!(check_thread(2).is_err());
    assert!(matches!(
        check_control(DebugControlReceipt::AcceptedWakeFailed {
            error: DebugError::Closed
        }),
        Err(DapError::Debug(DebugError::Closed))
    ));
}
#[test]
fn reference_and_output_exhaustion_never_wrap_or_recycle_old_numbers() {
    let (_store, backend, _coordinator, mut dap) = fixture();
    let pause = backend.pause(PauseReason::Requested);
    dap.synchronize_pause();
    let port = backend.port();
    let ids = port.identities();
    let frame = match ids
        .register(pause, ReferenceKind::Frame, NonZeroU64::new(1).unwrap())
        .unwrap()
    {
        DebugReference::Frame(f) => f,
        _ => unreachable!(),
    };
    let scope = match ids
        .register(pause, ReferenceKind::Scope, NonZeroU64::new(2).unwrap())
        .unwrap()
    {
        DebugReference::Scope(s) => s,
        _ => unreachable!(),
    };
    let object = match ids
        .register(pause, ReferenceKind::Object, NonZeroU64::new(3).unwrap())
        .unwrap()
    {
        DebugReference::Object(o) => o,
        _ => unreachable!(),
    };
    let f = dap.frame_reference(frame).unwrap();
    assert_eq!(dap.frame_reference(frame).unwrap(), f);
    let s = dap.parent_reference(VariableParent::Scope(scope)).unwrap();
    assert_eq!(
        dap.parent_reference(VariableParent::Scope(scope)).unwrap(),
        s
    );
    let o = dap
        .parent_reference(VariableParent::Object(object))
        .unwrap();
    assert_eq!(
        dap.parent_reference(VariableParent::Object(object))
            .unwrap(),
        o
    );
    assert!(!same_parent(
        VariableParent::Object(object),
        VariableParent::Scope(scope)
    ));
    assert!(dap.frame(u64::MAX).is_err());
    dap.limits.inspection_objects = dap.frames.len() + dap.parents.len();
    let another = match ids
        .register(pause, ReferenceKind::Frame, NonZeroU64::new(4).unwrap())
        .unwrap()
    {
        DebugReference::Frame(f) => f,
        _ => unreachable!(),
    };
    assert!(matches!(
        dap.frame_reference(another),
        Err(DapError::Capacity)
    ));
    dap.reference_seq = i32::MAX as u64;
    assert!(matches!(dap.next_reference(), Err(DapError::Capacity)));
    dap.output_seq = u64::MAX;
    assert!(matches!(dap.next_output(), Err(DapError::Capacity)));
    dap.limits.reply_bytes = 1;
    assert!(matches!(dap.page_count(), Err(DapError::Capacity)));
}

#[test]
fn protocol_argument_failures_are_correlated_without_detaching_a_healthy_session() {
    let init = Initialize::default();
    assert!(
        init.lines_start_at1.is_none()
            && init.columns_start_at1.is_none()
            && init.path_format.is_none()
    );
    let (_store, _backend, coordinator, mut dap) = fixture();
    for (seq, command, arguments) in [
        (1, "threads", serde_json::json!({})),
        (
            2,
            "initialize",
            serde_json::json!({"pathFormat":"relative"}),
        ),
        (3, "initialize", serde_json::json!({})),
        (4, "initialize", serde_json::json!({})),
        (5, "pause", serde_json::json!({"threadId":"bad"})),
        (6, "pause", serde_json::json!({"threadId":2})),
        (7, "scopes", serde_json::json!({"frameId":999})),
        (
            8,
            "variables",
            serde_json::json!({"variablesReference":999}),
        ),
        (9, "evaluate", serde_json::json!({"expression":"x"})),
        (10, "source", serde_json::json!({"sourceReference":999})),
        (11, "source", serde_json::json!({})),
        (12, "setBreakpoints", serde_json::json!({"source":{}})),
        (
            13,
            "setBreakpoints",
            serde_json::json!({"source":{"path":"relative.js"}}),
        ),
        (
            14,
            "setBreakpoints",
            serde_json::json!({"source":{"path":"file:///a.js"},"breakpoints":[{"line":0}]}),
        ),
        (15, "continue", serde_json::json!({})),
    ] {
        let output = request(&mut dap, seq, command, arguments);
        assert_eq!(output[0]["request_seq"], seq);
        assert_eq!(
            output[0]["success"],
            seq == 3,
            "{command}/{seq}: {output:?}"
        );
    }
    let output = request(
        &mut dap,
        u64::from(i32::MAX as u32) + 1,
        "threads",
        serde_json::json!({}),
    );
    assert_eq!(output[0]["success"], false);
    assert_ne!(coordinator.state(), DebugExecutionState::Detached);
}

fn pending(kind: PendingKind) -> Pending {
    Pending {
        seq: 10,
        command: "test".into(),
        order: 1,
        setup: false,
        kind,
    }
}
fn original_query() -> SourceQuery {
    SourceQuery::Original(ScriptSelector {
        origin: "file:///a.js".into(),
        expected_digest: None,
    })
}
#[test]
fn invalid_native_source_pages_and_allocation_failures_never_emit_successful_truncation() {
    let (_store, _backend, _coordinator, mut dap) = fixture();
    let frame = FrameRef {
        pause: _backend.pause(PauseReason::Requested),
        slot: 0,
        generation: 1,
    };
    for (total, text, previous) in [
        (1, "xx", None),
        (2, "", None),
        (2, "x", Some(1)),
        (usize::MAX, "x", None),
    ] {
        let p = pending(PendingKind::Source {
            query: original_query(),
            units: Vec::new(),
            total: previous,
        });
        let mut output = Vec::new();
        assert!(
            dap.reply(
                p,
                DebugReply::Source {
                    text: JsString::from_utf8(text),
                    total_units: total
                },
                &mut output
            )
            .is_err()
        );
        assert!(output.is_empty());
    }
    let p = pending(PendingKind::Source {
        query: original_query(),
        units: Vec::new(),
        total: None,
    });
    let reply = DebugReply::Source {
        text: JsString::from_utf8("ok"),
        total_units: 2,
    };
    let mut output = Vec::new();
    let (result, fired) =
        crate::vm::test_allocator::fail_nth(0, || dap.reply(p, reply, &mut output));
    assert!(fired);
    assert!(matches!(result, Err(DapError::Capacity)));
    assert!(output.is_empty());
    assert!(
        dap.reply(
            pending(PendingKind::Sources {
                scripts: Vec::new()
            }),
            DebugReply::Sources {
                scripts: Vec::new(),
                total: 1
            },
            &mut output
        )
        .is_err()
    );
    assert!(output.is_empty());
    dap.reply(
        pending(PendingKind::Evaluation(frame)),
        DebugReply::ExceptionPolicy(ExceptionPolicy::None),
        &mut output,
    )
    .unwrap();
    assert_eq!(decode(&output[0])["success"], false);
}
#[test]
fn source_identity_is_reused_and_original_content_without_digest_is_not_fabricated() {
    let (_store, backend, _coordinator, mut dap) = fixture();
    let port = backend.port();
    let script = port
        .register_script(NativeScriptMetadata {
            key: NativeScriptKey::new(port.identities().context(), NonZeroU64::new(7).unwrap()),
            source: ScriptSource::new(JsString::from_utf8("source"), "file:///a.js".into()),
            kind: ScriptKind::Script,
        })
        .unwrap();
    dap.source_generated(script.id, script.revision, None)
        .unwrap();
    let before = dap.sources.len();
    dap.source_generated(script.id, script.revision, Some("file:///a.js".into()))
        .unwrap();
    assert_eq!(dap.sources.len(), before);
    assert_eq!(dap.sources[0].origin.as_deref(), Some("file:///a.js"));
    let original = OriginalLocation {
        origin: "file:///a.ts".into(),
        line: 0,
        column: 0,
        digest: None,
    };
    dap.source_original(&original).unwrap();
    assert_eq!(dap.sources.len(), before);
    let original = OriginalLocation {
        digest: Some(SourceDigest::of_utf16(&JsString::from_utf8("old"))),
        ..original
    };
    dap.source_original(&original).unwrap();
    dap.source_original(&original).unwrap();
    assert_eq!(dap.sources.len(), before + 1);
    let unknown = WireSource {
        path: None,
        source_reference: Some(u64::MAX),
    };
    assert!(dap.source_origin(&unknown).is_err());
}

// Exercise the same public protocol assertions alongside the private guards in
// this cfg(test) build; the external integration binary remains an API check.
mod conformance {
    use super::fixture::*;
    use coldmind_runtime::{debug::*, engine::*};
    use serde_json::{Value, json};
    use std::{num::NonZeroU64, sync::atomic::Ordering};
    include!("../support/dap_conformance.rs");
}

#[test]
fn invalid_stack_totals_and_empty_pages_produce_one_error_before_reference_creation() {
    let (_store, backend, _coordinator, mut dap) = fixture();
    let pause = backend.pause(PauseReason::Requested);
    dap.synchronize_pause();
    for (start, total) in [(2, 1), (0, 1)] {
        let p = pending(PendingKind::Stack {
            pause,
            start,
            requested: None,
            frames: Vec::new(),
        });
        let mut output = Vec::new();
        assert!(
            dap.reply(
                p,
                DebugReply::Stack {
                    frames: Vec::new(),
                    total
                },
                &mut output
            )
            .is_err()
        );
        assert!(output.is_empty());
        assert!(dap.frames.is_empty());
    }
}
#[test]
fn evaluation_exceptions_and_oversized_previews_cannot_be_reported_as_success() {
    let (_store, _backend, _coordinator, mut dap) = fixture();
    let frame = FrameRef {
        pause: _backend.pause(PauseReason::Requested),
        slot: 0,
        generation: 1,
    };
    let mut output = Vec::new();
    let value = |preview| DebugValue {
        preview,
        object: None,
        accessor: false,
        unavailable: None,
    };
    dap.reply(
        pending(PendingKind::Evaluation(frame)),
        DebugReply::Evaluation {
            value: value(JsString::from_utf8("expression threw")),
            exception: true,
        },
        &mut output,
    )
    .unwrap();
    assert_eq!(decode(&output[0])["success"], false);
    output.clear();
    dap.limits.dap_output_bytes = 512;
    dap.reply(
        pending(PendingKind::Evaluation(frame)),
        DebugReply::Evaluation {
            value: value(JsString::from_units(vec![65; 2000])),
            exception: false,
        },
        &mut output,
    )
    .unwrap();
    assert_eq!(output.len(), 1);
    assert_eq!(decode(&output[0])["success"], false);
    assert!(
        decode(&output[0])["message"]
            .as_str()
            .unwrap()
            .contains("allowance")
    );
    let mut framer = DapFramer::new(1024, 1024).unwrap();
    assert!(framer.push(b"X-Header: 1\r\n\r\n").is_err());
}

#[test]
fn malformed_native_variable_totals_and_breakpoint_ids_are_rejected_before_encoding() {
    let (_store, backend, _coordinator, mut dap) = fixture();
    let pause = backend.pause(PauseReason::Requested);
    dap.synchronize_pause();
    let port = backend.port();
    let scope = match port
        .identities()
        .register(pause, ReferenceKind::Scope, NonZeroU64::new(17).unwrap())
        .unwrap()
    {
        DebugReference::Scope(s) => s,
        _ => unreachable!(),
    };
    for (start, total) in [(2, 1), (0, 1)] {
        let p = pending(PendingKind::Variables {
            parent: VariableParent::Scope(scope),
            start,
            requested: Some(1),
            variables: Vec::new(),
        });
        assert!(
            dap.reply(
                p,
                DebugReply::Variables {
                    variables: Vec::new(),
                    total
                },
                &mut Vec::new()
            )
            .is_err()
        );
    }
    let p = pending(PendingKind::Breakpoints {
        origin: "file:///a.js".into(),
        metadata: Vec::new(),
    });
    let error = dap
        .reply(
            p,
            DebugReply::Breakpoints(vec![BreakpointInfo {
                client_id: 999,
                state: BreakpointState::Pending,
            }]),
            &mut Vec::new(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("breakpoint identity"));
}
#[test]
fn evaluated_object_references_are_reusable_and_custom_source_uris_stay_lossless() {
    let (_store, backend, _coordinator, mut dap) = fixture();
    let pause = backend.pause(PauseReason::Requested);
    let frame = FrameRef {
        pause,
        slot: 0,
        generation: 1,
    };
    dap.synchronize_pause();
    let port = backend.port();
    let object = match port
        .identities()
        .register(pause, ReferenceKind::Object, NonZeroU64::new(27).unwrap())
        .unwrap()
    {
        DebugReference::Object(o) => o,
        _ => unreachable!(),
    };
    let mut refs = Vec::new();
    for _ in 0..2 {
        let mut output = Vec::new();
        dap.reply(
            pending(PendingKind::Evaluation(frame)),
            DebugReply::Evaluation {
                value: DebugValue {
                    preview: JsString::from_utf8("object"),
                    object: Some(object),
                    accessor: false,
                    unavailable: None,
                },
                exception: false,
            },
            &mut output,
        )
        .unwrap();
        refs.push(
            decode(&output[0])["body"]["variablesReference"]
                .as_u64()
                .unwrap(),
        );
    }
    assert!(refs[0] > 0);
    assert_eq!(refs[0], refs[1]);
    let value = dap.path_source("rust://fixture/generated.js", 7);
    let decoded = decode(&frame_value(&value, 1024).unwrap());
    assert_eq!(decoded["path"], "rust://fixture/generated.js");
    let script = port
        .register_script(NativeScriptMetadata {
            key: NativeScriptKey::new(port.identities().context(), NonZeroU64::new(77).unwrap()),
            source: ScriptSource::new(JsString::from_utf8("x"), "file:///known.js".into()),
            kind: ScriptKind::Script,
        })
        .unwrap();
    dap.source_generated(script.id, script.revision, Some("file:///known.js".into()))
        .unwrap();
    let reference = dap.sources.last().unwrap().reference;
    assert_eq!(
        dap.source_origin(&WireSource {
            path: None,
            source_reference: Some(reference)
        })
        .unwrap(),
        "file:///known.js"
    );
}
#[test]
fn breakpoint_and_request_order_exhaustion_fail_before_admitting_native_work() {
    let (_store, backend, _coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", serde_json::json!({}));
    dap.breakpoint_seq = i32::MAX as u64;
    let output = request(
        &mut dap,
        2,
        "setBreakpoints",
        serde_json::json!({"source":{"path":"file:///a.js"},"breakpoints":[{"line":1}]}),
    );
    assert_eq!(output[0]["success"], false);
    assert!(backend.last_breakpoints.borrow().is_empty());
    dap.order = u64::MAX;
    let output = request(&mut dap, 3, "threads", serde_json::json!({}));
    assert_eq!(output[0]["success"], false);
}

#[test]
fn a_pending_configuration_sequence_cannot_be_reused_by_another_command() {
    let (_store, _backend, coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", serde_json::json!({}));
    assert!(
        request(
            &mut dap,
            2,
            "setExceptionBreakpoints",
            serde_json::json!({"filters":[]}),
        )
        .is_empty()
    );
    assert!(request(&mut dap, 3, "configurationDone", serde_json::json!({})).is_empty());

    let duplicate = request(&mut dap, 3, "threads", serde_json::json!({}));
    assert_eq!(duplicate.len(), 1);
    assert_eq!(duplicate[0]["request_seq"], 3);
    assert_eq!(duplicate[0]["success"], false);
    assert!(
        duplicate[0]["message"]
            .as_str()
            .unwrap()
            .contains("duplicate")
    );

    let completed = drive(&coordinator, &mut dap);
    let configuration: Vec<_> = completed
        .iter()
        .filter(|message| message["command"] == "configurationDone")
        .collect();
    assert_eq!(configuration.len(), 1);
    assert_eq!(configuration[0]["request_seq"], 3);
    assert_eq!(configuration[0]["success"], true);
    assert!(dap.configured);
}

#[test]
fn framing_accepts_ascii_extensions_and_rejects_unicode_or_invalid_utf8_headers() {
    let mut framer = DapFramer::new(8, 8).unwrap();
    assert_eq!(
        framer
            .push(b"X-Client: fixture\r\ncOnTeNt-LeNgTh: \t2 \t\r\n\r\n{}")
            .unwrap(),
        vec![b"{}".to_vec()]
    );
    for header in [
        "X-Client: café\r\nContent-Length: 2\r\n\r\n".as_bytes(),
        b"X-Client: \xff\r\nContent-Length: 2\r\n\r\n".as_slice(),
    ] {
        let mut framer = DapFramer::new(8, 8).unwrap();
        let error = framer.push(header).unwrap_err();
        assert!(matches!(error, DapError::Protocol(_)));
        assert!(error.to_string().contains("non-ASCII"));
        assert!(
            framer
                .push(b"{}")
                .unwrap_err()
                .to_string()
                .contains("closed")
        );
    }
}

#[test]
fn malformed_json_and_non_request_messages_detach_before_commands_are_admitted() {
    for body in [
        b"{".as_slice(),
        br#"{"seq":1,"type":"event","command":"initialize"}"#,
        br#"{"seq":1,"type":"request"}"#,
    ] {
        let (_store, backend, coordinator, mut dap) = fixture();
        let framed = DapFramer::new(1024, 1024).unwrap().encode(body).unwrap();
        assert!(matches!(dap.receive(&framed), Err(DapError::Protocol(_))));
        assert!(dap.disconnected);
        assert!(dap.pending.is_empty());
        coordinator.service().unwrap();
        assert_eq!(coordinator.state(), DebugExecutionState::Detached);
        assert!(backend.last_expression.borrow().is_none());
        let after_close = request(&mut dap, 2, "threads", serde_json::json!({}));
        assert!(
            after_close
                .iter()
                .any(|message| { message["request_seq"] == 2 && message["success"] == false })
        );
    }
}

#[test]
fn evaluation_admission_failures_leave_the_selected_frame_usable() {
    let limits = DebugLimits {
        expression_units: 2,
        ..DebugLimits::default()
    };
    let (_store, backend, coordinator, mut dap) = fixture_with_limits(limits);
    let initialized = request(&mut dap, 1, "initialize", serde_json::json!({}));
    assert_eq!(initialized[0]["body"]["supportsEvaluateForHovers"], false);
    let pause = backend.pause(PauseReason::Requested);
    dap.poll().unwrap();
    let frame = match backend
        .port()
        .identities()
        .register(pause, ReferenceKind::Frame, NonZeroU64::new(31).unwrap())
        .unwrap()
    {
        DebugReference::Frame(frame) => frame,
        _ => unreachable!(),
    };
    let frame_id = dap.frame_reference(frame).unwrap();
    for (seq, expression, context) in [(2, "x", "hover"), (3, "long", "repl")] {
        let rejected = request(
            &mut dap,
            seq,
            "evaluate",
            serde_json::json!({"frameId":frame_id,"expression":expression,"context":context}),
        );
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0]["request_seq"], seq);
        assert_eq!(rejected[0]["success"], false);
        assert!(dap.pending.is_empty());
        assert_eq!(dap.frame(frame_id).unwrap(), frame);
        assert_eq!(coordinator.state(), DebugExecutionState::Paused(pause));
    }
    assert!(backend.last_expression.borrow().is_none());
    assert!(
        request(
            &mut dap,
            4,
            "evaluate",
            serde_json::json!({"frameId":frame_id,"expression":"x","context":"repl"}),
        )
        .is_empty()
    );
    let completed = drive(&coordinator, &mut dap);
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0]["request_seq"], 4);
    assert_eq!(completed[0]["success"], true);
    assert_eq!(completed[0]["body"]["result"], "fixture result");
}

#[test]
fn unavailable_variable_metadata_counts_toward_retained_output_allowance() {
    let (_store, backend, _coordinator, mut dap) = fixture();
    let pause = backend.pause(PauseReason::Requested);
    dap.synchronize_pause();
    let scope = match backend
        .port()
        .identities()
        .register(pause, ReferenceKind::Scope, NonZeroU64::new(32).unwrap())
        .unwrap()
    {
        DebugReference::Scope(scope) => scope,
        _ => unreachable!(),
    };
    dap.limits.dap_output_bytes = 512;
    let mut output = Vec::new();
    let error = dap
        .reply(
            pending(PendingKind::Variables {
                parent: VariableParent::Scope(scope),
                start: 0,
                requested: None,
                variables: Vec::new(),
            }),
            DebugReply::Variables {
                variables: vec![DebugVariable {
                    name: JsString::from_utf8("x"),
                    value: DebugValue {
                        preview: JsString::from_utf8("unavailable"),
                        object: None,
                        accessor: false,
                        unavailable: Some("reason".repeat(200)),
                    },
                }],
                total: 1,
            },
            &mut output,
        )
        .unwrap_err();
    assert!(matches!(error, DapError::Capacity));
    assert!(output.is_empty());
    assert!(dap.parents.is_empty());
}

#[test]
fn adapters_require_an_entry_barrier_and_close_on_framing_or_output_failure() {
    let identity = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&identity, ContextLimits::default()).unwrap();
    let backend = std::rc::Rc::new(Backend::new());
    let coordinator = DebugCoordinator::new(
        backend,
        store.context(),
        DebugLimits::default(),
        std::sync::Arc::new(|| Ok(())),
        NonZeroU64::new(1).unwrap(),
    )
    .unwrap();
    let handle = coordinator
        .attach(DebugSessionOptions {
            break_on_start: false,
        })
        .unwrap();
    assert!(matches!(
        DapAdapter::new(handle),
        Err(DapError::Protocol(_))
    ));

    let (_store, _backend, coordinator, mut dap) = fixture();
    assert!(dap.receive(b"Invalid-Header\r\n\r\n").is_err());
    assert!(dap.disconnected);
    coordinator.service().unwrap();
    assert_eq!(coordinator.state(), DebugExecutionState::Detached);

    let (_store, backend, coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", serde_json::json!({}));
    dap.limits.dap_output_bytes = 1;
    backend.pause(PauseReason::Exception);
    assert!(matches!(dap.poll(), Err(DapError::Capacity)));
    assert!(dap.disconnected);
    coordinator.service().unwrap();
    assert_eq!(coordinator.state(), DebugExecutionState::Detached);
}

#[test]
fn attach_disconnect_and_source_path_failures_keep_protocol_correlation() {
    let (_store, _backend, coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", serde_json::json!({}));
    let attached = request(&mut dap, 2, "attach", serde_json::json!({}));
    assert_eq!(attached.len(), 1);
    assert_eq!(attached[0]["command"], "attach");
    assert_eq!(attached[0]["success"], true);
    assert!(
        request(
            &mut dap,
            3,
            "source",
            serde_json::json!({"source":{"path":"file:///missing.js"}}),
        )
        .is_empty()
    );
    let source = drive(&coordinator, &mut dap);
    assert_eq!(source.len(), 1);
    assert_eq!(source[0]["request_seq"], 3);
    assert_eq!(source[0]["success"], false);
    assert!(!dap.disconnected);
    let disconnected = request(&mut dap, 4, "disconnect", serde_json::json!({}));
    assert_eq!(disconnected.len(), 1);
    assert_eq!(disconnected[0]["success"], true);
    assert!(dap.disconnected);
    assert!(
        drive(&coordinator, &mut dap)
            .iter()
            .any(|message| message["event"] == "terminated")
    );
}

#[test]
fn optional_breakpoint_columns_are_validated_before_unsupported_native_admission() {
    let (_store, backend, _coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", serde_json::json!({}));
    for (seq, column) in [(2, 0), (3, 1)] {
        let output = request(
            &mut dap,
            seq,
            "setBreakpoints",
            serde_json::json!({"source":{"path":"file:///a.js"},"breakpoints":[{"line":1,"column":column}]}),
        );
        assert_eq!(output.len(), 1);
        assert_eq!(output[0]["request_seq"], seq);
        assert_eq!(output[0]["success"], false);
        assert!(dap.pending.is_empty());
        assert!(backend.last_breakpoints.borrow().is_empty());
    }
    let breakpoint = BreakpointMeta {
        id: 7,
        origin: "file:///a.js".into(),
        line: 2,
        column: Some(4),
    };
    let value = dap
        .breakpoint_value(&breakpoint, &BreakpointState::Pending)
        .unwrap();
    let value = decode(&frame_value(&value, 1024).unwrap());
    assert_eq!(value["line"], 3);
    assert_eq!(value["column"], 5);
    assert_eq!(value["verified"], false);
}

#[test]
fn retained_stack_frames_and_exhausted_scope_references_fail_before_output() {
    let (_store, backend, _coordinator, mut dap) = fixture();
    let pause = backend.pause(PauseReason::Requested);
    dap.synchronize_pause();
    let port = backend.port();
    let identities = port.identities();
    let frame = match identities
        .register(pause, ReferenceKind::Frame, NonZeroU64::new(41).unwrap())
        .unwrap()
    {
        DebugReference::Frame(frame) => frame,
        _ => unreachable!(),
    };
    let scope = match identities
        .register(pause, ReferenceKind::Scope, NonZeroU64::new(42).unwrap())
        .unwrap()
    {
        DebugReference::Scope(scope) => scope,
        _ => unreachable!(),
    };
    dap.limits.dap_output_bytes = 512;
    let mut output = Vec::new();
    assert!(matches!(
        dap.reply(
            pending(PendingKind::Stack {
                pause,
                start: 0,
                requested: None,
                frames: Vec::new()
            }),
            DebugReply::Stack {
                frames: vec![DebugFrame {
                    id: frame,
                    name: JsString::from_units(vec![65; 1024]),
                    generated: None,
                    original: None,
                }],
                total: 1
            },
            &mut output,
        ),
        Err(DapError::Capacity)
    ));
    assert!(output.is_empty());
    assert!(dap.frames.is_empty());
    dap.limits.inspection_objects = 0;
    assert!(matches!(
        dap.parent_reference(VariableParent::Scope(scope)),
        Err(DapError::Capacity)
    ));
    assert!(dap.parents.is_empty());
}

#[test]
fn response_encoding_failure_detaches_for_each_immediate_command() {
    for command in [
        "initialize",
        "attach",
        "threads",
        "pause",
        "continue",
        "next",
        "stepIn",
        "stepOut",
        "disconnect",
        "configurationDone",
        "unsupported",
    ] {
        let (_store, backend, coordinator, mut dap) = fixture();
        if command != "initialize" {
            request(&mut dap, 1, "initialize", serde_json::json!({}));
        }
        if matches!(command, "continue" | "next" | "stepIn" | "stepOut") {
            request(&mut dap, 2, "configurationDone", serde_json::json!({}));
            backend.pause(PauseReason::Requested);
            dap.poll().unwrap();
        }
        dap.limits.dap_output_bytes = 1;
        let result = dap.receive(&wire(serde_json::json!({
            "seq":3,"type":"request","command":command,"arguments":{}
        })));
        assert!(
            matches!(result, Err(DapError::Capacity)),
            "{command}: {result:?}"
        );
        assert!(dap.disconnected, "{command}");
        coordinator.service().unwrap();
        assert_eq!(
            coordinator.state(),
            DebugExecutionState::Detached,
            "{command}"
        );
    }
}

#[test]
fn event_encoding_failure_detaches_for_each_native_event_family() {
    for event in [
        "continued",
        "loadedSource",
        "breakpoint",
        "diagnostic",
        "closed",
    ] {
        let (store, backend, coordinator, mut dap) = fixture();
        request(&mut dap, 1, "initialize", serde_json::json!({}));
        let port = backend.port();
        match event {
            "continued" => {
                request(&mut dap, 2, "configurationDone", serde_json::json!({}));
                backend.pause(PauseReason::Requested);
                dap.poll().unwrap();
                request(&mut dap, 3, "continue", serde_json::json!({}));
                coordinator.service().unwrap();
            }
            "loadedSource" => {
                port.register_script(NativeScriptMetadata {
                    key: NativeScriptKey::new(store.context(), NonZeroU64::new(51).unwrap()),
                    source: ScriptSource::new(JsString::from_utf8("x"), "file:///event.js".into()),
                    kind: ScriptKind::Script,
                })
                .unwrap();
            }
            "breakpoint" => {
                request(
                    &mut dap,
                    2,
                    "setBreakpoints",
                    serde_json::json!({
                        "source":{"path":"file:///event.js"},"breakpoints":[{"line":1}]
                    }),
                );
                drive(&coordinator, &mut dap);
                port.publish(DebugUpdate::BreakpointsChanged(vec![BreakpointInfo {
                    client_id: dap.breakpoints[0].id,
                    state: BreakpointState::Rejected("changed".into()),
                }]))
                .unwrap();
            }
            "diagnostic" => port
                .publish(DebugUpdate::Diagnostic {
                    report: FailureReport::fault(FailureCode::EngineFailure, "diagnostic"),
                    dropped: 0,
                })
                .unwrap(),
            "closed" => {
                let session = backend.session.take().unwrap();
                port.identities().close_session(session).unwrap();
                port.publish(DebugUpdate::SessionClosed {
                    session,
                    reason: Some(FailureReport::fault(FailureCode::EngineFailure, "closed")),
                })
                .unwrap();
            }
            _ => unreachable!(),
        }
        dap.limits.dap_output_bytes = 1;
        assert!(matches!(dap.poll(), Err(DapError::Capacity)), "{event}");
        assert!(dap.disconnected, "{event}");
        coordinator.service().unwrap();
        assert_eq!(
            coordinator.state(),
            DebugExecutionState::Detached,
            "{event}"
        );
    }
}

#[test]
fn native_reply_encoding_failures_do_not_leave_pending_requests_or_live_sessions() {
    for outcome in [
        "native error",
        "unexpected reply",
        "oversized result",
        "exception",
    ] {
        let (_store, backend, coordinator, mut dap) = fixture();
        request(&mut dap, 1, "initialize", serde_json::json!({}));
        let port = backend.port();
        if matches!(outcome, "oversized result" | "exception") {
            let pause = backend.pause(PauseReason::Requested);
            dap.poll().unwrap();
            let frame = match port
                .identities()
                .register(pause, ReferenceKind::Frame, NonZeroU64::new(53).unwrap())
                .unwrap()
            {
                DebugReference::Frame(frame) => frame,
                _ => unreachable!(),
            };
            let frame_id = dap.frame_reference(frame).unwrap();
            request(
                &mut dap,
                2,
                "evaluate",
                serde_json::json!({"frameId":frame_id,"expression":"x"}),
            );
        } else {
            request(
                &mut dap,
                2,
                "setExceptionBreakpoints",
                serde_json::json!({"filters":[]}),
            );
        }
        let command = match port.next_directive(false).unwrap() {
            DebugDirective::Request(command) => command,
            other => panic!("expected admitted setup request, got {other:?}"),
        };
        let result = match outcome {
            "native error" => Err(DebugError::Unsupported),
            "unexpected reply" => Ok(DebugReply::Scopes(Vec::new())),
            "oversized result" | "exception" => Ok(DebugReply::Evaluation {
                value: DebugValue {
                    preview: JsString::from_utf8("result"),
                    object: None,
                    accessor: false,
                    unavailable: None,
                },
                exception: outcome == "exception",
            }),
            _ => unreachable!(),
        };
        port.publish(DebugUpdate::Reply {
            id: command.id,
            result,
        })
        .unwrap();
        dap.limits.dap_output_bytes = 1;
        assert!(matches!(dap.poll(), Err(DapError::Capacity)), "{outcome}");
        assert!(dap.pending.is_empty(), "{outcome}");
        assert!(dap.disconnected, "{outcome}");
        coordinator.service().unwrap();
        assert_eq!(
            coordinator.state(),
            DebugExecutionState::Detached,
            "{outcome}"
        );
    }
}

#[test]
fn repeated_configuration_and_inspection_while_running_fail_without_native_admission() {
    let (_store, _backend, coordinator, mut dap) = fixture();
    let before_initialize = request(&mut dap, 1, "attach", serde_json::json!({}));
    assert_eq!(before_initialize[0]["success"], false);
    request(&mut dap, 2, "initialize", serde_json::json!({}));
    request(&mut dap, 3, "configurationDone", serde_json::json!({}));
    for (seq, command, arguments) in [
        (4, "configurationDone", serde_json::json!({})),
        (5, "stackTrace", serde_json::json!({})),
        (6, "continue", serde_json::json!({})),
        (
            7,
            "evaluate",
            serde_json::json!({"frameId":999,"expression":"x"}),
        ),
    ] {
        let output = request(&mut dap, seq, command, arguments);
        assert_eq!(output.len(), 1, "{command}");
        assert_eq!(output[0]["request_seq"], seq);
        assert_eq!(output[0]["success"], false, "{command}");
        assert!(dap.pending.is_empty());
        assert!(!dap.disconnected);
    }
    assert_eq!(coordinator.state(), DebugExecutionState::Running);
}

#[test]
fn excessive_native_source_pages_are_rejected_before_allocating_protocol_references() {
    let (store, backend, _coordinator, mut dap) = fixture();
    let port = backend.port();
    let registration = port
        .register_script(NativeScriptMetadata {
            key: NativeScriptKey::new(store.context(), NonZeroU64::new(52).unwrap()),
            source: ScriptSource::new(JsString::from_utf8("x"), "file:///page.js".into()),
            kind: ScriptKind::Script,
        })
        .unwrap();
    let script = ScriptInfo {
        registration,
        origin: "file:///page.js".into(),
        length_units: 1,
    };
    for (limit, total) in [(0, 1), (1, 0)] {
        dap.limits.scripts = limit;
        let mut output = Vec::new();
        assert!(matches!(
            dap.reply(
                pending(PendingKind::Sources {
                    scripts: Vec::new()
                }),
                DebugReply::Sources {
                    scripts: vec![script.clone()],
                    total
                },
                &mut output,
            ),
            Err(DapError::Capacity)
        ));
        assert!(output.is_empty());
        assert!(dap.sources.is_empty());
    }
}

#[test]
fn a_full_native_request_lane_rejects_every_command_without_losing_the_admitted_request() {
    let limits = DebugLimits {
        requests: 1,
        ..DebugLimits::default()
    };
    let (_store, backend, coordinator, mut dap) = fixture_with_limits(limits);
    request(&mut dap, 1, "initialize", serde_json::json!({}));
    request(&mut dap, 2, "configurationDone", serde_json::json!({}));
    let pause = backend.pause(PauseReason::Requested);
    dap.poll().unwrap();
    let port = backend.port();
    let frame = match port
        .identities()
        .register(pause, ReferenceKind::Frame, NonZeroU64::new(61).unwrap())
        .unwrap()
    {
        DebugReference::Frame(frame) => frame,
        _ => unreachable!(),
    };
    let scope = match port
        .identities()
        .register(pause, ReferenceKind::Scope, NonZeroU64::new(62).unwrap())
        .unwrap()
    {
        DebugReference::Scope(scope) => scope,
        _ => unreachable!(),
    };
    let frame_id = dap.frame_reference(frame).unwrap();
    let variables_reference = dap.parent_reference(VariableParent::Scope(scope)).unwrap();
    assert!(request(&mut dap, 3, "loadedSources", serde_json::json!({})).is_empty());
    for (index, (command, arguments)) in [
        ("stackTrace", serde_json::json!({})),
        ("scopes", serde_json::json!({"frameId":frame_id})),
        (
            "variables",
            serde_json::json!({"variablesReference":variables_reference}),
        ),
        (
            "evaluate",
            serde_json::json!({"frameId":frame_id,"expression":"x"}),
        ),
        ("setExceptionBreakpoints", serde_json::json!({"filters":[]})),
        (
            "setBreakpoints",
            serde_json::json!({"source":{"path":"file:///a.js"}}),
        ),
        ("loadedSources", serde_json::json!({})),
        (
            "source",
            serde_json::json!({"source":{"path":"file:///a.js"}}),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let seq = index as u64 + 4;
        let rejected = request(&mut dap, seq, command, arguments);
        assert_eq!(rejected.len(), 1, "{command}");
        assert_eq!(rejected[0]["request_seq"], seq);
        assert_eq!(rejected[0]["success"], false, "{command}");
        assert_eq!(dap.pending.len(), 1, "{command}");
        assert!(!dap.disconnected, "{command}");
    }
    let completed = drive(&coordinator, &mut dap);
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0]["request_seq"], 3);
    assert_eq!(completed[0]["success"], true);
    assert_eq!(completed[0]["body"]["sources"], serde_json::json!([]));
    assert!(dap.pending.is_empty());
    assert_eq!(coordinator.state(), DebugExecutionState::Paused(pause));
}

#[test]
fn failure_to_wake_an_automatic_entry_resume_detaches_instead_of_stranding_the_pause() {
    let (_store, backend, coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", serde_json::json!({}));
    request(&mut dap, 2, "configurationDone", serde_json::json!({}));
    backend.pause(PauseReason::Entry);
    backend
        .signal
        .0
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(matches!(
        dap.poll(),
        Err(DapError::Debug(DebugError::WakeFailed(_)))
    ));
    assert!(dap.disconnected);
    backend
        .signal
        .0
        .store(false, std::sync::atomic::Ordering::Release);
    assert!(matches!(
        coordinator.service(),
        Err(DebugError::WakeFailed(_))
    ));
    assert_eq!(coordinator.state(), DebugExecutionState::Detached);
}

#[test]
fn closure_between_native_pages_returns_one_correlated_failure_instead_of_partial_success() {
    for command in ["stackTrace", "variables", "loadedSources", "source"] {
        let (store, backend, coordinator, mut dap) = fixture();
        let port = backend.port();
        if matches!(command, "source" | "loadedSources") {
            let scripts = if command == "source" { 1 } else { 35 };
            for index in 1..=scripts {
                port.register_script(NativeScriptMetadata {
                    key: NativeScriptKey::new(store.context(), NonZeroU64::new(index).unwrap()),
                    source: ScriptSource::new(
                        JsString::from_utf8(&"x".repeat(if command == "source" {
                            20_000
                        } else {
                            1
                        })),
                        format!("file:///page{index}.js"),
                    ),
                    kind: ScriptKind::Script,
                })
                .unwrap();
            }
        }
        request(&mut dap, 1, "initialize", serde_json::json!({}));
        let arguments = if matches!(command, "stackTrace" | "variables") {
            let pause = backend.pause(PauseReason::Requested);
            dap.poll().unwrap();
            if command == "stackTrace" {
                backend.frame_count.set(65);
                serde_json::json!({})
            } else {
                backend.variable_count.set(65);
                let scope = match port
                    .identities()
                    .register(pause, ReferenceKind::Scope, NonZeroU64::new(63).unwrap())
                    .unwrap()
                {
                    DebugReference::Scope(scope) => scope,
                    _ => unreachable!(),
                };
                let reference = dap.parent_reference(VariableParent::Scope(scope)).unwrap();
                serde_json::json!({"variablesReference":reference})
            }
        } else if command == "source" {
            serde_json::json!({"sourceReference":dap.sources[0].reference})
        } else {
            serde_json::json!({})
        };
        assert!(request(&mut dap, 2, command, arguments).is_empty());
        coordinator.service().unwrap();
        backend.detach().unwrap();
        let output: Vec<_> = dap
            .poll()
            .unwrap()
            .iter()
            .map(|frame| decode(frame))
            .collect();
        let replies: Vec<_> = output
            .iter()
            .filter(|message| message["request_seq"] == 2)
            .collect();
        assert_eq!(replies.len(), 1, "{command}: {output:?}");
        assert_eq!(replies[0]["success"], false, "{command}");
        assert!(
            output
                .iter()
                .any(|message| message["event"] == "terminated"),
            "{command}"
        );
        assert!(dap.pending.is_empty(), "{command}");
        assert!(dap.disconnected, "{command}");
    }
}

#[test]
fn a_reply_without_adapter_correlation_does_not_create_a_protocol_response() {
    let (_store, _backend, coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", serde_json::json!({}));
    dap.handle
        .try_request(DebugRequest::ExceptionPolicy(ExceptionPolicy::None))
        .unwrap();
    coordinator.service().unwrap();
    assert!(dap.poll().unwrap().is_empty());
    assert!(dap.pending.is_empty());
    let threads = request(&mut dap, 2, "threads", serde_json::json!({}));
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0]["request_seq"], 2);
    assert_eq!(threads[0]["success"], true);
}

#[test]
fn state_sequence_validation_rejects_replays_without_advancing_the_last_sequence() {
    let mut last = 4;
    assert!(!advance_state_sequence(&mut last, 4));
    assert_eq!(last, 4);
    assert!(!advance_state_sequence(&mut last, 3));
    assert_eq!(last, 4);
    assert!(advance_state_sequence(&mut last, 5));
    assert_eq!(last, 5);
    assert!(!advance_state_sequence(&mut last, 4));
    assert_eq!(last, 5);
}

#[test]
fn request_rejection_mapping_preserves_debug_errors_and_rejects_other_transport_reasons() {
    assert!(matches!(
        request_rejection(RejectionReason::Debug(DebugError::NotPaused)),
        DapError::Debug(DebugError::NotPaused)
    ));
    for reason in [
        RejectionReason::Full,
        RejectionReason::Closed,
        RejectionReason::Unsupported,
    ] {
        assert!(matches!(request_rejection(reason), DapError::Protocol(_)));
    }
}

#[test]
fn output_failure_recovery_encodes_only_capacity_failures_and_preserves_other_errors() {
    let (_store, _backend, _coordinator, mut dap) = fixture();
    let capacity = dap
        .output_failure_response(7, "evaluate", DapError::Capacity)
        .unwrap();
    let capacity = decode(&capacity);
    assert_eq!(capacity["request_seq"], 7);
    assert_eq!(capacity["command"], "evaluate");
    assert_eq!(capacity["success"], false);
    assert!(capacity["message"].as_str().unwrap().contains("allowance"));
    assert!(matches!(
        dap.output_failure_response(8, "evaluate", DapError::Protocol("encoding failed".into())),
        Err(DapError::Protocol(message)) if message == "encoding failed"
    ));
    assert!(matches!(
        dap.output_failure_response(9, "evaluate", DapError::Debug(DebugError::Closed)),
        Err(DapError::Debug(DebugError::Closed))
    ));
    assert_eq!(dap.output_seq, 1);
}

#[test]
fn replayed_native_state_updates_do_not_duplicate_events_or_invalidate_newer_frames() {
    let (_store, backend, _coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", serde_json::json!({}));
    request(&mut dap, 2, "configurationDone", serde_json::json!({}));
    let port = backend.port();
    let identities = port.identities();
    let pause = identities
        .begin_pause(backend.session.get().unwrap())
        .unwrap();
    let paused = DebugUpdate::Paused {
        sequence: identities.next_update_sequence().unwrap(),
        pause,
        reason: PauseReason::Breakpoint,
        location: None,
        original: None,
        breakpoint_ids: vec![],
    };
    port.publish(paused.clone()).unwrap();
    let mut output = Vec::new();
    assert!(dap.process_update(paused.clone(), &mut output).unwrap());
    assert_eq!(output.len(), 1);
    assert_eq!(decode(&output[0])["event"], "stopped");
    output.clear();
    assert!(!dap.process_update(paused.clone(), &mut output).unwrap());
    assert!(output.is_empty());
    assert!(dap.poll().unwrap().is_empty());

    identities.end_pause(pause).unwrap();
    let resumed = DebugUpdate::Resumed {
        sequence: identities.next_update_sequence().unwrap(),
        previous: pause,
    };
    port.publish(resumed.clone()).unwrap();
    assert!(dap.process_update(resumed.clone(), &mut output).unwrap());
    assert_eq!(output.len(), 1);
    assert_eq!(decode(&output[0])["event"], "continued");
    output.clear();
    assert!(!dap.process_update(resumed.clone(), &mut output).unwrap());
    assert!(output.is_empty());
    assert!(dap.poll().unwrap().is_empty());

    let current = backend.pause(PauseReason::Requested);
    assert_eq!(dap.poll().unwrap().len(), 1);
    let frame = match identities
        .register(current, ReferenceKind::Frame, NonZeroU64::new(71).unwrap())
        .unwrap()
    {
        DebugReference::Frame(frame) => frame,
        _ => unreachable!(),
    };
    let frame_id = dap.frame_reference(frame).unwrap();
    let last_sequence = dap.last_state_sequence;
    for stale in [paused, resumed] {
        assert!(!dap.process_update(stale, &mut output).unwrap());
        assert!(output.is_empty());
        assert_eq!(dap.pause, Some(current));
        assert_eq!(dap.last_state_sequence, last_sequence);
        assert_eq!(dap.frame(frame_id).unwrap(), frame);
    }
}

#[test]
fn release_ack_window_keeps_the_original_dap_sequence_reserved() {
    let (_store, backend, coordinator, mut dap) = fixture_with_capabilities(
        DebugLimits::default(),
        DebugCapabilities::baseline().with(DebugCapability::LazyInspection),
    );
    request(&mut dap, 1, "initialize", serde_json::json!({}));
    request(&mut dap, 2, "configurationDone", serde_json::json!({}));
    let pause = backend.pause(PauseReason::Breakpoint);
    dap.poll().unwrap();
    let cursor = match backend
        .port()
        .identities()
        .register(pause, ReferenceKind::Cursor, NonZeroU64::new(42).unwrap())
        .unwrap()
    {
        DebugReference::Cursor(cursor) => cursor,
        _ => unreachable!(),
    };
    let pending = Pending {
        seq: 7,
        command: "variables".into(),
        order: 3,
        setup: false,
        kind: PendingKind::Inspection {
            retried: false,
            reclaimed: false,
            staged_targets: vec![],
            reference: 1,
            start: 0,
            count: 64,
            unpaged: false,
            cursor: Some(cursor),
        },
    };
    dap.release_cursor(
        cursor,
        ReleaseNext::Fail(Box::new(pending), DapError::Capacity),
    )
    .unwrap();
    // The owner has not serviced ReleaseCursor yet: its ACK is deliberately pending.
    assert_eq!(dap.pending.len(), 1);
    let duplicate = request(&mut dap, 7, "scopes", serde_json::json!({"frameId":123}));
    assert!(
        duplicate.iter().any(|r| r["request_seq"] == 7
            && r["success"] == false
            && r["message"]
                .as_str()
                .unwrap()
                .contains("duplicate in-flight")),
        "{duplicate:?}"
    );
    assert_eq!(backend.scopes.get(), 0);
    assert_eq!(dap.pending.len(), 1);
    coordinator.service().unwrap();
    dap.poll().unwrap();
    assert!(dap.pending.is_empty());
}

fn evaluator_cleanup_rejection_keeps_terminal_response(later: bool) {
    let (_store, backend, coordinator, mut dap) = fixture_with_capabilities(
        DebugLimits {
            requests: 2,
            ..DebugLimits::default()
        },
        DebugCapabilities::baseline()
            .with(DebugCapability::LazyInspection)
            .with(DebugCapability::CancelEvaluation),
    );
    let pause = backend.pause(PauseReason::Breakpoint);
    dap.poll().unwrap();
    let port = backend.port();
    let DebugReference::Frame(frame) = port
        .identities()
        .register(pause, ReferenceKind::Frame, NonZeroU64::new(1).unwrap())
        .unwrap()
    else {
        panic!()
    };
    let evaluation = |seq, order| Pending {
        seq,
        order,
        command: "evaluate".into(),
        setup: false,
        kind: PendingKind::Evaluation(frame),
    };
    let request = || DebugRequest::Evaluate {
        frame,
        expression: JsString::from_utf8("metadata operation"),
        allow_side_effects: true,
    };
    dap.submit(request(), evaluation(71, 17)).unwrap();
    let DebugDirective::Request(original) = port.next_directive(false).unwrap() else {
        panic!()
    };
    port.publish(DebugUpdate::Reply {
        id: original.id,
        result: Err(DebugError::Capacity),
    })
    .unwrap();

    let register_cursors = |dap: &mut DapAdapter| {
        for key in [41, 42] {
            let DebugReference::Cursor(cursor) = port
                .identities()
                .register(pause, ReferenceKind::Cursor, NonZeroU64::new(key).unwrap())
                .unwrap()
            else {
                panic!()
            };
            dap.inspection_checkpoints.insert(key, (64, cursor));
        }
    };
    let mut released = None;
    if later {
        register_cursors(&mut dap);
        assert!(dap.poll().unwrap().is_empty());
        let DebugDirective::Request(release) = port.next_directive(false).unwrap() else {
            panic!()
        };
        let DebugRequest::ReleaseCursor(cursor) = release.request else {
            panic!()
        };
        released = Some(cursor);
        port.identities().release_cursor(cursor).unwrap();
        port.publish(DebugUpdate::Reply {
            id: release.id,
            result: Ok(DebugReply::CursorReleased),
        })
        .unwrap();
    }
    // A second real transport evaluator is dispatched before the frontend drains
    // the preceding reply. Cursor release admission must now reject InFlight.
    dap.submit(request(), evaluation(72, 18)).unwrap();
    let DebugDirective::Request(active) = port.next_directive(false).unwrap() else {
        panic!()
    };
    if !later {
        register_cursors(&mut dap);
    }
    let result = dap.poll();
    assert!(
        result.is_ok(),
        "recoverable release rejection lost correlation: {result:?}"
    );
    let frames: Vec<_> = result.unwrap().iter().map(|frame| decode(frame)).collect();
    let terminal: Vec<_> = frames
        .iter()
        .filter(|frame| frame["request_seq"] == 71)
        .collect();
    assert_eq!(terminal.len(), 1, "{frames:?}");
    assert_eq!(terminal[0]["command"], "evaluate");
    assert_eq!(terminal[0]["success"], false);
    assert_eq!(terminal[0]["message"], "Capacity");
    assert!(!dap.disconnected);
    assert!(!coordinator.is_stopping());
    assert_eq!(
        port.evaluation_control(active.id)
            .unwrap()
            .poll_at(std::time::Instant::now()),
        None
    );
    assert_eq!(dap.inspection_checkpoints.len(), if later { 1 } else { 2 });
    if let Some(released) = released {
        assert!(
            dap.inspection_checkpoints
                .values()
                .all(|(_, cursor)| *cursor != released)
        );
    }
    assert_eq!(dap.pending.len(), 1);
    assert_eq!(dap.pending[&active.id].seq, 72);
    assert_eq!(dap.pending[&active.id].order, 18);
    assert!(
        dap.poll().unwrap().is_empty(),
        "duplicate evaluator response"
    );
    port.publish(DebugUpdate::Reply {
        id: active.id,
        result: Err(DebugError::Closed),
    })
    .unwrap();
    coordinator.shutdown().unwrap();
}

#[test]
fn evaluator_error_first_cleanup_rejection_keeps_response_and_live_checkpoints() {
    evaluator_cleanup_rejection_keeps_terminal_response(false);
}

#[test]
fn evaluator_error_later_cleanup_rejection_keeps_response_and_remaining_checkpoints() {
    evaluator_cleanup_rejection_keeps_terminal_response(true);
}

fn editing_pending(reference: u64, count: usize) -> Pending {
    Pending {
        seq: 71,
        command: "variables".into(),
        order: 17,
        setup: false,
        kind: PendingKind::Inspection {
            reference,
            start: 0,
            count,
            unpaged: false,
            cursor: None,
            retried: false,
            reclaimed: false,
            staged_targets: vec![],
        },
    }
}
fn editing_page_fixture(
    dap: &mut DapAdapter,
    port: &Rc<dyn DebugPort>,
    pause: PauseId,
    count: usize,
    continuation: bool,
) -> (FrameRef, u64, InspectionPage) {
    let DebugReference::Frame(frame) = port
        .identities()
        .register(pause, ReferenceKind::Frame, NonZeroU64::new(100).unwrap())
        .unwrap()
    else {
        panic!()
    };
    let DebugReference::Object(object) = port
        .identities()
        .register(pause, ReferenceKind::Object, NonZeroU64::new(101).unwrap())
        .unwrap()
    else {
        panic!()
    };
    dap.frame_reference(frame).unwrap();
    let reference = dap
        .parent_reference_in_frame(VariableParent::Object(object), Some(frame))
        .unwrap();
    let items = (0..count)
        .map(|i| {
            let DebugReference::Property(target) = port
                .identities()
                .register(
                    pause,
                    ReferenceKind::Property,
                    NonZeroU64::new(200 + i as u64).unwrap(),
                )
                .unwrap()
            else {
                panic!()
            };
            InspectionEntry {
                variable: DebugVariable {
                    name: JsString::from_utf8(&format!("p{i}")),
                    value: DebugValue {
                        preview: JsString::from_utf8("1"),
                        object: None,
                        accessor: false,
                        unavailable: None,
                    },
                },
                type_name: JsString::from_utf8("number"),
                write: Some(WriteTarget::Property(target)),
                write_unavailable: None,
            }
        })
        .collect();
    let next = if continuation {
        let DebugReference::Cursor(cursor) = port
            .identities()
            .register(pause, ReferenceKind::Cursor, NonZeroU64::new(3000).unwrap())
            .unwrap()
        else {
            panic!()
        };
        Some(cursor)
    } else {
        None
    };
    (
        frame,
        reference,
        InspectionPage {
            items,
            next,
            complete: !continuation,
            scanned_slots: count,
            total: None,
        },
    )
}
fn acknowledge_editing_cleanup(port: &Rc<dyn DebugPort>) -> usize {
    let DebugDirective::Request(command) = port.next_directive(false).unwrap() else {
        panic!()
    };
    let (reply, count) = match command.request {
        DebugRequest::ReleaseWriteTargets(targets) => {
            for target in &targets {
                let reference = match target {
                    WriteTarget::Binding(r) => DebugReference::Binding(*r),
                    WriteTarget::Property(r) => DebugReference::Property(*r),
                };
                if port.identities().resolve(reference).is_ok() {
                    port.identities().release_target(reference).unwrap();
                }
            }
            (DebugReply::WriteTargetsReleased, targets.len())
        }
        DebugRequest::ReleaseCursor(cursor) => {
            port.identities().release_cursor(cursor).unwrap();
            (DebugReply::CursorReleased, 0)
        }
        other => panic!("{other:?}"),
    };
    port.publish(DebugUpdate::Reply {
        id: command.id,
        result: Ok(reply),
    })
    .unwrap();
    count
}
#[test]
fn discarded_page_cleanup_keeps_ids_cursor_and_response_on_first_or_later_admission_rejection() {
    for later in [false, true] {
        let (_store, backend, _coordinator, mut dap) = fixture_with_capabilities(
            DebugLimits {
                requests: 2,
                ..DebugLimits::default()
            },
            DebugCapabilities::baseline()
                .with(DebugCapability::LazyInspection)
                .with(DebugCapability::VariableMutation),
        );
        let pause = backend.pause(PauseReason::Breakpoint);
        dap.poll().unwrap();
        let port = backend.port();
        let (frame, reference, page) = editing_page_fixture(&mut dap, &port, pause, 1, true);
        let target = page.items[0].write.unwrap();
        let cursor = page.next.unwrap();
        let mut output = vec![];
        let evaluation = Pending {
            seq: 72,
            command: "evaluate".into(),
            order: 18,
            setup: false,
            kind: PendingKind::Evaluation(frame),
        };
        let eval_request = DebugRequest::Evaluate {
            frame,
            expression: JsString::from_utf8("1"),
            allow_side_effects: true,
        };
        let active;
        if !later {
            dap.submit(eval_request, evaluation).unwrap();
            let DebugDirective::Request(command) = port.next_directive(false).unwrap() else {
                panic!()
            };
            active = command;
            dap.reply(
                editing_pending(reference, 2),
                DebugReply::Inspection(page),
                &mut output,
            )
            .unwrap();
            assert!(dap.deferred_submission.is_some());
        } else {
            dap.reply(
                editing_pending(reference, 2),
                DebugReply::Inspection(page),
                &mut output,
            )
            .unwrap();
            // Queue an evaluator after the target release has been dispatched;
            // it blocks admission of the following cursor release.
            let DebugDirective::Request(release) = port.next_directive(false).unwrap() else {
                panic!()
            };
            let DebugRequest::ReleaseWriteTargets(targets) = release.request else {
                panic!()
            };
            for target in targets {
                let WriteTarget::Property(target) = target else {
                    panic!()
                };
                port.identities()
                    .release_target(DebugReference::Property(target))
                    .unwrap();
            }
            port.publish(DebugUpdate::Reply {
                id: release.id,
                result: Ok(DebugReply::WriteTargetsReleased),
            })
            .unwrap();
            dap.submit(eval_request, evaluation).unwrap();
            let DebugDirective::Request(command) = port.next_directive(false).unwrap() else {
                panic!()
            };
            active = command;
            output.extend(dap.poll().unwrap());
            assert!(dap.deferred_submission.is_some());
        }
        assert!(output.is_empty());
        port.publish(DebugUpdate::Reply {
            id: active.id,
            result: Ok(DebugReply::Evaluation {
                value: DebugValue {
                    preview: JsString::from_utf8("1"),
                    object: None,
                    accessor: false,
                    unavailable: None,
                },
                exception: false,
            }),
        })
        .unwrap();
        output.extend(dap.poll().unwrap());
        dap.initialized = true;
        for (seq, command, args) in [
            (
                81,
                "variables",
                serde_json::json!({"variablesReference":reference,"count":1}),
            ),
            (
                82,
                "evaluate",
                serde_json::json!({"frameId":dap.frames[0].0,"expression":"1","context":"repl"}),
            ),
            (
                83,
                "setVariable",
                serde_json::json!({"variablesReference":reference,"name":"p0","value":"1"}),
            ),
        ] {
            let rejected = request(&mut dap, seq, command, args);
            assert_eq!(rejected.len(), 1, "{later}/{command}: {rejected:?}");
            assert_eq!(rejected[0]["success"], false);
            assert!(
                rejected[0]["message"]
                    .as_str()
                    .unwrap()
                    .contains("InFlight"),
                "{later}/{command}: {rejected:?}"
            );
        }
        if !later {
            assert_eq!(acknowledge_editing_cleanup(&port), 1);
            output.extend(dap.poll().unwrap());
        }
        assert!(
            !dap.pending.is_empty(),
            "cleanup pending missing later={later} deferred={} output={:?}",
            dap.deferred_submission.is_some(),
            output.iter().map(|wire| decode(wire)).collect::<Vec<_>>()
        );
        match port.next_directive(false) {
            Some(DebugDirective::Request(command)) => {
                let DebugRequest::ReleaseCursor(actual) = command.request else {
                    panic!("expected cursor cleanup")
                };
                assert_eq!(actual, cursor);
                port.identities().release_cursor(cursor).unwrap();
                port.publish(DebugUpdate::Reply {
                    id: command.id,
                    result: Ok(DebugReply::CursorReleased),
                })
                .unwrap();
            }
            None => assert_eq!(
                port.identities().resolve(DebugReference::Cursor(cursor)),
                Err(DebugError::StaleInspection),
                "coordinator completes stale cursor release without native dispatch"
            ),
            other => panic!("{other:?}"),
        }
        output.extend(dap.poll().unwrap());
        let original: Vec<_> = output
            .iter()
            .map(|wire| decode(wire))
            .filter(|frame| frame["request_seq"] == 71)
            .collect();
        assert_eq!(original.len(), 1);
        assert_eq!(original[0]["success"], false);
        let WriteTarget::Property(target) = target else {
            panic!()
        };
        assert_eq!(
            port.identities().resolve(DebugReference::Property(target)),
            Err(DebugError::StaleInspection)
        );
        assert_eq!(
            port.identities().resolve(DebugReference::Cursor(cursor)),
            Err(DebugError::StaleInspection)
        );
        assert!(!dap.disconnected);
        assert!(dap.pending.is_empty());
        assert!(dap.deferred_submission.is_none());
    }
}
#[test]
fn staged_page_materialization_failure_cleans_unexposed_targets_and_keeps_terminal() {
    let (_store, backend, _coordinator, mut dap) = fixture_with_capabilities(
        DebugLimits::default(),
        DebugCapabilities::baseline()
            .with(DebugCapability::LazyInspection)
            .with(DebugCapability::VariableMutation),
    );
    let pause = backend.pause(PauseReason::Breakpoint);
    dap.poll().unwrap();
    let port = backend.port();
    let (_, reference, mut page) = editing_page_fixture(&mut dap, &port, pause, 1, false);
    let DebugReference::Object(child) = port
        .identities()
        .register(pause, ReferenceKind::Object, NonZeroU64::new(102).unwrap())
        .unwrap()
    else {
        panic!()
    };
    page.items[0].variable.value.object = Some(child);
    dap.limits.inspection_objects = dap.frames.len() + dap.parents.len();
    let mut output = vec![];
    dap.finish_cursor_release(
        ReleaseNext::Page {
            pending: Box::new(editing_pending(reference, 1)),
            page,
            released: vec![],
        },
        Ok(DebugReply::WriteTargetsReleased),
        &mut output,
    )
    .unwrap();
    assert!(output.is_empty());
    acknowledge_editing_cleanup(&port);
    output.extend(dap.poll().unwrap());
    let original: Vec<_> = output
        .iter()
        .map(|wire| decode(wire))
        .filter(|frame| frame["request_seq"] == 71)
        .collect();
    assert_eq!(original.len(), 1);
    assert_eq!(original[0]["success"], false);
    assert!(!dap.disconnected);
}
#[test]
fn cleanup_ack_after_resume_never_reinstalls_or_publishes_staged_old_page() {
    let (_store, backend, _coordinator, mut dap) = fixture_with_capabilities(
        DebugLimits::default(),
        DebugCapabilities::baseline()
            .with(DebugCapability::LazyInspection)
            .with(DebugCapability::VariableMutation),
    );
    let pause = backend.pause(PauseReason::Breakpoint);
    dap.poll().unwrap();
    let port = backend.port();
    let (_, reference, page) = editing_page_fixture(&mut dap, &port, pause, 1, false);
    port.identities().end_pause(pause).unwrap();
    port.publish(DebugUpdate::Resumed {
        sequence: port.identities().next_update_sequence().unwrap(),
        previous: pause,
    })
    .unwrap();
    let next = backend.pause(PauseReason::Breakpoint);
    dap.poll().unwrap();
    assert_eq!(dap.pause, Some(next));
    let mut output = vec![];
    dap.finish_cursor_release(
        ReleaseNext::Page {
            pending: Box::new(editing_pending(reference, 1)),
            page,
            released: vec![],
        },
        Err(DebugError::StalePause),
        &mut output,
    )
    .unwrap();
    output.extend(dap.poll().unwrap());
    let original: Vec<_> = output
        .iter()
        .map(|wire| decode(wire))
        .filter(|frame| frame["request_seq"] == 71)
        .collect();
    assert_eq!(original.len(), 1);
    assert_eq!(original[0]["success"], false);
    assert!(dap.write_views.is_empty());
    assert!(dap.inspection_checkpoints.is_empty());
    assert!(!dap.disconnected);
}
#[test]
fn target_cleanup_chunks_actual_owned_bytes_below_request_budget() {
    let limits = DebugLimits {
        request_bytes: size_of::<DebugRequest>() + size_of::<WriteTarget>() * 2,
        ..DebugLimits::default()
    };
    let (_store, backend, _coordinator, mut dap) = fixture_with_capabilities(
        limits,
        DebugCapabilities::baseline()
            .with(DebugCapability::LazyInspection)
            .with(DebugCapability::VariableMutation),
    );
    let pause = backend.pause(PauseReason::Breakpoint);
    dap.poll().unwrap();
    let port = backend.port();
    let (_, reference, page) = editing_page_fixture(&mut dap, &port, pause, 7, false);
    let targets = page.items.iter().filter_map(|entry| entry.write).collect();
    let mut output = vec![];
    dap.release_targets(
        targets,
        ReleaseNext::Discard {
            pending: Box::new(editing_pending(reference, 7)),
            error: DapError::Capacity,
            cursor: None,
        },
    )
    .unwrap();
    let mut released = 0;
    for expected in [2, 2, 2, 1] {
        let count = acknowledge_editing_cleanup(&port);
        assert_eq!(count, expected);
        released += count;
        output.extend(dap.poll().unwrap());
    }
    assert_eq!(released, 7);
    let original: Vec<_> = output
        .iter()
        .map(|wire| decode(wire))
        .filter(|frame| frame["request_seq"] == 71)
        .collect();
    assert_eq!(original.len(), 1);
    assert_eq!(original[0]["success"], false);
    assert!(dap.pending.is_empty());
}

#[test]
fn combined_capacity_and_editable_metadata_accounting_bounds_many_page_names() {
    let limits = DebugLimits {
        inspection_objects: 8,
        inspection_page_items: 2,
        write_targets: 1,
        ..DebugLimits::default()
    };
    let (_store, backend, _coordinator, mut dap) = fixture_with_capabilities(
        limits,
        DebugCapabilities::baseline()
            .with(DebugCapability::LazyInspection)
            .with(DebugCapability::VariableMutation),
    );
    let pause = backend.pause(PauseReason::Breakpoint);
    dap.poll().unwrap();
    let port = backend.port();
    let (_, reference, page) = editing_page_fixture(&mut dap, &port, pause, 1, false);
    let target = page.items[0].write.unwrap();
    let view = vec![(0, JsString::from_utf8("editable"), target)];
    dap.write_views.insert(reference, view.clone());
    dap.view_order.push(reference);
    for at in (1..400).step_by(2) {
        let mut capacity = dap
            .write_capacity_views
            .get(&reference)
            .cloned()
            .unwrap_or_default();
        capacity.push((at, JsString::from_utf8(&format!("capacity-{at}"))));
        capacity.push((at + 1, JsString::from_utf8(&format!("capacity-{}", at + 1))));
        dap.bound_capacity_views(reference, at..at + 2, &view, &mut capacity)
            .unwrap();
        dap.write_capacity_views.insert(reference, capacity);
        assert!(
            dap.write_views.values().map(Vec::len).sum::<usize>()
                + dap
                    .write_capacity_views
                    .values()
                    .map(Vec::len)
                    .sum::<usize>()
                <= 16
        );
        assert_eq!(
            dap.write_views[&reference][0].2, target,
            "existing editable row must remain owned while capacity-only names are evicted"
        );
    }
    let retained_names: usize = dap
        .write_views
        .values()
        .flatten()
        .map(|(_, name, _)| name.capacity_bytes())
        .sum::<usize>()
        + dap
            .write_capacity_views
            .values()
            .flatten()
            .map(|(_, name)| name.capacity_bytes())
            .sum::<usize>();
    assert!(retained_names < 1024);
}

#[test]
fn completed_lookup_reply_after_resume_never_publishes_old_values_or_references() {
    for new_pause in [false, true] {
        for object_reply in [false, true] {
            let (_store, backend, _coordinator, mut dap) = fixture_with_capabilities(
                DebugLimits::default(),
                DebugCapabilities::baseline().with(DebugCapability::SafeLookup),
            );
            request(&mut dap, 1, "initialize", serde_json::json!({}));
            let old = backend.pause(PauseReason::Requested);
            dap.poll().unwrap();
            let port = backend.port();
            let DebugReference::Frame(frame) = port
                .identities()
                .register(old, ReferenceKind::Frame, NonZeroU64::new(710).unwrap())
                .unwrap()
            else {
                panic!()
            };
            let frame_id = dap.frame_reference(frame).unwrap();
            let DebugReference::Object(object) = port
                .identities()
                .register(old, ReferenceKind::Object, NonZeroU64::new(711).unwrap())
                .unwrap()
            else {
                panic!()
            };
            assert!(
                request(
                    &mut dap,
                    2,
                    "evaluate",
                    serde_json::json!({"frameId":frame_id,"expression":"value","context":"hover"})
                )
                .is_empty()
            );
            let Some(DebugDirective::Request(command)) = port.next_directive(false) else {
                panic!()
            };
            assert!(matches!(command.request, DebugRequest::LookupPath { .. }));
            port.publish(DebugUpdate::Reply {
                id: command.id,
                result: Ok(DebugReply::Lookup(DebugValue {
                    preview: JsString::from_utf8("old value"),
                    object: object_reply.then_some(object),
                    accessor: false,
                    unavailable: None,
                })),
            })
            .unwrap();
            port.identities().end_pause(old).unwrap();
            port.publish(DebugUpdate::Resumed {
                sequence: port.identities().next_update_sequence().unwrap(),
                previous: old,
            })
            .unwrap();
            let mut current_mapping = None;
            if new_pause {
                let current = backend.pause(PauseReason::Breakpoint);
                dap.synchronize_pause();
                let DebugReference::Frame(fresh) = port
                    .identities()
                    .register(current, ReferenceKind::Frame, NonZeroU64::new(712).unwrap())
                    .unwrap()
                else {
                    panic!()
                };
                let fresh_id = dap.frame_reference(fresh).unwrap();
                current_mapping = Some((fresh_id, fresh));
            }
            dap.submit(
                DebugRequest::Sources {
                    page: Page { start: 0, count: 1 },
                },
                pending(PendingKind::Simple),
            )
            .unwrap();
            let unrelated = *dap.pending.keys().find(|id| **id != command.id).unwrap();
            let out = dap.poll().unwrap();
            let terminal: Vec<_> = out
                .iter()
                .map(|wire| decode(wire))
                .filter(|value| value["request_seq"] == 2)
                .collect();
            assert_eq!(terminal.len(), 1);
            assert_eq!(terminal[0]["success"], false, "{terminal:?}");
            assert_eq!(terminal[0]["message"], "StalePause");
            assert!(dap.parents.is_empty());
            assert!(dap.parent_frames.is_empty());
            if let Some((id, fresh)) = current_mapping {
                assert_eq!(dap.frame(id).unwrap(), fresh);
            }
            assert_eq!(dap.pending.len(), 1);
            assert!(dap.pending.contains_key(&unrelated));
            assert!(!dap.disconnected);
        }
    }
}
