//! Private scope guards exercised with live native values and checked store tokens.
use super::*;
use std::time::Duration;

struct NoServices;
impl HostServices for NoServices {
    #[allow(clippy::result_large_err)]
    fn post(&self, _: CallbackJob) -> Result<TaskId, Rejected<CallbackJob>> {
        unreachable!("these scope tests do not dispatch host work")
    }
    #[allow(clippy::result_large_err)]
    fn timer(
        &self,
        _: Duration,
        _: Option<Duration>,
        _: CallbackJob,
    ) -> Result<TaskId, Rejected<CallbackJob>> {
        unreachable!("these scope tests do not dispatch timers")
    }
    fn cancel(&self, _: TaskId) -> CancelOutcome {
        unreachable!()
    }
    fn request_stop(&self) -> Result<(), FailureReport> {
        unreachable!()
    }
    fn reserve_operation(&self, _: usize) -> Result<OperationReservation, FailureReport> {
        unreachable!()
    }
    fn bind_deferred(
        &self,
        _: OperationReservation,
        _: RootedDeferred,
        _: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport> {
        unreachable!()
    }
    fn bind_callback(
        &self,
        _: OperationReservation,
        _: RootedValue,
        _: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport> {
        unreachable!()
    }
    fn cancel_operation(&self, _: OperationId) -> CancelOutcome {
        unreachable!()
    }
}

fn live_scope(config: QuickJsNgConfig, body: impl FnOnce(&Scope<'_, '_>)) {
    let engine = QuickJsNgEngine::with_config(config).unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    let state = engine.context(id).unwrap();
    state
        .values
        .with_scope(|values| {
            body(&Scope {
                state: &state,
                values,
                host: &NoServices,
            });
            Ok(())
        })
        .unwrap();
    engine.close_context(id).unwrap();
}
fn evaluate<'s>(scope: &'s Scope<'_, '_>, text: &str) -> ScopedValue<'s> {
    scope
        .source()
        .unwrap()
        .evaluate(&ScriptSource::new(
            JsString::from_utf8(text),
            "file:///scope-private.js".into(),
        ))
        .unwrap()
}
fn fault() -> FailureReport {
    FailureReport::new(
        FailureClass::Host,
        FailureCode::HostPanic,
        "preserved private test fault",
    )
}

#[test]
fn resolver_capabilities_are_rejected_as_values_and_cannot_be_reused() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        // SAFETY: Undefined is a valid owned primitive and requires no allocation.
        let mut hidden = NativeValue::new(unsafe { ffi::jbq_undefined() }, scope.state.raw.clone());
        hidden.resolver = true;
        let hidden = scope.values.insert(hidden).unwrap();
        assert_eq!(
            scope.read(hidden).err().unwrap().code,
            FailureCode::InvalidHandle
        );
        let ordinary = evaluate(scope, "42");
        let fake = scope.values.retain_deferred(ordinary).unwrap();
        assert_eq!(
            scope
                .detach(
                    scope
                        .settle(fake, Settlement::Resolve(ordinary))
                        .unwrap_err()
                )
                .code,
            FailureCode::InvalidHandle
        );
        let pair = scope.values.read(hidden).unwrap();
        pair.settled.set(true);
        let reused = scope.values.retain_deferred(hidden).unwrap();
        assert_eq!(
            scope
                .detach(
                    scope
                        .settle(reused, Settlement::Resolve(ordinary))
                        .unwrap_err()
                )
                .code,
            FailureCode::InvalidHandle
        );
    });
}

#[test]
fn property_keys_require_symbols_and_preserve_valid_native_symbols() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        let number = evaluate(scope, "7");
        let error = scope.key(PropertyKey::Symbol(number)).err().unwrap();
        assert_eq!(scope.detach(error).code, FailureCode::InvalidHandle);
        let symbol = evaluate(scope, "Symbol('private')");
        assert_eq!(
            scope.native_kind(scope.key(PropertyKey::Symbol(symbol)).unwrap().value),
            ValueKind::Symbol
        );
    });
}

