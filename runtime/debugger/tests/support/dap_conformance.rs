// Shared DAP conformance assertions, included (`include!`) by the external API tests
// and the internal guard build.
// Shared assertions run against both the external API and internal guard build.
#[test]
fn framing_handles_fragmentation_multiple_messages_and_checked_output_lengths() {
    let mut framer = DapFramer::new(1024, 1024).unwrap();
    let first = wire(json!({"seq":1,"type":"request","command":"threads"}));
    assert!(framer.push(&first[..8]).unwrap().is_empty());
    let rest = framer.push(&first[8..]).unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(serde_json::from_slice::<Value>(&rest[0]).unwrap()["seq"], 1);
    let encoded = framer.encode(&rest[0]).unwrap();
    assert_eq!(encoded, first);
    let mut pair = first.clone();
    pair.extend(&first);
    assert_eq!(framer.push(&pair).unwrap().len(), 2);
    assert!(framer.encode(&vec![b'x'; 1025]).is_err());
}
#[test]
fn malformed_or_oversized_content_length_fails_before_body_admission() {
    for header in [
        b"Content-Length: 2048\r\n\r\n".as_slice(),
        b"Content-Length: 1\r\nContent-Length: 2\r\n\r\n".as_slice(),
        b"Content-Length: -1\r\n\r\n".as_slice(),
    ] {
        assert!(DapFramer::new(1024, 1024).unwrap().push(header).is_err());
    }
}
#[test]
fn configuration_done_waits_for_terminal_setup_replies_before_releasing_entry_pause() {
    let (_store, backend, coordinator, mut dap) = fixture();
    let init = request(
        &mut dap,
        1,
        "initialize",
        json!({"linesStartAt1":true,"columnsStartAt1":true}),
    );
    assert_eq!(init[0]["body"]["supportsConfigurationDoneRequest"], true);
    assert!(init.iter().any(|v| v["event"] == "initialized"));
    backend.pause(PauseReason::Entry);
    assert!(
        request(
            &mut dap,
            2,
            "setBreakpoints",
            json!({"source":{"path":"file:///future.ts"},"breakpoints":[{"line":1}]})
        )
        .iter()
        .all(|v| v["request_seq"] != 2)
    );
    assert!(
        request(&mut dap, 3, "configurationDone", json!({}))
            .iter()
            .all(|v| v["request_seq"] != 3)
    );
    assert_eq!(backend.continued.get(), 0);
    let replies = drive(&coordinator, &mut dap);
    assert!(
        replies
            .iter()
            .any(|v| v["request_seq"] == 2 && v["body"]["breakpoints"][0]["verified"] == false)
    );
    assert!(replies.iter().any(|v| v["request_seq"] == 3));
    coordinator.service().unwrap();
    assert_eq!(backend.continued.get(), 1);
}
#[test]
fn full_source_larger_than_common_reply_is_assembled_without_truncation() {
    let (store, backend, coordinator, mut dap) = fixture();
    let source = "x".repeat(400_000);
    backend
        .port()
        .register_script(NativeScriptMetadata {
            key: NativeScriptKey::new(store.context(), NonZeroU64::new(1).unwrap()),
            source: ScriptSource::new(JsString::from_utf8(&source), "file:///large.js".to_owned()),
            kind: ScriptKind::Script,
        })
        .unwrap();
    request(&mut dap, 1, "initialize", json!({}));
    request(&mut dap, 2, "loadedSources", json!({}));
    let mut sources = drive(&coordinator, &mut dap);
    for _ in 0..10 {
        if sources.iter().any(|v| v["request_seq"] == 2) {
            break;
        }
        sources.extend(drive(&coordinator, &mut dap));
    }
    let response = sources.iter().find(|v| v["request_seq"] == 2).unwrap();
    let reference = response["body"]["sources"][0]["sourceReference"]
        .as_i64()
        .unwrap();
    request(&mut dap, 3, "source", json!({"sourceReference":reference}));
    let mut result = None;
    for _ in 0..100 {
        for value in drive(&coordinator, &mut dap) {
            if value["request_seq"] == 3 {
                result = Some(value);
            }
        }
        if result.is_some() {
            break;
        }
    }
    let response = result.expect("complete source response");
    assert_eq!(response["success"], true);
    assert_eq!(response["body"]["content"].as_str().unwrap(), source);
}
#[test]
fn frame_reference_numbers_are_never_reused_after_resume_or_coalesced_pause() {
    let (_store, backend, coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", json!({}));
    request(&mut dap, 2, "configurationDone", json!({}));
    let first = backend.pause(PauseReason::Breakpoint);
    dap.poll().unwrap();
    request(&mut dap, 3, "stackTrace", json!({"threadId":1}));
    let result = drive(&coordinator, &mut dap);
    let frame =
        result.iter().find(|v| v["request_seq"] == 3).unwrap()["body"]["stackFrames"][0]["id"]
            .as_i64()
            .unwrap();
    let handle_state = coordinator.state();
    assert_eq!(handle_state, DebugExecutionState::Paused(first));
    request(&mut dap, 4, "continue", json!({"threadId":1}));
    coordinator.service().unwrap();
    backend.pause(PauseReason::Breakpoint);
    let rejection = request(&mut dap, 5, "scopes", json!({"frameId":frame}));
    assert!(
        rejection
            .iter()
            .any(|v| v["request_seq"] == 5 && v["success"] == false)
    );
    assert_eq!(backend.scopes.get(), 0);
}
#[test]
fn evaluation_reply_is_correlated_after_a_later_native_callback() {
    let (_store, backend, coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", json!({}));
    request(&mut dap, 2, "configurationDone", json!({}));
    backend.pause(PauseReason::Breakpoint);
    dap.poll().unwrap();
    request(&mut dap, 3, "stackTrace", json!({"threadId":1}));
    let frames = drive(&coordinator, &mut dap);
    let frame =
        frames.iter().find(|v| v["request_seq"] == 3).unwrap()["body"]["stackFrames"][0]["id"]
            .clone();
    backend.async_eval.set(true);
    let immediate = request(
        &mut dap,
        4,
        "evaluate",
        json!({"frameId":frame,"expression":"1+1","context":"repl"}),
    );
    assert!(immediate.iter().all(|v| v["request_seq"] != 4));
    assert!(
        drive(&coordinator, &mut dap)
            .iter()
            .all(|v| v["request_seq"] != 4)
    );
    backend.finish();
    let result = dap
        .poll()
        .unwrap()
        .iter()
        .map(|f| decode(f))
        .collect::<Vec<_>>();
    assert!(
        result
            .iter()
            .any(|v| v["request_seq"] == 4 && v["body"]["result"] == "async fixture")
    );
}

fn initialized_pause(
    backend: &Backend,
    coordinator: &DebugCoordinator,
    dap: &mut DapAdapter,
) -> u64 {
    request(dap, 1, "initialize", json!({"pathFormat":"uri"}));
    request(dap, 2, "configurationDone", json!({}));
    backend.pause(PauseReason::Breakpoint);
    dap.poll().unwrap();
    request(dap, 3, "stackTrace", json!({"threadId":1}));
    drive(coordinator, dap)
        .iter()
        .find(|v| v["request_seq"] == 3)
        .unwrap()["body"]["stackFrames"][0]["id"]
        .as_u64()
        .unwrap()
}
#[test]
fn scopes_and_paged_variables_preserve_expandable_references() {
    let (_store, backend, coordinator, mut dap) = fixture();
    let frame = initialized_pause(&backend, &coordinator, &mut dap);
    request(&mut dap, 4, "scopes", json!({"frameId":frame}));
    let scopes = drive(&coordinator, &mut dap);
    let reference=scopes.iter().find(|v|v["request_seq"]==4).unwrap()["body"]["scopes"][0]["variablesReference"].as_u64().unwrap();
    request(
        &mut dap,
        5,
        "variables",
        json!({"variablesReference":reference,"start":0,"count":1}),
    );
    let variables = drive(&coordinator, &mut dap);
    let value = &variables.iter().find(|v| v["request_seq"] == 5).unwrap()["body"]["variables"][0];
    assert_eq!(value["name"], "item0");
    assert!(value["variablesReference"].as_u64().unwrap() > 0);
    request(
        &mut dap,
        6,
        "variables",
        json!({"variablesReference":value["variablesReference"],"count":1}),
    );
    assert!(
        drive(&coordinator, &mut dap)
            .iter()
            .any(|v| v["request_seq"] == 6 && v["success"] == true)
    );
}
#[test]
fn unpaged_variables_gather_every_common_page_instead_of_truncating_at_sixty_four() {
    let (_store, backend, coordinator, mut dap) = fixture();
    let frame = initialized_pause(&backend, &coordinator, &mut dap);
    backend.variable_count.set(70);
    request(&mut dap, 4, "scopes", json!({"frameId":frame}));
    let scopes = drive(&coordinator, &mut dap);
    let reference=scopes.iter().find(|v|v["request_seq"]==4).unwrap()["body"]["scopes"][0]["variablesReference"].clone();
    request(
        &mut dap,
        5,
        "variables",
        json!({"variablesReference":reference}),
    );
    let mut output = vec![];
    for _ in 0..10 {
        output.extend(drive(&coordinator, &mut dap));
        if output.iter().any(|v| v["request_seq"] == 5) {
            break;
        }
    }
    let variables = output.iter().find(|v| v["request_seq"] == 5).unwrap()["body"]["variables"]
        .as_array()
        .unwrap();
    assert_eq!(variables.len(), 70);
    assert_eq!(variables[69]["name"], "item69");
}
#[test]
fn pause_and_every_step_command_translate_to_urgent_native_controls() {
    let (_store, backend, coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", json!({}));
    request(&mut dap, 2, "configurationDone", json!({}));
    assert_eq!(
        request(&mut dap, 3, "threads", json!({}))[0]["body"]["threads"][0]["id"],
        1
    );
    assert_eq!(
        request(&mut dap, 4, "pause", json!({"threadId":1}))[0]["success"],
        true
    );
    assert!(
        drive(&coordinator, &mut dap)
            .iter()
            .any(|v| v["event"] == "stopped")
    );
    for (seq, command) in [(5, "next"), (6, "stepIn"), (7, "stepOut")] {
        assert_eq!(
            request(&mut dap, seq, command, json!({"threadId":1}))[0]["success"],
            true
        );
        assert!(
            drive(&coordinator, &mut dap)
                .iter()
                .any(|v| v["event"] == "continued")
        );
        backend.pause(PauseReason::Step);
        dap.poll().unwrap();
    }
    assert_eq!(
        &*backend.actions.borrow(),
        &[
            DebugContinue::StepOver,
            DebugContinue::StepInto,
            DebugContinue::StepOut
        ]
    );
    assert_eq!(
        request(&mut dap, 8, "pause", json!({"threadId":2}))[0]["success"],
        false
    );
}
#[test]
fn exception_filter_configuration_and_unsupported_extensions_are_truthful() {
    let (_store, backend, coordinator, mut dap) = fixture();
    let init = request(&mut dap, 1, "initialize", json!({}));
    assert_eq!(init[0]["body"]["supportsLogPoints"], false);
    assert_eq!(init[0]["body"]["supportsEvaluateForHovers"], false);
    for (seq, filters) in [
        (2, json!(["all"])),
        (3, json!(["uncaught"])),
        (4, json!([])),
    ] {
        request(
            &mut dap,
            seq,
            "setExceptionBreakpoints",
            json!({"filters":filters}),
        );
        assert!(
            drive(&coordinator, &mut dap)
                .iter()
                .any(|v| v["request_seq"] == seq && v["success"] == true)
        );
    }
    assert_eq!(
        &*backend.policies.borrow(),
        &[
            ExceptionPolicy::All,
            ExceptionPolicy::Uncaught,
            ExceptionPolicy::None
        ]
    );
    assert_eq!(
        request(
            &mut dap,
            5,
            "setExceptionBreakpoints",
            json!({"filters":["caught"]})
        )[0]["success"],
        false
    );
    assert_eq!(
        request(&mut dap, 6, "setVariable", json!({}))[0]["success"],
        false
    );
}
#[test]
fn breakpoint_conditions_verified_rejected_and_changed_states_keep_client_identity() {
    let (store, backend, coordinator, mut dap) = fixture();
    backend
        .port()
        .register_script(NativeScriptMetadata {
            key: NativeScriptKey::new(store.context(), NonZeroU64::new(1).unwrap()),
            source: ScriptSource::new(JsString::from_utf8("x"), "file:///tmp/a.js".to_owned()),
            kind: ScriptKind::Script,
        })
        .unwrap();
    request(&mut dap, 1, "initialize", json!({"pathFormat":"uri"}));
    backend.breakpoint_mode.set(1);
    request(
        &mut dap,
        2,
        "setBreakpoints",
        json!({"source":{"path":"file:///tmp/a.js"},"breakpoints":[{"line":1,"condition":"ready"}]}),
    );
    let output = drive(&coordinator, &mut dap);
    let breakpoint =
        &output.iter().find(|v| v["request_seq"] == 2).unwrap()["body"]["breakpoints"][0];
    assert_eq!(breakpoint["verified"], true);
    let id = breakpoint["id"].as_u64().unwrap();
    assert_eq!(
        backend.last_breakpoints.borrow()[0]
            .condition
            .as_ref()
            .unwrap()
            .to_utf8()
            .unwrap(),
        "ready"
    );
    backend
        .port()
        .publish(DebugUpdate::BreakpointsChanged(vec![BreakpointInfo {
            client_id: id,
            state: BreakpointState::Rejected("fixture changed".to_owned()),
        }]))
        .unwrap();
    assert!(
        dap.poll()
            .unwrap()
            .iter()
            .map(|v| decode(v))
            .any(|v| v["event"] == "breakpoint"
                && v["body"]["breakpoint"]["message"] == "fixture changed")
    );
    backend.breakpoint_mode.set(2);
    request(
        &mut dap,
        3,
        "setBreakpoints",
        json!({"source":{"path":"file:///tmp/a.js"},"breakpoints":[{"line":1}]}),
    );
    assert!(
        drive(&coordinator, &mut dap)
            .iter()
            .any(|v| v["request_seq"] == 3 && v["body"]["breakpoints"][0]["verified"] == false)
    );
    assert_eq!(
        request(
            &mut dap,
            4,
            "setBreakpoints",
            json!({"source":{"path":"file:///tmp/a.js"},"breakpoints":[{"line":1,"logMessage":"unsupported"}]})
        )[0]["success"],
        false
    );
}
#[test]
fn clearing_a_breakpoint_set_removes_its_old_protocol_identity() {
    let (_store, backend, coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", json!({}));
    request(
        &mut dap,
        2,
        "setBreakpoints",
        json!({"source":{"path":"file:///a.js"},"breakpoints":[{"line":1}]}),
    );
    drive(&coordinator, &mut dap);
    let id = backend.last_breakpoints.borrow()[0].client_id;
    request(
        &mut dap,
        3,
        "setBreakpoints",
        json!({"source":{"path":"file:///a.js"},"breakpoints":[]}),
    );
    drive(&coordinator, &mut dap);
    backend
        .port()
        .publish(DebugUpdate::BreakpointsChanged(vec![BreakpointInfo {
            client_id: id,
            state: BreakpointState::Rejected("stale".to_owned()),
        }]))
        .unwrap();
    assert!(
        dap.poll()
            .unwrap()
            .iter()
            .map(|v| decode(v))
            .all(|v| v["event"] != "breakpoint")
    );
}
#[test]
fn mapped_stack_locations_and_original_source_retrieval_use_real_original_content() {
    let (store, backend, coordinator, mut dap) = fixture();
    let source = ScriptSource {
        text: JsString::from_utf8("x"),
        origin: "file:///out.js".to_owned(),
        source_map: Some(SourceMapAttachment {
            json:
                br#"{"version":3,"sources":["a.ts"],"sourcesContent":["aa\nbb"],"mappings":"AACA"}"#
                    .to_vec(),
            originals: vec![],
            preceding_maps: vec![],
        }),
    };
    let registration = backend
        .port()
        .register_script(NativeScriptMetadata {
            key: NativeScriptKey::new(store.context(), NonZeroU64::new(1).unwrap()),
            source,
            kind: ScriptKind::Script,
        })
        .unwrap();
    *backend.frame_location.borrow_mut() = Some(DebugLocation {
        script: registration.id,
        revision: registration.revision,
        line: 0,
        column: Some(0),
    });
    let frame = initialized_pause(&backend, &coordinator, &mut dap);
    assert!(frame > 0);
    request(&mut dap, 4, "stackTrace", json!({"threadId":1}));
    let output = drive(&coordinator, &mut dap);
    let item = &output.iter().find(|v| v["request_seq"] == 4).unwrap()["body"]["stackFrames"][0];
    assert_eq!(item["line"], 2);
    assert_eq!(item["source"]["path"], "file:///a.ts");
    let reference = item["source"]["sourceReference"].clone();
    request(&mut dap, 5, "source", json!({"sourceReference":reference}));
    let content = drive(&coordinator, &mut dap);
    assert!(
        content
            .iter()
            .any(|v| v["request_seq"] == 5 && v["body"]["content"] == "aa\nbb")
    );
}
#[test]
fn native_source_updates_wait_for_the_initialize_handshake() {
    let (store, backend, _coordinator, mut dap) = fixture();
    backend
        .port()
        .register_script(NativeScriptMetadata {
            key: NativeScriptKey::new(store.context(), NonZeroU64::new(1).unwrap()),
            source: ScriptSource::new(JsString::from_utf8("x"), "file:///a.js".to_owned()),
            kind: ScriptKind::Script,
        })
        .unwrap();
    let initialized = request(&mut dap, 1, "initialize", json!({}));
    assert_eq!(initialized[0]["type"], "response");
    assert_eq!(initialized[1]["event"], "initialized");
}
#[test]
fn malformed_utf8_disconnects_instead_of_leaving_the_owner_suspended() {
    let (_store, backend, coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", json!({}));
    backend.pause(PauseReason::Breakpoint);
    assert!(dap.receive(b"Content-Length: 1\r\n\r\n\xff").is_err());
    coordinator.service().unwrap();
    assert_eq!(coordinator.state(), DebugExecutionState::Detached);
}
#[test]
fn accepted_wake_failure_produces_one_terminal_protocol_response() {
    let (_store, backend, coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", json!({}));
    backend.signal.0.store(true, Ordering::Release);
    let mut output = request(&mut dap, 2, "loadedSources", json!({}));
    assert!(
        output.iter().all(|v| v["request_seq"] != 2),
        "accepted failure must await its reserved terminal result"
    );
    let _ = coordinator.service();
    output.extend(dap.poll().unwrap().iter().map(|v| decode(v)));
    assert_eq!(output.iter().filter(|v| v["request_seq"] == 2).count(), 1);
    assert!(output.iter().any(|v| v["event"] == "terminated"));
}
#[test]
fn adapter_does_not_acknowledge_launching_an_application_it_did_not_start() {
    let (_store, _backend, _coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", json!({}));
    assert_eq!(
        request(&mut dap, 2, "launch", json!({"program":"not-started.js"}))[0]["success"],
        false
    );
}
#[test]
fn configuration_cannot_release_startup_before_initialization() {
    let (_store, _backend, _coordinator, mut dap) = fixture();
    assert_eq!(
        request(&mut dap, 1, "configurationDone", json!({}))[0]["success"],
        false
    );
}

#[test]
fn excessive_unpaged_variables_return_one_dap_error_and_keep_the_pause_usable() {
    let (_store, backend, coordinator, mut dap) = fixture_with_limits(DebugLimits {
        source_bytes: 512,
        dap_output_bytes: 1536,
        ..DebugLimits::default()
    });
    let frame = initialized_pause(&backend, &coordinator, &mut dap);
    backend.variable_count.set(70);
    request(&mut dap, 4, "scopes", json!({"frameId":frame}));
    let scopes = drive(&coordinator, &mut dap);
    let reference=scopes.iter().find(|v|v["request_seq"]==4).unwrap()["body"]["scopes"][0]["variablesReference"].clone();
    request(
        &mut dap,
        5,
        "variables",
        json!({"variablesReference":reference}),
    );
    let output = drive(&coordinator, &mut dap);
    assert!(
        output
            .iter()
            .any(|v| v["request_seq"] == 5 && v["success"] == false)
    );
    assert!(matches!(
        coordinator.state(),
        DebugExecutionState::Paused(_)
    ));
    assert_eq!(
        request(&mut dap, 6, "continue", json!({"threadId":1}))[0]["success"],
        true
    );
}
#[test]
fn stack_levels_zero_gathers_all_common_pages() {
    let (_store, backend, coordinator, mut dap) = fixture();
    request(&mut dap, 1, "initialize", json!({}));
    request(&mut dap, 2, "configurationDone", json!({}));
    backend.frame_count.set(70);
    backend.pause(PauseReason::Breakpoint);
    dap.poll().unwrap();
    request(&mut dap, 3, "stackTrace", json!({"threadId":1,"levels":0}));
    let mut output = vec![];
    for _ in 0..10 {
        output.extend(drive(&coordinator, &mut dap));
        if output.iter().any(|v| v["request_seq"] == 3) {
            break;
        }
    }
    let reply = output.iter().find(|v| v["request_seq"] == 3).unwrap();
    assert_eq!(reply["body"]["totalFrames"], 70);
    assert_eq!(reply["body"]["stackFrames"].as_array().unwrap().len(), 70);
}

#[test]
fn generated_stack_locations_and_default_thread_ids_round_trip_without_a_source_map() {
    let (store,backend,coordinator,mut dap)=fixture();
    request(&mut dap,1,"initialize",json!({}));
    let registration=backend.port().register_script(NativeScriptMetadata {key:NativeScriptKey::new(store.context(),NonZeroU64::new(71).unwrap()),source:ScriptSource::new(JsString::from_utf8("let x=1;"),"file:///generated.js".into()),kind:ScriptKind::Script}).unwrap();
    *backend.frame_location.borrow_mut()=Some(DebugLocation {script:registration.id,revision:registration.revision,line:0,column:Some(4)});
    request(&mut dap,2,"pause",json!({}));
    drive(&coordinator,&mut dap);
    request(&mut dap,3,"stackTrace",json!({"levels":1}));
    let output=drive(&coordinator,&mut dap);
    let frame=&output.iter().find(|v|v["request_seq"]==3).unwrap()["body"]["stackFrames"][0];
    assert_eq!(frame["line"],1);assert_eq!(frame["column"],5);assert!(frame["source"]["sourceReference"].as_u64().unwrap()>0);
}
#[test]
fn loaded_sources_gathers_more_than_one_common_page_without_losing_registrations() {
    let (store,backend,coordinator,mut dap)=fixture();
    request(&mut dap,1,"initialize",json!({}));
    for i in 1..=35 {backend.port().register_script(NativeScriptMetadata {key:NativeScriptKey::new(store.context(),NonZeroU64::new(i).unwrap()),source:ScriptSource::new(JsString::from_utf8("x"),format!("file:///source{i}.js")),kind:ScriptKind::Script}).unwrap();}
    request(&mut dap,2,"loadedSources",json!({}));
    let mut output=Vec::new();
    for _ in 0..4 {output.extend(drive(&coordinator,&mut dap));}
    let response=output.iter().find(|v|v["request_seq"]==2).unwrap();
    assert_eq!(response["success"],true);assert_eq!(response["body"]["sources"].as_array().unwrap().len(),35);
}
#[test]
fn diagnostics_and_native_close_reasons_are_bounded_dap_events() {
    let (_store,backend,_coordinator,mut dap)=fixture();
    request(&mut dap,1,"initialize",json!({}));
    let mut report=FailureReport::new(FailureClass::Host,FailureCode::EngineFailure,"debug report");report.message=None;
    backend.port().publish(DebugUpdate::Diagnostic {report,dropped:2}).unwrap();
    let output:Vec<Value>=dap.poll().unwrap().iter().map(|f|decode(f)).collect();
    assert!(output.iter().any(|v|v["event"]=="output"&&v["body"]["output"].as_str().unwrap().contains("2 diagnostics omitted")));
    let session=backend.session.take().unwrap();backend.port().identities().close_session(session).unwrap();
    backend.port().publish(DebugUpdate::SessionClosed {session,reason:Some(FailureReport::new(FailureClass::Host,FailureCode::EngineFailure,"native control closed"))}).unwrap();
    let output:Vec<Value>=dap.poll().unwrap().iter().map(|f|decode(f)).collect();
    assert!(output.iter().any(|v|v["event"]=="terminated"));
    assert!(output.iter().any(|v|v["body"]["category"]=="stderr"));
}
