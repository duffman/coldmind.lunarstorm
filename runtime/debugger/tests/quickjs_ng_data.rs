//! Copied-data acceptance against the actual native QuickJS-NG heap.
#![cfg(feature = "quickjs-ng")]

use coldmind_runtime::{
    debug::DebugRequirements, engine::*, engines::quickjs_ng::QuickJsNgEngine, vm::*,
};
use std::mem::size_of;

fn vm(limits: ExportLimits) -> VirtualMachine<QuickJsNgEngine> {
    VirtualMachine::new(
        QuickJsNgEngine::new().unwrap(),
        VmConfig {
            requirements: EngineRequirements {
                debug: DebugRequirements::disabled(),
                ..Default::default()
            },
            context: ContextLimits {
                export: limits,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap()
}

fn source(program: &str) -> ScriptSource {
    ScriptSource::new(
        JsString::from_utf8(program),
        "file:///quickjs-ng-data.js".into(),
    )
}

fn evaluate<'s>(scope: &'s dyn JsScope, program: &str) -> JsResult<'s, ScopedValue<'s>> {
    scope.source().unwrap().evaluate(&source(program))
}

fn copied(scope: &dyn JsScope, program: &str) -> Result<JsData, FailureReport> {
    let value = evaluate(scope, program).map_err(|error| scope.detach(error))?;
    scope
        .export_data(value, ExportLimits::default())
        .map_err(|error| scope.detach(error))
}

fn fault<T: std::fmt::Debug>(result: JsResult<'_, T>, code: FailureCode) {
    let Err(ScopedError::Fault(report)) = result else {
        panic!("expected {code:?}, got {result:?}")
    };
    assert_eq!(report.code, code);
    assert_eq!(report.class, FailureClass::Contract);
}

#[test]
fn pristine_import_preserves_utf16_null_prototypes_and_own_data_descriptors() {
    let mut vm = vm(ExportLimits::default());
    vm.execute(Box::new(|scope: &dyn JsScope| {
        evaluate(scope, "globalThis.NativeObject=Object; globalThis.NativeBytes=Uint8Array; Object=Array=Uint8Array=function(){throw 'replaced constructor'};")
            .map_err(|error| scope.detach(error))?;
        let key = JsString::from_units(vec![0xd800, 0, 0xdc00]);
        let text = JsString::from_units(vec![0xdc00, 0, 0xd800, 0xd83d, 0xde00]);
        let data = JsData::Record(vec![
            (JsString::from_utf8("__proto__"), JsData::Number(7.0)),
            (key.clone(), JsData::String(text.clone())),
            (JsString::from_utf8(""), JsData::Array(vec![JsData::Undefined])),
            (JsString::from_utf8("bytes"), JsData::Bytes(vec![0, 128, 255])),
        ]);
        let native = scope.import_data(data.clone()).map_err(|error| scope.detach(error))?;
        assert_eq!(scope.export_data(native, ExportLimits::default()).map_err(|error| scope.detach(error))?, data);
        let global = scope.global().map_err(|error| scope.detach(error))?;
        scope.set(global, PropertyKey::String(&JsString::from_utf8("payload")), native).map_err(|error| scope.detach(error))?;
        assert_eq!(copied(scope, "[NativeObject.getPrototypeOf(payload)===null, NativeObject.hasOwn(payload,'__proto__'), payload.__proto__, payload.bytes instanceof NativeBytes, NativeObject.getOwnPropertyDescriptor(payload,'__proto__').writable, NativeObject.getOwnPropertyDescriptor(payload,'__proto__').enumerable, NativeObject.getOwnPropertyDescriptor(payload,'__proto__').configurable]")?,
            JsData::Array(vec![JsData::Bool(true), JsData::Bool(true), JsData::Number(7.0), JsData::Bool(true), JsData::Bool(true), JsData::Bool(true), JsData::Bool(true)]));
        assert!(key.to_utf8().is_err());
        assert!(text.to_utf8().is_err());
        Ok(())
    })).unwrap();
    vm.shutdown().unwrap();
}

#[test]
fn export_retains_native_key_order_and_calls_getters_in_that_order() {
    let mut vm = vm(ExportLimits::default());
    vm.execute(Box::new(|scope: &dyn JsScope| {
        let data = copied(scope, "globalThis.trace=[]; const o={}; for(const key of ['z','2','1','a']) Object.defineProperty(o,key,{enumerable:true,get(){trace.push(key);return key;}}); Object.defineProperty(o,'hidden',{get(){throw 'hidden getter ran'}});o")?;
        let keys: Vec<_> = ["1", "2", "z", "a"].map(JsString::from_utf8).into();
        assert_eq!(data, JsData::Record(keys.iter().map(|key| (key.clone(), JsData::String(key.clone()))).collect()));
        assert_eq!(copied(scope, "trace")?, JsData::Array(keys.into_iter().map(JsData::String).collect()));
        Ok(())
    })).unwrap();
}

#[test]
fn getter_exception_retains_original_identity_and_stops_before_later_getters() {
    let mut vm = vm(ExportLimits::default());
    vm.execute(Box::new(|scope: &dyn JsScope| {
        let object = evaluate(scope, "globalThis.trace=[]; globalThis.sentinel={}; ({get first(){trace.push('first');return 1},get second(){trace.push('second');throw sentinel},get third(){trace.push('third');return 3}})").map_err(|error| scope.detach(error))?;
        let Err(ScopedError::Exception(thrown)) = scope.export_data(object, ExportLimits::default()) else { panic!("getter must throw its original native value") };
        let same = evaluate(scope, "(value => value === sentinel)").map_err(|error| scope.detach(error))?;
        let identity = scope.call(same, None, &[thrown]).map_err(|error| scope.detach(error))?;
        assert_eq!(scope.export_data(identity, ExportLimits::default()).map_err(|error| scope.detach(error))?, JsData::Bool(true));
        assert_eq!(copied(scope, "trace")?, JsData::Array(vec![JsData::String(JsString::from_utf8("first")), JsData::String(JsString::from_utf8("second"))]));
        Ok(())
    })).unwrap();
}

#[test]
fn unsupported_values_holes_symbols_proxies_and_exotics_are_rejected() {
    let mut vm = vm(ExportLimits::default());
    vm.execute(Box::new(|scope: &dyn JsScope| {
        for program in [
            "(()=>{})",
            "Symbol('x')",
            "1n",
            "[,undefined]",
            "({[Symbol('x')]: 1})",
            "Object.defineProperty({},Symbol('hidden'),{value:1})",
            "new Date(0)",
            "new Map()",
            "new Uint16Array([1])",
            "Object.create({x:1})",
            "new Proxy({}, {ownKeys(){throw 'proxy trap ran'}})",
            "new Proxy([], {get(){throw 'proxy trap ran'}})",
            "new SharedArrayBuffer(2)",
            "new Uint8Array(new SharedArrayBuffer(2))",
        ] {
            let value = evaluate(scope, program).map_err(|error| scope.detach(error))?;
            fault(
                scope.export_data(value, ExportLimits::default()),
                FailureCode::UnsupportedConversion,
            );
        }
        assert_eq!(
            copied(scope, "[undefined]")?,
            JsData::Array(vec![JsData::Undefined])
        );
        Ok(())
    }))
    .unwrap();
}

#[test]
fn shared_acyclic_children_are_copied_and_current_path_cycles_are_rejected() {
    let mut vm = vm(ExportLimits::default());
    vm.execute(Box::new(|scope: &dyn JsScope| {
        let child = JsData::Record(vec![(JsString::from_utf8("x"), JsData::Number(1.0))]);
        assert_eq!(
            copied(scope, "(()=>{const child={x:1};return [child,child]})()")?,
            JsData::Array(vec![child.clone(), child])
        );
        for program in [
            "(()=>{const a={};a.self=a;return a})()",
            "(()=>{const a=[];a.push({back:a});return a})()",
        ] {
            let value = evaluate(scope, program).map_err(|error| scope.detach(error))?;
            fault(
                scope.export_data(value, ExportLimits::default()),
                FailureCode::UnsupportedConversion,
            );
        }
        Ok(())
    }))
    .unwrap();
}

#[test]
fn ordinary_byte_buffers_views_and_empty_storage_copy_exactly() {
    let mut vm = vm(ExportLimits::default());
    vm.execute(Box::new(|scope: &dyn JsScope| {
        assert_eq!(
            copied(scope, "new Uint8Array([1,2,3,4]).buffer")?,
            JsData::Bytes(vec![1, 2, 3, 4])
        );
        assert_eq!(
            copied(
                scope,
                "new Uint8Array(new Uint8Array([1,2,3,4]).buffer,1,2)"
            )?,
            JsData::Bytes(vec![2, 3])
        );
        assert_eq!(
            copied(scope, "new Uint8Array(new ArrayBuffer(3),3,0)")?,
            JsData::Bytes(vec![])
        );
        assert_eq!(copied(scope, "new ArrayBuffer(0)")?, JsData::Bytes(vec![]));
        for program in [
            "(()=>{let a=new ArrayBuffer(2);a.transfer();return a})()",
            "(()=>{let a=new ArrayBuffer(2),v=new Uint8Array(a);a.transfer();return v})()",
        ] {
            let native = evaluate(scope, program).map_err(|error| scope.detach(error))?;
            let Err(ScopedError::Exception(_)) = scope.export_data(native, ExportLimits::default())
            else {
                panic!("detached storage must raise its native conversion exception")
            };
        }
        Ok(())
    }))
    .unwrap();
}

#[test]
fn primitive_numbers_preserve_negative_zero_nonfinite_values_and_nan() {
    let mut vm = vm(ExportLimits::default());
    vm.execute(Box::new(|scope: &dyn JsScope| {
        for (program, expected) in [
            ("-0", -0.0f64),
            ("Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
            ("NaN", f64::NAN),
        ] {
            let JsData::Number(number) = copied(scope, program)? else {
                panic!("number")
            };
            if expected.is_nan() {
                assert!(number.is_nan());
            } else {
                assert_eq!(number.to_bits(), expected.to_bits());
            }
            let native = scope
                .import_data(JsData::Number(expected))
                .map_err(|error| scope.detach(error))?;
            let JsData::Number(roundtrip) = scope
                .export_data(native, ExportLimits::default())
                .map_err(|error| scope.detach(error))?
            else {
                panic!("roundtrip number")
            };
            if expected.is_nan() {
                assert!(roundtrip.is_nan());
            } else {
                assert_eq!(roundtrip.to_bits(), expected.to_bits());
            }
        }
        assert_eq!(
            copied(scope, "[undefined,null,true,false,'']")?,
            JsData::Array(vec![
                JsData::Undefined,
                JsData::Null,
                JsData::Bool(true),
                JsData::Bool(false),
                JsData::String(JsString::from_utf8(""))
            ])
        );
        Ok(())
    }))
    .unwrap();
}

#[test]
fn export_enforces_exact_depth_node_and_owned_storage_boundaries() {
    let mut vm = vm(ExportLimits::default());
    vm.execute(Box::new(|scope: &dyn JsScope| {
        let value = evaluate(scope, "({x:['A',new Uint8Array([1,2])]})")
            .map_err(|error| scope.detach(error))?;
        let expected = scope
            .export_data(value, ExportLimits::default())
            .map_err(|error| scope.detach(error))?;
        let bytes = expected.footprint(ExportLimits::default())?;
        let exact = ExportLimits {
            max_depth: 3,
            max_nodes: 4,
            max_bytes: bytes,
        };
        assert_eq!(
            scope
                .export_data(value, exact)
                .map_err(|error| scope.detach(error))?,
            expected
        );
        for limits in [
            ExportLimits {
                max_depth: 2,
                ..exact
            },
            ExportLimits {
                max_nodes: 3,
                ..exact
            },
            ExportLimits {
                max_bytes: bytes - 1,
                ..exact
            },
            ExportLimits {
                max_depth: 0,
                ..exact
            },
            ExportLimits {
                max_nodes: 0,
                ..exact
            },
            ExportLimits {
                max_bytes: size_of::<JsData>() - 1,
                ..exact
            },
        ] {
            fault(scope.export_data(value, limits), FailureCode::Capacity);
        }
        assert_eq!(
            scope
                .export_data(value, exact)
                .map_err(|error| scope.detach(error))?,
            expected,
            "failure releases temporaries without damaging the protected input"
        );
        Ok(())
    }))
    .unwrap();
}

#[test]
fn import_enforces_context_depth_nodes_and_backing_capacities_before_conversion() {
    let limits = ExportLimits {
        max_depth: 2,
        max_nodes: 2,
        max_bytes: 128,
    };
    let mut vm = vm(limits);
    vm.execute(Box::new(move |scope: &dyn JsScope| {
        for data in [
            JsData::Array(vec![JsData::Array(vec![JsData::Null])]),
            JsData::Array(vec![JsData::Null, JsData::Null]),
        ] {
            fault(scope.import_data(data), FailureCode::Capacity);
        }
        let mut bytes = Vec::with_capacity(256);
        bytes.push(1);
        fault(
            scope.import_data(JsData::Bytes(bytes)),
            FailureCode::Capacity,
        );
        let mut text = Vec::with_capacity(256);
        text.push(65);
        fault(
            scope.import_data(JsData::String(JsString::from_units(text))),
            FailureCode::Capacity,
        );
        let values = Vec::with_capacity(10);
        fault(
            scope.import_data(JsData::Array(values)),
            FailureCode::Capacity,
        );
        let entries = Vec::with_capacity(10);
        fault(
            scope.import_data(JsData::Record(entries)),
            FailureCode::Capacity,
        );
        let mut key = Vec::with_capacity(256);
        key.push(65);
        fault(
            scope.import_data(JsData::Record(vec![(
                JsString::from_units(key),
                JsData::Null,
            )])),
            FailureCode::Capacity,
        );
        let accepted = JsData::Array(vec![JsData::Null]);
        let value = scope
            .import_data(accepted.clone())
            .map_err(|error| scope.detach(error))?;
        assert_eq!(
            scope
                .export_data(value, limits)
                .map_err(|error| scope.detach(error))?,
            accepted
        );
        Ok(())
    }))
    .unwrap();
}

#[test]
fn native_lengths_and_utf16_storage_are_bounded_before_large_rust_allocations() {
    let mut vm = vm(ExportLimits::default());
    vm.execute(Box::new(|scope: &dyn JsScope| {
        let root_bytes = size_of::<JsData>();
        let limits = ExportLimits {
            max_depth: usize::MAX,
            max_nodes: usize::MAX,
            max_bytes: root_bytes,
        };
        for program in [
            "Array(4294967295)",
            "'abc'",
            "new Uint8Array(1024)",
            "({a:1})",
        ] {
            let native = evaluate(scope, program).map_err(|error| scope.detach(error))?;
            fault(scope.export_data(native, limits), FailureCode::Capacity);
        }
        let string = evaluate(scope, "'\\ud800\\0\\udc00'").map_err(|error| scope.detach(error))?;
        let exact = ExportLimits {
            max_bytes: root_bytes + 6,
            ..limits
        };
        assert_eq!(
            scope
                .export_data(string, exact)
                .map_err(|error| scope.detach(error))?,
            JsData::String(JsString::from_units(vec![0xd800, 0, 0xdc00]))
        );
        fault(
            scope.export_data(
                string,
                ExportLimits {
                    max_bytes: root_bytes + 5,
                    ..limits
                },
            ),
            FailureCode::Capacity,
        );
        Ok(())
    }))
    .unwrap();
}

#[test]
fn deep_import_export_and_invalid_input_cleanup_use_iterative_walks() {
    const DEPTH: usize = 2048;
    let limits = ExportLimits {
        max_depth: DEPTH + 1,
        max_nodes: DEPTH + 1,
        max_bytes: 2 * 1024 * 1024,
    };
    let mut vm = vm(limits);
    vm.execute(Box::new(move |scope: &dyn JsScope| {
        let mut data = JsData::Null;
        for _ in 0..DEPTH {
            data = JsData::Array(vec![data]);
        }
        let native = scope
            .import_data(data)
            .map_err(|error| scope.detach(error))?;
        let mut output = scope
            .export_data(native, limits)
            .map_err(|error| scope.detach(error))?;
        let mut depth = 0;
        while let JsData::Array(children) = &mut output {
            assert_eq!(children.len(), 1);
            output = children.pop().unwrap();
            depth += 1;
        }
        assert_eq!(depth, DEPTH);
        assert_eq!(output, JsData::Null);
        let mut invalid = JsData::Null;
        for _ in 0..DEPTH + 1 {
            invalid = JsData::Array(vec![invalid]);
        }
        fault(scope.import_data(invalid), FailureCode::Capacity);
        Ok(())
    }))
    .unwrap();
}
