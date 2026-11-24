//! Real native copy-out operations preserve cleanup when the Rust allocator fails.
#![cfg(feature = "quickjs-ng")]
use coldmind_runtime::{engine::*, engines::quickjs_ng::QuickJsNgEngine};
#[path = "support/copy_allocator.rs"]
mod allocator;
mod support;

#[test]
fn native_string_copy_failure_preserves_the_value_and_releases_native_buffer() {
    let engine = QuickJsNgEngine::new().unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    engine
        .with_scope(id, &support::NoLoop, &mut |scope: &dyn JsScope| {
            let expected = JsData::String(JsString::from_utf8("native copy"));
            let value = scope
                .import_data(expected.clone())
                .map_err(|error| scope.detach(error))?;
            let (result, fired) =
                allocator::fail_size(22, || scope.export_data(value, ExportLimits::default()));
            assert!(fired, "the actual Rust UTF-16 copy must request storage");
            let report = scope.detach(result.unwrap_err());
            assert_eq!(report.code, FailureCode::Capacity);
            assert_eq!(
                report.message,
                Some(JsString::from_utf8("string copy allocation failed"))
            );
            assert_eq!(
                scope
                    .export_data(value, ExportLimits::default())
                    .map_err(|error| scope.detach(error))?,
                expected
            );
            Ok(())
        })
        .unwrap();
    engine.close_context(id).unwrap();
}

#[test]
fn native_artifact_copy_failure_releases_compiler_storage_and_allows_later_compilation() {
    let engine = QuickJsNgEngine::new().unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    engine
        .with_scope(id, &support::NoLoop, &mut |scope: &dyn JsScope| {
            let source =
                ScriptSource::new(JsString::from_utf8("6 * 7"), "file:///allocation.js".into());
            let options = QuickJsNgEngine::script_compile_options();
            let previous = scope
                .compiler()
                .unwrap()
                .compile(&source, QuickJsNgEngine::script_compile_options())?;
            let bytes = previous.payload().len();
            drop(previous);
            // Fail the exact payload buffer, independent of earlier bookkeeping allocations.
            let (result, fired) = allocator::fail_size(bytes, || {
                scope.compiler().unwrap().compile(&source, options)
            });
            assert!(fired);
            let report = result.unwrap_err();
            assert_eq!(report.code, FailureCode::Capacity);
            assert_eq!(
                report.message,
                Some(JsString::from_utf8("artifact copy allocation failed"))
            );
            let artifact = scope
                .compiler()
                .unwrap()
                .compile(&source, QuickJsNgEngine::script_compile_options())?;
            let value = scope
                .artifacts()
                .unwrap()
                .execute(&artifact)
                .map_err(|error| scope.detach(error))?;
            assert_eq!(
                scope
                    .export_data(value, ExportLimits::default())
                    .map_err(|error| scope.detach(error))?,
                JsData::Number(42.)
            );
            Ok(())
        })
        .unwrap();
    engine.close_context(id).unwrap();
}

#[test]
fn diagnostic_copy_allocation_failure_is_reported_without_masking_the_fault() {
    let engine = QuickJsNgEngine::new().unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    engine
        .with_scope(id, &support::NoLoop, &mut |scope: &dyn JsScope| {
            for program in ["throw 'diagnostic'", "throw new Error('diagnostic')"] {
                let source = ScriptSource::new(
                    JsString::from_utf8(program),
                    "file:///diagnostic-copy.js".into(),
                );
                let exception = scope.source().unwrap().evaluate(&source).unwrap_err();
                let (report, fired) = allocator::fail_size(20, || scope.detach(exception));
                assert!(fired);
                assert_eq!(
                    (report.class, report.code),
                    (FailureClass::Contract, FailureCode::Capacity)
                );
                assert_eq!(
                    report.message,
                    Some(JsString::from_utf8("string copy allocation failed"))
                );
            }
            Ok(())
        })
        .unwrap();
    engine.close_context(id).unwrap();
}
