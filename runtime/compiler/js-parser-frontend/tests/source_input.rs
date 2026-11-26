//! Source input: astral characters, lone surrogates and NULs, raw template text,
//! diagnostics mapped back to original positions, and regexp validation.

use qjs_js_parser_frontend::source::{SourceInput, Utf16Span};

#[test]
fn astral_source_has_exact_boundary_mapping() {
    let units: Vec<u16> = "const 𐐀 = 42;".encode_utf16().collect();
    let input = SourceInput::from_utf16(&units).unwrap();
    assert_eq!(input.parser_text(), "const 𐐀 = 42;");
    assert_eq!(
        input.original_span(6..10),
        Some(Utf16Span { start: 6, end: 8 })
    );
    assert_eq!(
        input.parser_offset(7),
        None,
        "inside a surrogate pair is not a UTF8 boundary"
    );
    assert_eq!(input.parser_offset(8), Some(10));
    assert!(input.require_literal_fidelity().is_ok());
}

#[test]
fn lone_surrogate_and_nul_are_owned_without_loss() {
    let units = [b'\'' as u16, 0xd800, 0, b'\'' as u16];
    let input = SourceInput::from_utf16(&units).unwrap();
    assert_eq!(input.original(), &units);
    assert_eq!(input.original_units(1..8), Some(&units[1..3]));
    assert!(
        input.require_literal_fidelity().is_err(),
        "marker text must not be mistaken for original literal values"
    );
    let arena = oxc_allocator::Allocator::default();
    let parsed =
        oxc_parser::Parser::new(&arena, input.parser_text(), oxc_span::SourceType::cjs()).parse();
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
}

#[test]
fn tagged_template_raw_is_recovered_from_original_units() {
    let units = [b'`' as u16, b'\\' as u16, 0xd800, b'`' as u16];
    let input = SourceInput::from_utf16(&units).unwrap();
    assert_eq!(input.original_units(1..7), Some(&units[1..3]));
    assert_ne!(
        input.parser_text().encode_utf16().collect::<Vec<_>>(),
        units
    );
    assert!(input.require_literal_fidelity().is_err());
}

#[test]
fn diagnostics_map_back_across_astral_and_lone_surrogate() {
    let mut units: Vec<u16> = "/*😀".encode_utf16().collect();
    units.push(0xdfff);
    units.extend("*/ const x = ;".encode_utf16());
    let input = SourceInput::from_utf16(&units).unwrap();
    let arena = oxc_allocator::Allocator::default();
    let parsed =
        oxc_parser::Parser::new(&arena, input.parser_text(), oxc_span::SourceType::cjs()).parse();
    assert!(!parsed.diagnostics.is_empty());
    let label = parsed.diagnostics[0].labels.first().unwrap();
    let span = input
        .original_span(label.offset() as usize..(label.offset() + label.len()) as usize)
        .unwrap();
    assert_eq!(units[span.start as usize], b';' as u16);
    assert_eq!(input.line_column(span.start), Some((1, span.start + 1)));
}

#[test]
fn directives_comments_and_invalid_surrogate_identifier_keep_grammar() {
    let mut units: Vec<u16> = "\"use strict\"; /*".encode_utf16().collect();
    units.push(0xd800);
    units.extend("*/ const 𐐀=1;".encode_utf16());
    let input = SourceInput::from_utf16(&units).unwrap();
    let arena = oxc_allocator::Allocator::default();
    let parsed =
        oxc_parser::Parser::new(&arena, input.parser_text(), oxc_span::SourceType::cjs()).parse();
    assert!(parsed.diagnostics.is_empty());
    assert_eq!(parsed.program.directives[0].directive, "use strict");
    let invalid = SourceInput::from_utf16(&[0xd800, b'=' as u16, b'1' as u16]).unwrap();
    let parsed =
        oxc_parser::Parser::new(&arena, invalid.parser_text(), oxc_span::SourceType::cjs()).parse();
    assert!(!parsed.diagnostics.is_empty());
}

