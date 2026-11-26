use qjs_js_parser_frontend::{compile_module_input, compile_program_input, source::SourceInput};
use qjs_scalar_ssa_ir::js::{Export, ExpressionKind, Literal, StatementKind};
fn source(prefix: &str, raw: &[u16], suffix: &str) -> SourceInput {
    let mut units: Vec<_> = prefix.encode_utf16().collect();
    units.extend(raw);
    units.extend(suffix.encode_utf16());
    SourceInput::from_utf16(&units).unwrap()
}
#[test]
fn directive_cooked_values_preserve_raw_and_escaped_surrogates() {
    for raw in [
        vec![0xd800, 0],
        vec![b'\\' as u16, 0xd800, 0],
        "\\ud800\\0".encode_utf16().collect(),
    ] {
        let input = source("'", &raw, "';");
        let program = compile_program_input(&input, "directive.js").unwrap();
        let StatementKind::Expression(expression) = &program.body[0].kind else {
            panic!()
        };
        let units = input
            .original()
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            qjs_reference_engine_bridge::ReferenceRunner::from_env()
                .evaluate(&format!("eval(String.fromCharCode({units}))"))
                .unwrap(),
            qjs_reference_engine_bridge::ReferenceValue::String(vec![0xd800, 0])
        );
        assert_eq!(
            expression.kind,
            ExpressionKind::Literal(Literal::String(vec![0xd800, 0]))
        );
    }
}
#[test]
fn quoted_method_keys_preserve_original_surrogate_units() {
    for raw in [
        vec![0xd800, 0],
        vec![b'\\' as u16, 0xd800, 0],
        "\\ud800\\0".encode_utf16().collect(),
    ] {
        let input = source("({'", &raw, "'(){return 42;}});");
        let program = compile_program_input(&input, "method.js").unwrap();
        let StatementKind::Expression(expression) = &program.body[0].kind else {
            panic!()
        };
        let ExpressionKind::Object(properties) = &expression.kind else {
            panic!()
        };
        let qjs_scalar_ssa_ir::js::ObjectEntry::Data {
            key: qjs_scalar_ssa_ir::js::ObjectKey::Static(key),
            ..
        } = &properties[0]
        else {
            panic!()
        };
        assert_eq!(*key, vec![0xd800, 0]);
        let units = input
            .original()
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            qjs_reference_engine_bridge::ReferenceRunner::from_env()
                .evaluate(&format!(
                    "Object.keys(eval(String.fromCharCode({units})))[0]"
                ))
                .unwrap(),
            qjs_reference_engine_bridge::ReferenceValue::String(vec![0xd800, 0])
        );
    }
}
#[test]
fn module_request_and_string_export_names_preserve_cooked_utf16() {
    for raw in [
        vec![0xd800],
        vec![b'\\' as u16, 0xd800],
        "\\ud800".encode_utf16().collect(),
    ] {
        let input = source("import value from '", &raw, "';export{value as 'named'};");
        let program = compile_module_input(&input, "module.mjs").unwrap();
        assert_eq!(program.module.unwrap().requests, vec![vec![0xd800]]);
    }
    let input = source("let x=42;export{x as '", &[0xd83d, 0xde00, 0], "'};");
    let program = compile_module_input(&input, "module.mjs").unwrap();
    assert!(
        matches!(&program.module.unwrap().exports[0],Export::Local{name,..} if name==&vec![0xd83d,0xde00,0])
    );
}
#[test]
fn string_export_names_reject_lone_surrogates() {
    for raw in [vec![0xd800], "\\ud800".encode_utf16().collect()] {
        let input = source("let x=42;export{x as '", &raw, "'};");
        assert!(compile_module_input(&input, "module.mjs").is_err());
    }
}