#[test]
fn typed_pending_faults_override_success_status_and_native_exceptions() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        scope.state.raw.record_fault(fault());
        // SAFETY: Undefined is a valid owned primitive for this live context.
        let error = scope.owned(unsafe { ffi::jbq_undefined() }).err().unwrap();
        assert_eq!(scope.detach(error), fault());
        scope.state.raw.record_fault(fault());
        assert_eq!(scope.detach(scope.check_status(0).unwrap_err()), fault());
        scope.state.raw.record_fault(fault());
        let source = ScriptSource::new(
            JsString::from_utf8("throw 'original'"),
            "file:///pending.js".into(),
        );
        assert_eq!(
            scope.detach(scope.source().unwrap().evaluate(&source).unwrap_err()),
            fault()
        );
        assert!(scope.state.raw.take_fault().is_none());
        assert_eq!(
            scope
                .export_data(evaluate(scope, "40 + 2"), ExportLimits::default())
                .unwrap(),
            JsData::Number(42.)
        );
    });
}

#[test]
fn diagnostic_failure_preserves_typed_fault_and_never_recurses_into_exception() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        assert_eq!(diagnostic_failure(ScopedError::Fault(fault())), fault());
        let exception = evaluate(scope, "'original diagnostic exception'");
        let failure = diagnostic_failure(ScopedError::Exception(exception));
        assert_eq!(failure.class, FailureClass::Engine);
        assert_eq!(failure.code, FailureCode::EngineFailure);
        assert_eq!(
            failure.message.unwrap(),
            JsString::from_utf8("native diagnostic extraction failed")
        );
        // The original owned exception remains usable after the diagnostic failure.
        assert_eq!(
            scope
                .export_data(exception, ExportLimits::default())
                .unwrap(),
            JsData::String(JsString::from_utf8("original diagnostic exception"))
        );
        let object = evaluate(scope, "new Error('original')");
        scope.state.raw.record_fault(fault());
        assert_eq!(scope.detach(ScopedError::Exception(object)), fault());
    });
}

#[test]
fn native_error_stack_preserves_invalid_utf16_as_truncation() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        let error = evaluate(
            scope,
            "Object.defineProperty(new Error('message'), 'stack', { value: '\\ud800' })",
        );
        let report = scope.detach(ScopedError::Exception(error));
        assert_eq!(report.message, Some(JsString::from_utf8("message")));
        assert!(report.truncated);
        assert!(report.stack.is_empty());
    });
}

#[test]
fn stack_parser_ignores_malformed_rows_and_bounds_frames() {
    let frames = stack_frames(
        "Error: x\nnot a frame\nat malformed\nat invalid:line\nat anonymous.js:9\nat named (file:///x.js:5:3)\nat zero.js:0:0\nat scheme:part:8\n",
    );
    assert_eq!(frames.len(), 4);
    assert!(frames[0].function.is_none());
    let first = frames[0].location.as_ref().unwrap();
    assert_eq!(first.origin, "anonymous.js");
    assert_eq!(first.line, Some(8));
    assert_eq!(first.column, None);
    assert_eq!(frames[1].function, Some(JsString::from_utf8("named")));
    let named = frames[1].location.as_ref().unwrap();
    assert_eq!((named.line, named.column), (Some(4), Some(2)));
    let zero = frames[2].location.as_ref().unwrap();
    assert_eq!((zero.line, zero.column), (Some(0), Some(0)));
    assert_eq!(frames[3].location.as_ref().unwrap().origin, "scheme:part");
    assert_eq!(
        stack_frames(&"at f (file:///x.js:1:1)\n".repeat(70)).len(),
        65
    );
}

#[test]
fn bounded_native_heap_exhaustion_during_diagnostics_is_fatal() {
    for expression in [
        "'x'.repeat(160000)",
        "new Error('x'.repeat(160000))",
        "Object.defineProperty(new Error('small'), 'stack', { value: 'x'.repeat(160000) })",
    ] {
        live_scope(
            QuickJsNgConfig {
                max_memory_bytes: Some(512 * 1024),
                ..Default::default()
            },
            |scope| {
                let value = evaluate(scope, expression);
                let report = scope.detach(ScopedError::Exception(value));
                assert_eq!(
                    report.class,
                    FailureClass::Engine,
                    "{expression}: {report:?}"
                );
                assert_eq!(report.code, FailureCode::EngineFailure);
                assert_eq!(
                    report.message,
                    Some(JsString::from_utf8("QuickJS native allocation failed"))
                );
            },
        );
    }
}