#[test]
fn cooked_string_backslash_parity_matches_original_units_and_reference() {
    use oxc_ast::ast::{Expression, Statement};
    for slashes in 0..=4 {
        let mut units: Vec<u16> = "const x='".encode_utf16().collect();
        units.extend(std::iter::repeat_n(b'\\' as u16, slashes));
        units.extend([0xd800, 0]);
        units.extend("';".encode_utf16());
        let input = SourceInput::from_utf16(&units).unwrap();
        let arena = oxc_allocator::Allocator::default();
        let parsed =
            oxc_parser::Parser::new(&arena, input.parser_text(), oxc_span::SourceType::cjs())
                .parse();
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let Statement::VariableDeclaration(decl) = &parsed.program.body[0] else {
            panic!()
        };
        let Expression::StringLiteral(literal) = decl.declarations[0].init.as_ref().unwrap() else {
            panic!()
        };
        let mut expected = vec![b'\\' as u16; slashes / 2];
        expected.extend([0xd800, 0]);
        assert_eq!(
            literal.value.encode_utf16().collect::<Vec<_>>(),
            expected,
            "slashes={slashes}"
        );
        let original_spelling = input
            .original_units(literal.span.start as usize..literal.span.end as usize)
            .unwrap();
        assert_eq!(original_spelling, &units[8..units.len() - 1]);
        let reference_source = format!("'{}\\ud800\\0'", "\\\\".repeat(slashes / 2));
        assert_eq!(
            qjs_reference_engine_bridge::ReferenceRunner::from_env()
                .evaluate(&reference_source)
                .unwrap(),
            qjs_reference_engine_bridge::ReferenceValue::String(expected)
        );
    }
}

#[test]
fn regexp_identity_escape_counterexample_is_not_certified_as_lossless() {
    // /\<raw surrogate>/u is an invalid Unicode identity escape. Blindly
    // spelling that surrogate as uD800 after the slash would make it valid.
    // This is a named feasibility blocker, not permission to emit the AST.
    let units = [b'/' as u16, b'\\' as u16, 0xd800, b'/' as u16, b'u' as u16];
    let input = SourceInput::from_utf16(&units).unwrap();
    assert_eq!(input.parser_text(), "/\\ud800/u");
    let reference = qjs_reference_engine_bridge::ReferenceRunner::from_env();
    assert_eq!(reference.evaluate("try { eval(String.fromCharCode(47,92,55296,47,117)); false } catch(e) { e instanceof SyntaxError }").unwrap(),qjs_reference_engine_bridge::ReferenceValue::Boolean(true));
    assert_eq!(
        reference
            .evaluate("try { /\\ud800/u; false } catch(e) { e instanceof SyntaxError }")
            .unwrap(),
        qjs_reference_engine_bridge::ReferenceValue::Boolean(false)
    );
    assert!(input.require_literal_fidelity().is_err());
    assert_eq!(
        input.original_units(0..input.parser_text().len()),
        Some(units.as_slice())
    );
}

#[test]
fn original_line_columns_and_invalid_ranges_are_checked() {
    let input =
        SourceInput::from_utf16(&"a\r\n😀\u{2028}b".encode_utf16().collect::<Vec<_>>()).unwrap();
    assert_eq!(input.line_column(5), Some((2, 3)));
    assert_eq!(input.line_column(6), Some((3, 1)));
    assert_eq!(input.line_column(99), None);
    assert_eq!(input.original_span(99..100), None);
    assert_eq!(
        input.original_span(std::ops::Range { start: 3, end: 2 }),
        None
    );
}

#[test]
fn tagged_ast_cooked_and_original_raw_are_distinct() {
    use oxc_ast::ast::{Expression, Statement};
    let mut units: Vec<u16> = "tag`".encode_utf16().collect();
    units.extend([b'\\' as u16, 0xd800]);
    units.push(b'`' as u16);
    let input = SourceInput::from_utf16(&units).unwrap();
    let arena = oxc_allocator::Allocator::default();
    let parsed =
        oxc_parser::Parser::new(&arena, input.parser_text(), oxc_span::SourceType::cjs()).parse();
    assert!(parsed.diagnostics.is_empty());
    let Statement::ExpressionStatement(stmt) = &parsed.program.body[0] else {
        panic!()
    };
    let Expression::TaggedTemplateExpression(tagged) = &stmt.expression else {
        panic!()
    };
    let quasi = &tagged.quasi.quasis[0];
    assert_eq!(
        quasi
            .value
            .cooked
            .unwrap()
            .encode_utf16()
            .collect::<Vec<_>>(),
        vec![0xd800]
    );
    let raw = input
        .original_units(quasi.span.start as usize..quasi.span.end as usize)
        .unwrap();
    assert_eq!(raw, &[b'\\' as u16, 0xd800]);
    assert_ne!(quasi.value.raw.encode_utf16().collect::<Vec<_>>(), raw);
}

