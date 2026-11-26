//! Trusted native compiler output, exact compatibility gates, and source admission.
#![cfg(feature = "quickjs-ng")]

use coldmind_runtime::{engine::*, engines::quickjs_ng::QuickJsNgEngine};
mod support;

fn source(program: &str) -> ScriptSource {
    ScriptSource::new(
        JsString::from_utf8(program),
        "file:///quickjs-ng-artifacts.js".into(),
    )
}

fn run(
    engine: &QuickJsNgEngine,
    context: ContextId,
    mut body: impl FnMut(&dyn JsScope) -> Result<(), FailureReport>,
) {
    engine
        .with_scope(context, &support::NoLoop, &mut body)
        .unwrap();
}

fn copied(scope: &dyn JsScope, program: &str) -> Result<JsData, FailureReport> {
    let value = scope
        .source()
        .unwrap()
        .evaluate(&source(program))
        .map_err(|error| scope.detach(error))?;
    scope
        .export_data(value, ExportLimits::default())
        .map_err(|error| scope.detach(error))
}

fn assert_fault(error: FailureReport, code: FailureCode, message: &str) {
    assert_eq!(error.class, FailureClass::Contract);
    assert_eq!(error.code, code);
    assert_eq!(error.message, Some(JsString::from_utf8(message)));
}

/// Every payload remains authentic, unmodified output from the current compiler.
/// Deliberately inconsistent metadata is used only to exercise rejection gates.
fn with_metadata(
    original: &CompiledArtifact,
    metadata: ArtifactMetadata,
    source: ScriptSource,
) -> Result<CompiledArtifact, FailureReport> {
    // SAFETY: The payload is an unmodified copy of this trusted native compiler's
    // output. Tests deliberately change only metadata/source; each incompatible
    // artifact must be rejected before native loading, and no hostile bytes exist.
    unsafe {
        CompiledArtifact::from_trusted_compiler_output(
            metadata,
            original.payload().to_vec(),
            source,
        )
    }
}

#[test]
fn compilation_executes_nothing_and_artifacts_execute_in_the_loading_context() {
    let engine = QuickJsNgEngine::new().unwrap();
    let first = engine.create_context(ContextLimits::default()).unwrap();
    let second = engine.create_context(ContextLimits::default()).unwrap();
    let mut artifact = None;
    run(&engine, first, |scope| {
        assert_eq!(
            copied(scope, "globalThis.counter=0;counter")?,
            JsData::Number(0.0)
        );
        let input = source("++counter;counter");
        let output = scope
            .compiler()
            .unwrap()
            .compile(&input, QuickJsNgEngine::script_compile_options())?;
        assert!(!output.payload().is_empty());
        assert_eq!(output.source().text, input.text);
        assert_eq!(output.source().origin, input.origin);
        assert_eq!(
            output.metadata().source_digest,
            SourceDigest::of_utf16(&input.text)
        );
        assert_eq!(output.metadata().adapter_id, engine.descriptor().adapter_id);
        assert_eq!(
            output.metadata().native_version,
            engine.descriptor().native_version
        );
        assert_eq!(
            output.metadata().target_fingerprint,
            engine.descriptor().target_fingerprint
        );
        assert_eq!(
            output.metadata().build_fingerprint,
            engine.descriptor().build_fingerprint
        );
        assert_eq!(output.metadata().format_version, 1);
        assert_eq!(
            output.metadata().options,
            QuickJsNgEngine::script_compile_options()
        );
        assert_eq!(copied(scope, "counter")?, JsData::Number(0.0));
        for expected in [1.0, 2.0] {
            let value = scope
                .artifacts()
                .unwrap()
                .execute(&output)
                .map_err(|error| scope.detach(error))?;
            assert_eq!(
                scope
                    .export_data(value, ExportLimits::default())
                    .map_err(|error| scope.detach(error))?,
                JsData::Number(expected)
            );
        }
        artifact = Some(output);
        Ok(())
    });
    engine.close_context(first).unwrap();
    run(&engine, second, |scope| {
        assert_eq!(
            copied(scope, "globalThis.counter=40;counter")?,
            JsData::Number(40.0)
        );
        let value = scope
            .artifacts()
            .unwrap()
            .execute(artifact.as_ref().unwrap())
            .map_err(|error| scope.detach(error))?;
        assert_eq!(
            scope
                .export_data(value, ExportLimits::default())
                .map_err(|error| scope.detach(error))?,
            JsData::Number(41.0)
        );
        Ok(())
    });
    engine.close_context(second).unwrap();
}