#[test]
fn detach_rejects_foreign_scoped_exceptions_and_string_budget_keeps_original_live() {
    live_scope(QuickJsNgConfig::default(), |outer| {
        let original = evaluate(outer, "'original buffer'");
        let raw = outer.read(original).unwrap();
        let failure = outer.detach(outer.string(raw.value, 2).unwrap_err());
        assert_eq!(failure.code, FailureCode::Capacity);
        assert_eq!(
            outer.string(raw.value, 100).unwrap(),
            JsString::from_utf8("original buffer")
        );
        live_scope(QuickJsNgConfig::default(), |inner| {
            assert_eq!(
                inner.detach(ScopedError::Exception(original)).code,
                FailureCode::WrongContext
            );
        });
    });
}

#[test]
fn checked_copy_buffers_report_capacity_without_invalid_native_lengths() {
    assert!(copied_buffer::<u16>(4, "units").unwrap().capacity() >= 4);
    assert!(copied_buffer::<u8>(4, "bytes").unwrap().capacity() >= 4);
    for failure in [
        copied_buffer::<u16>(usize::MAX, "units").unwrap_err(),
        copied_buffer::<u8>(usize::MAX, "bytes").unwrap_err(),
    ] {
        assert_eq!(failure.code, FailureCode::Capacity);
    }
}

#[test]
fn checked_exception_insertion_and_job_results_preserve_typed_failures() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        assert_eq!(scope.detach(scoped_exception(Err(fault()))), fault());
        let value = evaluate(scope, "'script job failure'");
        assert_eq!(
            scope.detach(scoped_exception(Ok(value))).class,
            FailureClass::Script
        );
        let source = ScriptSource::new(
            JsString::from_utf8("throw 'pending job failure'"),
            "file:///job.js".into(),
        );
        // SAFETY: This live scope protects the borrowed source buffers. Leaving
        // the real pending exception untaken lets detached_status consume it.
        let native = unsafe {
            let origin = CString::new(source.origin.as_bytes()).unwrap();
            ffi::jbq_eval(
                scope.ptr(),
                source.text.as_units().as_ptr(),
                source.text.as_units().len(),
                origin.as_ptr(),
            )
        };
        // SAFETY: The value is the initialized result of native evaluation above.
        assert_ne!(unsafe { ffi::jbq_is_exception(native) }, 0);
        let failure = scope.detached_status(-1).unwrap_err();
        assert_eq!(failure.class, FailureClass::Script);
        let mut report = CheckpointReport::empty();
        collect_job_result(&mut report, Ok(())).unwrap();
        for _ in 0..67 {
            collect_job_result(&mut report, Err(failure.clone())).unwrap();
        }
        assert_eq!(report.failures.len(), 64);
        assert_eq!(report.suppressed_failures, 3);
        report.suppressed_failures = u64::MAX;
        collect_job_result(&mut report, Err(failure)).unwrap();
        assert_eq!(report.suppressed_failures, u64::MAX);
        assert_eq!(
            collect_job_result(&mut report, Err(fault())).unwrap_err(),
            fault()
        );
        scope.state.raw.record_fault(fault());
        assert_eq!(scope.detached_status(0).unwrap_err(), fault());
    });
}

#[test]
fn deferred_error_releases_both_successful_native_outputs() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        scope.state.raw.record_fault(fault());
        let error = scope.deferred().unwrap_err();
        assert_eq!(scope.detach(error).code, FailureCode::HostPanic);
        // live_scope closes the native runtime, whose live-object assertion
        // detects a leaked resolver pair from a successful native allocation.
    });
}