#[test]
fn original_regexp_validation_rejects_identity_escape_before_ast_use() {
    let input =
        SourceInput::from_utf16(&[b'/' as u16, b'\\' as u16, 0xd800, b'/' as u16, b'u' as u16])
            .unwrap();
    let error = input.validate_regexp_literal(1..7, 8..9).unwrap_err();
    assert!(error.span.start >= 1 && error.span.end <= 3, "{error:?}");
}

#[test]
fn original_regexp_validation_handles_surrogate_ranges_flags_and_nul() {
    for flags in ["", "u", "v"] {
        for pattern in [
            vec![0xd800],
            vec![0],
            vec![b'[' as u16, 0xd800, b'-' as u16, 0xdfff, b']' as u16],
        ] {
            qjs_js_parser_frontend::source::validate_regexp_units(
                &pattern,
                &flags.encode_utf16().collect::<Vec<_>>(),
            )
            .unwrap();
        }
    }
    assert!(
        qjs_js_parser_frontend::source::validate_regexp_units(
            &[b'[' as u16, 0xdfff, b'-' as u16, 0xd800, b']' as u16],
            &[b'u' as u16]
        )
        .is_err()
    );
    for flags in ["uu", "uv", "x"] {
        assert!(
            qjs_js_parser_frontend::source::validate_regexp_units(
                &[0xd800],
                &flags.encode_utf16().collect::<Vec<_>>()
            )
            .is_err()
        );
    }
}

#[test]
fn original_regexp_validation_matches_reference_for_raw_units() {
    let patterns = vec![
        vec![0xd800],
        vec![b'\\' as u16, 0xd800],
        vec![b'\\' as u16, b'\\' as u16, 0xd800],
        vec![0],
        vec![b'[' as u16, 0xd800, b'-' as u16, 0xdfff, b']' as u16],
        vec![b'[' as u16, 0xdfff, b'-' as u16, 0xd800, b']' as u16],
    ];
    let reference = qjs_reference_engine_bridge::ReferenceRunner::from_env();
    for pattern in patterns {
        for flags in ["", "u", "v", "uv", "uu"] {
            let units = pattern
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let js = format!(
                "try {{ new RegExp(String.fromCharCode({units}), '{flags}'); true }} catch(e) {{ if (!(e instanceof SyntaxError)) throw e; false }}"
            );
            let expected = reference.evaluate(&js).unwrap();
            let actual = qjs_js_parser_frontend::source::validate_regexp_units(
                &pattern,
                &flags.encode_utf16().collect::<Vec<_>>(),
            )
            .is_ok();
            assert_eq!(
                qjs_reference_engine_bridge::ReferenceValue::Boolean(actual),
                expected,
                "units={pattern:?} flags={flags}"
            );
        }
    }
}

#[test]
fn source_lines_cover_every_ecmascript_terminator_and_utf16_unit() {
    let units = [
        b'a' as u16,
        13,
        10,
        0xd83d,
        0xde00,
        10,
        0xd800,
        13,
        b'b' as u16,
        0x2028,
        b'c' as u16,
        0x2029,
        b'd' as u16,
    ];
    let input = SourceInput::from_utf16(&units).unwrap();
    for (at, expected) in [
        (0, (1, 1)),
        (1, (1, 2)),
        (2, (2, 1)),
        (3, (2, 1)),
        (4, (2, 2)),
        (5, (2, 3)),
        (6, (3, 1)),
        (7, (3, 2)),
        (8, (4, 1)),
        (9, (4, 2)),
        (10, (5, 1)),
        (11, (5, 2)),
        (12, (6, 1)),
        (13, (6, 2)),
    ] {
        assert_eq!(input.line_column(at), Some(expected), "offset {at}");
    }
    for (byte, unit) in input
        .parser_text()
        .char_indices()
        .map(|(byte, _)| byte)
        .filter_map(|byte| input.original_span(byte..byte).map(|s| (byte, s.start)))
    {
        assert_eq!(
            input.parser_line_column(byte as u32),
            input.line_column(unit)
        );
    }
    assert_eq!(input.line_column(14), None);
}