#[test]
fn every_native_compatibility_field_is_rejected_before_deserialization() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    run(&engine, context, |scope| {
        copied(scope, "globalThis.executions=0")?;
        let original = scope.compiler().unwrap().compile(
            &source("++executions"),
            QuickJsNgEngine::script_compile_options(),
        )?;
        for field in 0..5 {
            let mut metadata = original.metadata().clone();
            match field {
                0 => metadata.adapter_id.push_str("-other"),
                1 => metadata.native_version.push_str("-other"),
                2 => metadata.format_version += 1,
                3 => metadata.target_fingerprint.push_str("-other"),
                4 => metadata.build_fingerprint.push_str("-other"),
                _ => unreachable!(),
            }
            let mismatched = with_metadata(&original, metadata, original.source().clone())?;
            let error = scope.artifacts().unwrap().execute(&mismatched).unwrap_err();
            let ScopedError::Fault(error) = error else {
                panic!("compatibility failure must precede native execution")
            };
            assert_fault(error, FailureCode::InvalidArtifact, "incompatible artifact");
            assert_eq!(copied(scope, "executions")?, JsData::Number(0.0));
        }
        let result = scope
            .artifacts()
            .unwrap()
            .execute(&original)
            .map_err(|error| scope.detach(error))?;
        assert_eq!(
            scope
                .export_data(result, ExportLimits::default())
                .map_err(|error| scope.detach(error))?,
            JsData::Number(1.0)
        );
        Ok(())
    });
    engine.close_context(context).unwrap();
}

#[test]
fn source_digest_and_source_body_mismatches_fail_before_an_artifact_is_created() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    run(&engine, context, |scope| {
        let original = scope
            .compiler()
            .unwrap()
            .compile(&source("42"), QuickJsNgEngine::script_compile_options())?;
        let mut metadata = original.metadata().clone();
        metadata.source_digest = SourceDigest::of_utf16(&JsString::from_utf8("43"));
        assert_fault(
            with_metadata(&original, metadata, original.source().clone()).unwrap_err(),
            FailureCode::InvalidArtifact,
            "source digest mismatch",
        );
        let changed_source = source("43");
        assert_fault(
            with_metadata(&original, original.metadata().clone(), changed_source).unwrap_err(),
            FailureCode::InvalidArtifact,
            "source digest mismatch",
        );
        Ok(())
    });
    engine.close_context(context).unwrap();
}

#[test]
fn unsupported_compile_and_artifact_policies_are_rejected_before_entry() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    run(&engine, context, |scope| {
        copied(scope, "globalThis.executions=0")?;
        let input = source("++executions");
        let original = scope
            .compiler()
            .unwrap()
            .compile(&input, QuickJsNgEngine::script_compile_options())?;
        for field in 0..3 {
            let mut options = QuickJsNgEngine::script_compile_options();
            match field {
                0 => options.mode = CodeMode::Module,
                1 => options.debug_info = DebugInfo::Required,
                2 => options.fingerprint.push_str("-other"),
                _ => unreachable!(),
            }
            assert_fault(
                scope
                    .compiler()
                    .unwrap()
                    .compile(&input, options.clone())
                    .unwrap_err(),
                FailureCode::Unsupported,
                "QuickJS supports Script/Strip/default compiler policy",
            );
            let mut metadata = original.metadata().clone();
            metadata.options = options;
            let artifact = with_metadata(&original, metadata, input.clone())?;
            let Err(ScopedError::Fault(error)) = scope.artifacts().unwrap().execute(&artifact)
            else {
                panic!("unsupported policy must fail before deserialization")
            };
            assert_fault(
                error,
                FailureCode::Unsupported,
                "QuickJS supports Script/Strip/default compiler policy",
            );
            assert_eq!(copied(scope, "executions")?, JsData::Number(0.0));
        }
        Ok(())
    });
    engine.close_context(context).unwrap();
}

#[test]
fn malformed_maps_and_nul_origins_fail_source_compile_and_artifact_admission() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    run(&engine, context, |scope| {
        copied(scope, "globalThis.executions=0")?;
        let input = source("++executions");
        let original = scope
            .compiler()
            .unwrap()
            .compile(&input, QuickJsNgEngine::script_compile_options())?;
        for malformed in 0..2 {
            let mut invalid = input.clone();
            let message = if malformed == 0 {
                invalid.origin.push('\0');
                "source origin contains NUL"
            } else {
                invalid.source_map = Some(SourceMapAttachment {
                    json: b"{broken".to_vec(),
                    originals: Vec::new(),
                    preceding_maps: Vec::new(),
                });
                "invalid source metadata"
            };
            let Err(ScopedError::Fault(error)) = scope.source().unwrap().evaluate(&invalid) else {
                panic!("source metadata must fail before evaluation")
            };
            assert_fault(error, FailureCode::InvalidArtifact, message);
            assert_fault(
                scope
                    .compiler()
                    .unwrap()
                    .compile(&invalid, QuickJsNgEngine::script_compile_options())
                    .unwrap_err(),
                FailureCode::InvalidArtifact,
                message,
            );
            let artifact = with_metadata(&original, original.metadata().clone(), invalid)?;
            let Err(ScopedError::Fault(error)) = scope.artifacts().unwrap().execute(&artifact)
            else {
                panic!("source metadata must fail before native loading")
            };
            assert_fault(error, FailureCode::InvalidArtifact, message);
            assert_eq!(copied(scope, "executions")?, JsData::Number(0.0));
        }
        Ok(())
    });
    engine.close_context(context).unwrap();
}

