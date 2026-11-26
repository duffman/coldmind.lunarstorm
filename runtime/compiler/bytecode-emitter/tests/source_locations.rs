//! Source locations of emitted code: whole-source mapping, UTF-8 spans of scalar code
//! and hand-built IR.

use qjs_bytecode_image_verifier::image::{Constant, SourceUnit};
use qjs_bytecode_emitter::{
    CompileOptions, JsEmitOptions, SourceInput, compile, compile_script, emit_program_with_options,
};
#[test]
fn general_locations_use_whole_utf16_source_and_function_start_points() {
    let mut units: Vec<_> = "//".encode_utf16().collect();
    units.push(0xd800);
    units.extend("\r\n//😀\nfunction f(){\r let x=40;\u{2028} x+2;\u{2029}}\nf();".encode_utf16());
    let input = SourceInput::from_utf16(&units).unwrap();
    let program = qjs_js_parser_frontend::compile_program_input(&input, "unicode.js").unwrap();
    let image = emit_program_with_options(
        &program,
        &JsEmitOptions {
            debug_markers: true,
        },
    )
    .unwrap();
    let Constant::Function(function) = &image.function().constants[0] else {
        panic!()
    };
    assert_eq!(
        (
            function.debug_locations[0].line,
            function.debug_locations[0].column
        ),
        (3, 1)
    );
    assert_eq!(
        (
            function.debug_locations.last().unwrap().line,
            function.debug_locations.last().unwrap().column
        ),
        (3, 1)
    );
    assert!(
        function
            .debug_locations
            .iter()
            .any(|l| (l.line, l.column) == (4, 2))
    );
    assert!(
        function
            .debug_locations
            .iter()
            .any(|l| (l.line, l.column) == (5, 2))
    );
    let metadata = image.spans().unwrap();
    assert_eq!(metadata.source_unit, SourceUnit::Utf16);
    assert_eq!(metadata.source_len, units.len() as u32);
    assert_eq!(
        metadata.source_digest,
        qjs_bytecode_emitter::source_digest_utf16(&units)
    );
    let spans = metadata.functions.iter().find(|f| f.path == [0]).unwrap();
    let start = units
        .windows(8)
        .position(|w| w == "function".encode_utf16().collect::<Vec<_>>())
        .unwrap() as u32;
    assert_eq!(spans.function_start, start);
    assert_eq!(spans.positions[0].source_start, start);
    assert_eq!(spans.positions[0].source_end, start);
    assert!(
        spans
            .positions
            .iter()
            .all(|p| p.source_start >= spans.function_start && p.source_end <= spans.function_end)
    );
}
#[test]
fn scalar_locations_map_utf8_spans_to_true_utf16_columns() {
    let source = "//😀\r\nfunction main(){\r /*😀*/ return 40+2;\u{2028}}\n";
    let unit = compile(source, "scalar.js", &CompileOptions::default()).unwrap();
    let image = unit.finalize().unwrap();
    let metadata = image.spans().unwrap();
    assert_eq!(metadata.source_unit, SourceUnit::Utf8);
    assert_eq!(metadata.source_len, source.len() as u32);
    let source_map = unit.metadata.source.as_ref().unwrap();
    let spans = &metadata.functions[0];
    for span in &spans.positions {
        let native = image
            .function()
            .debug_locations
            .iter()
            .find(|l| l.code_offset == span.code_offset)
            .unwrap();
        assert_eq!(
            (native.line, native.column),
            source_map.parser_line_column(span.source_start).unwrap()
        );
    }
    assert!(
        image
            .function()
            .debug_locations
            .iter()
            .any(|l| (l.line, l.column) == (3, 16))
    );
}
#[test]
fn raw_native_decode_does_not_reconstruct_owned_span_endpoints() {
    let image = compile_script("function f(){\nreturn 42;\n}f();", "native-only.js").unwrap();
    let native = qjs_bytecode_image_verifier::image::decode_and_verify(image.bytes()).unwrap();
    assert!(native.spans().is_none());
    assert_eq!(
        native.function().debug_locations,
        image.function().debug_locations
    );
}

#[test]
fn manually_constructed_ir_can_explicitly_leave_source_locations_unknown() {
    let mut unit = compile(
        "function main(){return 42;}",
        "unknown.ir",
        &CompileOptions::default(),
    )
    .unwrap();
    unit.metadata.source = None;
    let emitted = qjs_bytecode_emitter::emit_target(&unit.target, &unit.metadata).unwrap();
    assert!(emitted.debug_locations.is_empty());
    let image = unit.finalize().unwrap();
    assert!(image.spans().is_none());
    assert!(image.function().source.is_none());
    assert!(
        image
            .function()
            .debug_locations
            .iter()
            .all(|p| p.line == 1 && p.column == 1)
    );
}