#[test]
fn diagnostic_stack_frame_limit_reports_omitted_frames() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        let error = scope.source().unwrap().evaluate(&ScriptSource::new(JsString::from_utf8(
            "const e = new Error('deep'); e.stack = Array(65).fill('at f (file:///deep.js:1:2)').join('\\n'); throw e;"
        ), "file:///stack.js".into())).unwrap_err();
        let report = scope.detach(error);
        assert_eq!(report.stack.len(), 64);
        assert!(report.truncated);
    });
}

#[test]
fn native_artifact_serialization_failure_releases_compiled_value_and_returns_engine_fault() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        let source = ScriptSource::new(JsString::from_utf8("6 * 7"), "file:///serialize.js".into());
        let origin = scope.source_origin(&source).unwrap();
        // SAFETY: The active scope owns the context and source/origin buffers.
        let raw = unsafe {
            ffi::jbq_compile(
                scope.ptr(),
                source.text.as_units().as_ptr(),
                source.text.as_units().len(),
                origin.as_ptr(),
            )
        };
        let compiled = scope.owned(raw).unwrap();
        // SAFETY: Legitimate native resource policy: any subsequent allocation
        // must fail; already owned native values remain valid and releasable.
        unsafe { ffi::jbq_set_memory_limit(scope.ptr(), 1) };
        let report = scope.compiled_payload(&compiled, false).unwrap_err();
        assert_eq!(report.class, FailureClass::Engine);
    });
}

#[test]
fn authentic_artifact_native_read_oom_is_fatal_without_deserializing_untrusted_data() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        let source = ScriptSource::new(JsString::from_utf8("6 * 7"), "file:///read.js".into());
        let artifact = scope
            .compiler()
            .unwrap()
            .compile(&source, QuickJsNgEngine::script_compile_options())
            .unwrap();
        // SAFETY: Only a live owner runtime's resource policy changes. The
        // bytecode is exact, compatible output of its own trusted compiler.
        unsafe { ffi::jbq_set_memory_limit(scope.ptr(), 1) };
        let report = scope.detach(scope.execute(&artifact).unwrap_err());
        assert_eq!(report.class, FailureClass::Engine);
    });
}

#[test]
fn native_compiler_checks_payload_budget_in_the_owner_guard_build() {
    let engine = QuickJsNgEngine::new().unwrap();
    let id = engine
        .create_context(ContextLimits {
            export: ExportLimits {
                max_bytes: 1,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    engine
        .with_scope(id, &NoServices, &mut |scope: &dyn JsScope| {
            let source = ScriptSource::new(
                JsString::from_utf8("6 * 7"),
                "file:///small-budget.js".into(),
            );
            let error = scope
                .compiler()
                .unwrap()
                .compile(&source, QuickJsNgEngine::script_compile_options())
                .unwrap_err();
            assert_eq!(error.code, FailureCode::Capacity);
            Ok(())
        })
        .unwrap();
    engine.close_context(id).unwrap();
}

#[test]
fn owner_guard_build_preserves_interrupt_fault_and_rejects_unsupported_compile_policy() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        let mut options = QuickJsNgEngine::script_compile_options();
        options.fingerprint = "unsupported".into();
        assert_eq!(
            scope.compile_policy(&options).unwrap_err().code,
            FailureCode::Unsupported
        );
        scope.state.raw.interrupt.store(true, Ordering::Release);
        let error = scope
            .source()
            .unwrap()
            .evaluate(&ScriptSource::new(
                JsString::from_utf8("while (true) {}"),
                "file:///interrupt.js".into(),
            ))
            .unwrap_err();
        let report = scope.detach(error);
        assert_eq!(
            (report.class, report.code),
            (FailureClass::Engine, FailureCode::EngineFailure)
        );
    });
}

