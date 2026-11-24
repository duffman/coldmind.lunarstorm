//! Acceptance against actual QuickJS-NG, not a Rust JavaScript fixture.
#![cfg(feature = "quickjs-ng")]
use coldmind_runtime::{
    debug::DebugRequirements, engine::*, engines::quickjs_ng::QuickJsNgEngine, vm::*,
};

#[test]
fn source_evaluation_runs_actual_javascript_through_the_public_vm() {
    let config = VmConfig {
        requirements: EngineRequirements {
            debug: DebugRequirements::disabled(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    let value = vm
        .evaluate(ScriptSource::new(
            JsString::from_utf8("6 * 7"),
            "file:///native.js".into(),
        ))
        .unwrap();
    vm.execute(Box::new(move |scope: &dyn JsScope| {
        let local = scope.local(&value).map_err(|e| scope.detach(e))?;
        let data = scope
            .export_data(local, ExportLimits::default())
            .map_err(|e| scope.detach(e))?;
        assert_eq!(data, JsData::Number(42.0));
        Ok(())
    }))
    .unwrap();
    vm.shutdown().unwrap();
}

#[test]
fn direct_eval_after_a_class_keeps_later_lexical_bindings_visible() {
    for debug in [DebugRequirements::disabled(), DebugRequirements::default()] {
        let config = VmConfig {
            requirements: EngineRequirements {
                debug,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
        let result = vm
            .evaluate(ScriptSource::new(
                JsString::from_utf8(
                    "(()=>{class Box {#n=40;}let offset=0;eval('offset=2');return offset;})()",
                ),
                "file:///class-before-eval.js".into(),
            ))
            .unwrap();
        vm.execute(Box::new(move |scope: &dyn JsScope| {
            let value = scope.local(&result).map_err(|error| scope.detach(error))?;
            assert_eq!(
                scope
                    .export_data(value, ExportLimits::default())
                    .map_err(|error| scope.detach(error))?,
                JsData::Number(2.0)
            );
            Ok(())
        }))
        .unwrap();
    }
}

#[test]
fn hybrid_default_keeps_full_c_language_and_dynamic_compilation_for_source_and_artifacts() {
    let config = VmConfig {
        requirements: EngineRequirements {
            debug: DebugRequirements::disabled(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut vm = VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap();
    // These deliberately exceed the separately exposed Rust compiler subset.
    let text = r#"(() => {
        class Box { #n=40; value({add=2}={}) {return this.#n+add;} }
        function* values(...args) {const [n]=args;yield n;}
        let value=values(new Box().value()).next().value;
        let offset=0;eval('offset=2');
        const subtract=new Function('n','return n-2');
        try {
            if(!/^\d+$/.test(String(value)))throw new Error('regexp mismatch');
            return subtract(value+offset);
        } finally {globalThis.hybridRuns=(globalThis.hybridRuns||0)+1;}
    })()"#;
    let source = || {
        ScriptSource::new(
            JsString::from_utf8(text),
            "file:///hybrid-default.js".into(),
        )
    };
    fn check(vm: &mut VirtualMachine<QuickJsNgEngine>, value: RootedValue, expected: f64) {
        vm.execute(Box::new(move |scope: &dyn JsScope| {
            let local = scope.local(&value).map_err(|e| scope.detach(e))?;
            let data = scope
                .export_data(local, ExportLimits::default())
                .map_err(|e| scope.detach(e))?;
            assert_eq!(data, JsData::Number(expected));
            Ok(())
        }))
        .unwrap();
    }
    let result = vm.evaluate(source()).unwrap();
    check(&mut vm, result, 42.0);
    let artifact = vm
        .compile(source(), QuickJsNgEngine::script_compile_options())
        .unwrap();
    let count = vm
        .evaluate(ScriptSource::new(
            JsString::from_utf8("hybridRuns"),
            "file:///count.js".into(),
        ))
        .unwrap();
    check(&mut vm, count, 1.0);
    let result = vm.execute_artifact(artifact).unwrap();
    check(&mut vm, result, 42.0);
    let count = vm
        .evaluate(ScriptSource::new(
            JsString::from_utf8("hybridRuns"),
            "file:///count.js".into(),
        ))
        .unwrap();
    check(&mut vm, count, 2.0);
    vm.shutdown().unwrap();
}