#[test]
fn native_syntax_errors_are_script_failures_and_do_not_execute_a_prefix() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    run(&engine, context, |scope| {
        copied(scope, "globalThis.executions=0")?;
        let invalid = source("++executions; function (");
        let error = scope
            .compiler()
            .unwrap()
            .compile(&invalid, QuickJsNgEngine::script_compile_options())
            .unwrap_err();
        assert_eq!(error.class, FailureClass::Script);
        assert_eq!(error.code, FailureCode::ScriptException);
        let Err(error @ ScopedError::Exception(_)) = scope.source().unwrap().evaluate(&invalid)
        else {
            panic!("syntax error must be an original native exception")
        };
        let error = scope.detach(error);
        assert_eq!(error.class, FailureClass::Script);
        assert_eq!(error.code, FailureCode::ScriptException);
        assert_eq!(copied(scope, "executions")?, JsData::Number(0.0));
        Ok(())
    });
    engine.close_context(context).unwrap();
}

#[test]
fn compiled_payload_copy_honors_the_exact_context_byte_budget() {
    let engine = QuickJsNgEngine::new().unwrap();
    let input = source("globalThis.executed=true;42");
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let mut length = 0;
    run(&engine, context, |scope| {
        length = scope
            .compiler()
            .unwrap()
            .compile(&input, QuickJsNgEngine::script_compile_options())?
            .payload()
            .len();
        Ok(())
    });
    engine.close_context(context).unwrap();
    assert!(length > 1);
    for limit in [length, length - 1] {
        let context = engine
            .create_context(ContextLimits {
                export: ExportLimits {
                    max_bytes: limit,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        run(&engine, context, |scope| {
            let result = scope
                .compiler()
                .unwrap()
                .compile(&input, QuickJsNgEngine::script_compile_options());
            if limit == length {
                assert_eq!(result?.payload().len(), length);
            } else {
                assert_fault(
                    result.unwrap_err(),
                    FailureCode::Capacity,
                    "compiled artifact exceeds copied payload limit",
                );
            }
            assert_eq!(
                copied(scope, "typeof executed")?,
                JsData::String(JsString::from_utf8("undefined"))
            );
            Ok(())
        });
        engine.close_context(context).unwrap();
    }
}

#[test]
fn lossless_utf16_source_and_valid_map_attachments_survive_compilation() {
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    run(&engine, context, |scope| {
        let expected = JsString::from_units(vec![0xd800, 0, 0xdc00, 0xd83d, 0xde00]);
        let mut units = vec![u16::from(b'\'')];
        units.extend_from_slice(expected.as_units());
        units.push(u16::from(b'\''));
        let mut input = ScriptSource::new(
            JsString::from_units(units),
            "file:///quickjs-ng-artifacts.js".into(),
        );
        input.source_map = Some(SourceMapAttachment {
            json: br#"{"version":3,"file":"quickjs-ng-artifacts.js","sources":[],"names":[],"mappings":""}"#.to_vec(),
            originals: Vec::new(), preceding_maps: Vec::new(),
        });
        let artifact = scope
            .compiler()
            .unwrap()
            .compile(&input, QuickJsNgEngine::script_compile_options())?;
        assert_eq!(artifact.source().text, input.text);
        assert_eq!(
            artifact.source().source_map.as_ref().unwrap().json,
            input.source_map.as_ref().unwrap().json
        );
        assert_eq!(
            artifact.metadata().source_digest,
            SourceDigest::of_utf16(&input.text)
        );
        for value in [
            scope.source().unwrap().evaluate(&input),
            scope.artifacts().unwrap().execute(&artifact),
        ] {
            let value = value.map_err(|error| scope.detach(error))?;
            assert_eq!(
                scope
                    .export_data(value, ExportLimits::default())
                    .map_err(|error| scope.detach(error))?,
                JsData::String(expected.clone())
            );
        }
        Ok(())
    });
    engine.close_context(context).unwrap();
}