#[test]
fn owner_copy_allocation_failures_release_native_string_and_compiler_buffers() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        let value = evaluate(scope, "'copy buffer'");
        let native = scope.read(value).unwrap();
        let (result, fired) =
            crate::vm::test_allocator::fail_nth(0, || scope.utf16(native.value, 4096, false));
        assert!(fired);
        assert_eq!(
            scope.detach(result.unwrap_err()).code,
            FailureCode::Capacity
        );
        assert_eq!(
            scope.string(native.value, 4096).unwrap(),
            JsString::from_utf8("copy buffer")
        );
        let source =
            ScriptSource::new(JsString::from_utf8("6 * 7"), "file:///copy-owner.js".into());
        let origin = scope.source_origin(&source).unwrap();
        // SAFETY: The active owner scope keeps source buffers and context live.
        let raw = unsafe {
            ffi::jbq_compile(
                scope.ptr(),
                source.text.as_units().as_ptr(),
                source.text.as_units().len(),
                origin.as_ptr(),
            )
        };
        let compiled = scope.owned(raw).unwrap();
        let (result, fired) =
            crate::vm::test_allocator::fail_nth(0, || scope.compiled_payload(&compiled, false));
        assert!(fired);
        // lunarstorm patch (expectation approved by the user, 2026-11-24): with
        // rquickjs-core's `rust-alloc`, QuickJS allocates through the Rust global
        // allocator, so allocation #0 is the native writer's and reports jetbunny's
        // class for a native allocation failure, EngineFailure (`pending_fault`).
        // On the libc allocator #0 is the Rust copy buffer: Capacity.
        let expected = if rquickjs_core::RUST_ALLOC {
            FailureCode::EngineFailure
        } else {
            FailureCode::Capacity
        };
        assert_eq!(result.unwrap_err().code, expected);
        assert!(!scope.compiled_payload(&compiled, false).unwrap().is_empty());
    });
}

/// Fix: the bytecode writer ignored a failed atom-index allocation (13 callers
/// drop `bc_put_atom`'s -1) and never read its buffer's allocation flag, so under
/// memory pressure `compiled_payload` returned a truncated image as written.
/// Every image written under a memory limit must be refused or equal the
/// unlimited one, and some limit in the sweep must make the writer fail.
#[test]
fn compiled_payload_under_a_memory_limit_is_refused_or_complete() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        let names: String = (0..2000).map(|i| format!("o.p{i}=1;")).collect();
        let source = ScriptSource::new(
            JsString::from_utf8(&format!("var o={{}};{names}")),
            "file:///write-limit.js".into(),
        );
        let origin = scope.source_origin(&source).unwrap();
        // SAFETY: The active owner scope keeps source buffers and context live.
        let raw = unsafe {
            ffi::jbq_compile(
                scope.ptr(),
                source.text.as_units().as_ptr(),
                source.text.as_units().len(),
                origin.as_ptr(),
            )
        };
        let compiled = scope.owned(raw).unwrap();
        let reference = scope.compiled_payload(&compiled, false).unwrap();
        let (mut used, mut limit) = (0, 0);
        // SAFETY: owner-local runtime of the live scope; plain out-parameters.
        unsafe { ffi::jbq_heap_usage(scope.ptr(), &mut used, &mut limit) };
        let mut refused = 0;
        for extra in (0..256 * 1024).step_by(64) {
            // SAFETY: owner-local runtime of the live scope.
            unsafe { ffi::jbq_set_memory_limit(scope.ptr(), used as usize + extra) };
            match scope.compiled_payload(&compiled, false) {
                Ok(bytes) => assert!(bytes == reference, "limit +{extra}: a different image was returned as written"),
                Err(_) => refused += 1,
            }
        }
        // SAFETY: owner-local runtime of the live scope; restores the original limit.
        unsafe { ffi::jbq_set_memory_limit(scope.ptr(), limit as usize) };
        assert!(refused > 0, "no limit in the sweep made the writer fail");
    });
}

#[test]
fn owner_source_metadata_and_nondata_diagnostics_are_rejected_without_application_entry() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        let invalid = ScriptSource::new(
            JsString::from_utf8("throw 42"),
            "file:///invalid\0.js".into(),
        );
        assert_eq!(
            scope.source_origin(&invalid).unwrap_err().code,
            FailureCode::InvalidArtifact
        );
        let mut invalid =
            ScriptSource::new(JsString::from_utf8("throw 42"), "file:///invalid.js".into());
        invalid.source_map = Some(SourceMapAttachment {
            json: b"{".to_vec(),
            originals: Vec::new(),
            preceding_maps: Vec::new(),
        });
        assert_eq!(
            scope.source_origin(&invalid).unwrap_err().code,
            FailureCode::InvalidArtifact
        );
        let value = evaluate(
            scope,
            "Object.defineProperty(new Error('data'), 'message', {get(){throw 'must not run';}})",
        );
        let native = scope.read(value).unwrap();
        assert_eq!(scope.diagnostic_field(native.value, 0, 4096).unwrap(), None);
    });
}

#[test]
fn owner_detachment_leaves_missing_or_accessor_only_error_fields_absent() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        for program in [
            "throw {}",
            "throw Object.defineProperties(new Error(), {message:{get(){throw 'message getter';}}, stack:{get(){throw 'stack getter';}}})",
        ] {
            let error = scope
                .source()
                .unwrap()
                .evaluate(&ScriptSource::new(
                    JsString::from_utf8(program),
                    "file:///absent-fields.js".into(),
                ))
                .unwrap_err();
            let report = scope.detach(error);
            assert_eq!(report.class, FailureClass::Script);
            assert!(report.message.is_none());
            assert!(report.stack.is_empty());
        }
    });
}

#[test]
fn native_compiler_syntax_error_detaches_before_the_owner_frame_closes() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        let source = ScriptSource::new(
            JsString::from_utf8("function {"),
            "file:///syntax.js".into(),
        );
        let report = scope
            .compiler()
            .unwrap()
            .compile(&source, QuickJsNgEngine::script_compile_options())
            .unwrap_err();
        assert_eq!(
            (report.class, report.code),
            (FailureClass::Script, FailureCode::ScriptException)
        );
        assert!(report.message.is_some());
    });
}

#[test]
fn numeric_diagnostics_preserve_pending_host_faults() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        let value = evaluate(scope, "42");
        scope.state.raw.record_fault(fault());
        let report = scope.detach(ScopedError::Exception(value));
        assert_eq!(report, fault());
    });
}

#[test]
fn owner_artifact_validation_rejects_another_build_before_native_loading() {
    live_scope(QuickJsNgConfig::default(), |scope| {
        let source = ScriptSource::new(
            JsString::from_utf8("throw 123"),
            "file:///rejected-build.js".into(),
        );
        let original = scope
            .compiler()
            .unwrap()
            .compile(&source, QuickJsNgEngine::script_compile_options())
            .unwrap();
        let mut metadata = original.metadata().clone();
        metadata.build_fingerprint.push_str("-incompatible");
        // SAFETY: Bytes are unchanged output of this trusted compiler. Only
        // compatibility metadata differs, and must be rejected before C loading.
        let incompatible = unsafe {
            CompiledArtifact::from_trusted_compiler_output(
                metadata,
                original.payload().to_vec(),
                source,
            )
        }
        .unwrap();
        let error = scope.execute(&incompatible).unwrap_err();
        assert_eq!(scope.detach(error).code, FailureCode::InvalidArtifact);
    });
}
#[test]
fn native_module_admission_and_lifecycle_guards_use_real_prepared_objects() {
    struct Export;
    impl HostFunction for Export {
        fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
            call.import_data(JsData::Undefined)
        }
    }
    live_scope(QuickJsNgConfig::default(), |s| {
        let key = JsString::from_utf8("guard");
        let text = ScriptSource::new(
            JsString::from_utf8("export const x=1"),
            "file:///guard.js".into(),
        );
        let id = ModuleApi::prepare(s, &text, &key).unwrap();
        assert_eq!(
            ModuleApi::evaluation_state(s, &id).unwrap(),
            ModuleEvaluationState::NotStarted
        );
        let unknown = ModuleId {
            context: s.context(),
            slot: u64::MAX,
        };
        assert_eq!(
            ModuleApi::imports(s, &unknown).unwrap_err().code,
            FailureCode::InvalidHandle
        );
        let foreign = ModuleId {
            context: ValueStore::<u8>::new(
                &EngineIdentity::new().unwrap(),
                ContextLimits::default(),
            )
            .unwrap()
            .context(),
            slot: 0,
        };
        assert_eq!(
            ModuleApi::imports(s, &foreign).unwrap_err().code,
            FailureCode::WrongContext
        );
        let mut mapped = text.clone();
        mapped.source_map = Some(SourceMapAttachment {
            json: b"{}".to_vec(),
            originals: vec![],
            preceding_maps: vec![],
        });
        assert_eq!(
            ModuleApi::prepare(s, &mapped, &key).unwrap_err().code,
            FailureCode::Unsupported
        );
        assert!(ModuleApi::prepare(s, &text, &JsString::from_units(vec![0xd800])).is_err());
        let export = BuiltinExport {
            name: JsString::from_utf8("x"),
            value: Rc::new(HostValueDefinition::Function {
                name: JsString::from_utf8("x"),
                arity: 0,
                function: Rc::new(Export),
            }),
        };
        assert!(ModuleApi::define_builtin(s, &key, &vec![export.clone(); 1025]).is_err());
        assert!(ModuleApi::define_builtin(s, &key, &[export.clone(), export.clone()]).is_err());
        let nul = BuiltinExport {
            name: JsString::from_units(vec![0]),
            ..export.clone()
        };
        assert!(ModuleApi::define_builtin(s, &key, &[nul]).is_err());
        assert!(ModuleApi::define_builtin(s, &JsString::from_units(vec![0xd800]), &[]).is_err());
        let builtin =
            ModuleApi::define_builtin(s, &JsString::from_utf8("host"), &[export]).unwrap();
        assert!(ModuleImageApi::compile(s, &builtin).is_err());
        ModuleApi::link(s, &builtin, &[]).unwrap();
        ModuleApi::evaluate(s, &builtin).unwrap();
        let pending = ModuleApi::prepare(
            s,
            &ScriptSource::new(
                JsString::from_utf8("await new Promise(()=>{})"),
                "file:///pending.js".into(),
            ),
            &JsString::from_utf8("pending"),
        )
        .unwrap();
        ModuleApi::link(s, &pending, &[]).unwrap();
        ModuleApi::evaluate(s, &pending).unwrap();
        assert_eq!(
            ModuleApi::evaluation_state(s, &pending).unwrap(),
            ModuleEvaluationState::Pending
        );
        let rejected = ModuleApi::prepare(
            s,
            &ScriptSource::new(
                JsString::from_utf8("await Promise.reject('rejected')"),
                "file:///reject.js".into(),
            ),
            &JsString::from_utf8("rejected"),
        )
        .unwrap();
        ModuleApi::link(s, &rejected, &[]).unwrap();
        ModuleApi::evaluate(s, &rejected).unwrap();
        s.drain_microtasks(std::num::NonZeroUsize::new(64).unwrap())
            .unwrap();
        assert!(matches!(
            ModuleApi::evaluation_state(s, &rejected).unwrap(),
            ModuleEvaluationState::Rejected(_)
        ));
        assert!(
            ModuleApi::finish_dynamic_import(
                s,
                ModuleImportId {
                    context: foreign.context(),
                    sequence: 1
                },
                Err(FailureReport::fault(FailureCode::InvalidHandle, "test"))
            )
            .is_err()
        );
        assert!(
            ModuleApi::finish_dynamic_import(
                s,
                ModuleImportId {
                    context: s.context(),
                    sequence: 123
                },
                Ok(id)
            )
            .is_err()
        );
        s.state.module_attempts.set(4096);
        assert_eq!(
            ModuleApi::prepare(s, &text, &key).unwrap_err().code,
            FailureCode::Capacity
        );
    });
}
/// Import attributes now reach the native module; Lunarstorm's graph lists the
/// request by specifier and ignores its attributes (the host picks a module's
/// kind from its file) instead of failing the import.
#[test]
fn native_module_request_with_attributes_is_listed_by_specifier() {
    live_scope(QuickJsNgConfig::default(), |s| {
        let text = ScriptSource::new(
            JsString::from_utf8("import d from 'dep' with { type: 'json' };export const x=d;"),
            "file:///attributes.js".into(),
        );
        let id = ModuleApi::prepare(s, &text, &JsString::from_utf8("attributes")).unwrap();
        assert_eq!(
            ModuleApi::imports(s, &id).unwrap(),
            vec![JsString::from_utf8("dep")]
        );
    });
}
