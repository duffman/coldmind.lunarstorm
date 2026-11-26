//! Parser, composition, and arithmetic boundaries without a native engine.
use super::*;

fn attachment(json: &str) -> SourceMapAttachment {
    SourceMapAttachment {
        json: json.as_bytes().to_vec(),
        originals: vec![],
        preceding_maps: vec![],
    }
}
fn parse(json: &str) -> Result<SourceMaps, DebugError> {
    SourceMaps::parse(
        "file:///out.js",
        &JsString::from_utf8("abcdef\nabcdef\nabcdef"),
        &attachment(json),
    )
}

#[test]
fn malformed_utf8_and_non_string_content_are_rejected_before_map_admission() {
    for raw in [
        br#"{"version":3,"sourcesContent":[42],"mappings":""}"#.as_slice(),
        br#"{"version":3,"names":[false],"mappings":""}"#.as_slice(),
        &[0xff],
    ] {
        assert!(matches!(parse_json(raw), Err(DebugError::InvalidMap)));
    }
    assert!(!equivalent_json(b"not JSON", b"also not JSON"));
    assert!(equivalent_json(
        br#"{"version":3,"mappings":""}"#,
        br#"{ "mappings":"", "version":3 }"#
    ));
}

#[test]
fn every_base64_digit_family_and_malformed_vlq_shape_is_checked() {
    for (encoded, expected) in [("0A", 10), ("+A", 15), ("/A", -15), ("9A", -14)] {
        let mut cursor = 0;
        assert_eq!(vlq(encoded.as_bytes(), &mut cursor).unwrap(), expected);
        assert_eq!(cursor, encoded.len());
    }
    for encoded in [
        ",",
        "A,",
        "AAAAAA",
        "AA",
        "AAA",
        "!",
        "g",
        "gggggggggggggA",
        "ggggggggggggQ",
    ] {
        assert!(
            matches!(decode_mappings(encoded, 1, 1), Err(DebugError::InvalidMap)),
            "{encoded}"
        );
    }
    let decoded = decode_mappings(";;A", 0, 0).unwrap();
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].generated, Position { line: 2, column: 0 });
    assert!(decoded[0].original.is_none());
}

#[test]
fn position_offsets_and_signed_deltas_reject_overflow_without_wrapping() {
    for (position, by) in [
        (
            Position {
                line: u32::MAX,
                column: 0,
            },
            Position { line: 1, column: 0 },
        ),
        (
            Position {
                line: 0,
                column: u32::MAX,
            },
            Position { line: 0, column: 1 },
        ),
    ] {
        assert_eq!(offset(position, by), Err(DebugError::InvalidMap));
    }
    assert_eq!(
        offset(
            Position { line: 1, column: 2 },
            Position { line: 3, column: 9 }
        )
        .unwrap(),
        Position { line: 4, column: 2 }
    );
    let mut previous = i64::MAX;
    assert_eq!(delta(&mut previous, 1), Err(DebugError::InvalidMap));
    assert_eq!(previous, i64::MAX);
    assert_eq!(add(usize::MAX, 1), Err(DebugError::Capacity));
    assert_eq!(
        allocation_bytes(usize::MAX, size_of::<Segment>()),
        Err(DebugError::Capacity)
    );
    assert_eq!(
        allocation_bytes(3, size_of::<Segment>()),
        Ok(3 * size_of::<Segment>())
    );
}

#[test]
fn a_column_past_any_line_terminator_never_resolves_to_the_next_line() {
    for text in ["a\r\nb", "a\nb", "a\u{2028}b", "a\u{2029}b"] {
        let text = JsString::from_utf8(text);
        assert_eq!(
            validate_position(&text, Position { line: 0, column: 2 }),
            Err(DebugError::InvalidMap)
        );
        validate_position(&text, Position { line: 1, column: 1 }).unwrap();
    }
}

#[test]
fn indexed_fields_external_sections_and_invalid_ignore_indices_fail_closed() {
    for json in [
        r#"{"version":3,"sections":[],"mappings":""}"#,
        r#"{"version":3,"sections":[],"sources":[]}"#,
        r#"{"version":3,"sections":[],"sourceRoot":"src"}"#,
        r#"{"version":3,"sections":[{"offset":{"line":0,"column":0},"url":"external.map","map":{"version":3,"mappings":""}}]}"#,
        r#"{"version":3,"sources":["a.ts"],"ignoreList":[1],"mappings":"AAAA"}"#,
        r#"{"version":3,"sources":["a.ts"],"x_google_ignoreList":[1],"mappings":"AAAA"}"#,
    ] {
        assert!(matches!(parse(json), Err(DebugError::InvalidMap)), "{json}");
    }
    assert!(parse(r#"{"version":3,"sources":["a.ts"],"ignoreList":[0],"x_google_ignoreList":[0],"mappings":"AAAA"}"#).is_ok());
}

#[test]
fn supplied_originals_fill_missing_text_but_unknown_and_conflicting_content_is_rejected() {
    let mut input = attachment(r#"{"version":3,"sources":["a.ts"],"mappings":"AAAA"}"#);
    input.originals.push(OriginalSource {
        origin: "file:///a.ts".into(),
        text: Some(JsString::from_utf8("original")),
    });
    let parsed = SourceMaps::parse("file:///out.js", &JsString::from_utf8("x"), &input).unwrap();
    assert_eq!(
        parsed.root.sources[0]
            .text
            .as_ref()
            .unwrap()
            .to_utf8()
            .unwrap(),
        "original"
    );
    assert_eq!(
        parsed.root.sources[0].digest,
        Some(SourceDigest::of_utf16(&JsString::from_utf8("original")))
    );
    input.originals.push(OriginalSource {
        origin: "file:///a.ts".into(),
        text: Some(JsString::from_utf8("conflict")),
    });
    assert!(matches!(
        SourceMaps::parse("file:///out.js", &JsString::from_utf8("x"), &input),
        Err(DebugError::InvalidMap)
    ));
    input.originals.pop();
    input.originals.push(OriginalSource {
        origin: "file:///unused.ts".into(),
        text: None,
    });
    assert!(matches!(
        SourceMaps::parse("file:///out.js", &JsString::from_utf8("x"), &input),
        Err(DebugError::InvalidMap)
    ));
    assert!(matches!(
        parse(
            r#"{"version":3,"sources":["a.ts","a.ts"],"sourcesContent":["a","b"],"mappings":"AAAA"}"#
        ),
        Err(DebugError::InvalidMap)
    ));
}

#[test]
fn lookups_before_a_mapping_after_its_line_and_without_a_source_uri_stay_unmapped() {
    let parsed = parse(r#"{"version":3,"sources":["a.ts"],"mappings":";AAAA"}"#).unwrap();
    for position in [
        Position { line: 0, column: 0 },
        Position { line: 2, column: 0 },
    ] {
        assert!(parsed.root.lookup_all(position).is_empty());
    }
    let parsed = parse(r#"{"version":3,"sources":[null],"mappings":"AAAA"}"#).unwrap();
    assert!(
        parsed
            .mapped_all(Position { line: 0, column: 0 })
            .unwrap()
            .is_empty()
    );
    let anonymous = Original {
        origin: None,
        text: None,
        digest: None,
    };
    assert!(matches!(
        parsed.trace(&anonymous, Position { line: 0, column: 0 }),
        Err(DebugError::InvalidMap)
    ));
}

#[test]
fn chain_targets_must_be_unique_linked_generated_files() {
    let root = r#"{"version":3,"sources":["a.js"],"mappings":"AAAA"}"#;
    for preceding in [
        vec![r#"{"version":3,"sources":[],"mappings":"A"}"#],
        vec![r#"{"version":3,"file":"out.js","sources":[],"mappings":"A"}"#],
        vec![
            r#"{"version":3,"file":"a.js","sources":[],"mappings":"A"}"#,
            r#"{"version":3,"file":"a.js","sources":[],"mappings":"A"}"#,
        ],
    ] {
        let mut input = attachment(root);
        input.preceding_maps = preceding
            .into_iter()
            .map(|text| text.as_bytes().to_vec())
            .collect();
        assert!(matches!(
            SourceMaps::parse("file:///out.js", &JsString::from_utf8("x"), &input),
            Err(DebugError::InvalidMap)
        ));
    }
}

#[test]
fn converging_transform_paths_expand_the_same_intermediate_state_only_once() {
    let mut input = attachment(r#"{"version":3,"sources":["a.js"],"mappings":"AAAA"}"#);
    input.preceding_maps = [
        r#"{"version":3,"file":"a.js","sources":["b.js","c.js"],"mappings":"AAAA,ACAA"}"#,
        r#"{"version":3,"file":"b.js","sources":["d.js"],"mappings":"AAAA"}"#,
        r#"{"version":3,"file":"c.js","sources":["d.js"],"mappings":"AAAA"}"#,
        r#"{"version":3,"file":"d.js","sources":["original.ts"],"sourcesContent":["original"],"mappings":"AAAA"}"#,
    ].into_iter().map(|text| text.as_bytes().to_vec()).collect();
    let parsed = SourceMaps::parse("file:///out.js", &JsString::from_utf8("x"), &input).unwrap();
    let locations = parsed.mapped_all(Position { line: 0, column: 0 }).unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].origin, "file:///original.ts");
    assert_eq!((locations[0].line, locations[0].column), (0, 0));
}

#[test]
fn deeply_nested_index_data_is_rejected_at_the_parser_depth_boundary() {
    let mut raw = parse_json(br#"{"version":3,"sources":[],"mappings":""}"#).unwrap();
    for _ in 0..65 {
        let mut next = parse_json(br#"{"version":3,"sections":[]}"#).unwrap();
        next.sections = Some(vec![RawSection {
            offset: RawOffset { line: 0, column: 0 },
            map: Box::new(raw),
            url: None,
        }]);
        raw = next;
    }
    assert!(matches!(
        parse_map(&raw, &Url::parse("file:///out.js").unwrap(), 0),
        Err(DebugError::InvalidMap)
    ));
}

#[test]
fn source_paths_with_control_characters_are_rejected_without_uri_resolution() {
    assert!(matches!(
        parse(r#"{"version":3,"sources":["a\u0000.ts"],"mappings":"AAAA"}"#),
        Err(DebugError::InvalidMap)
    ));
}

#[test]
fn a_linked_preceding_map_must_itself_pass_the_complete_v3_parser() {
    let mut input = attachment(r#"{"version":3,"sources":["a.js"],"mappings":"AAAA"}"#);
    input.preceding_maps.push(
        br#"{"version":2,"file":"a.js","sources":["original.ts"],"mappings":"AAAA"}"#.to_vec(),
    );
    assert!(matches!(
        SourceMaps::parse("file:///out.js", &JsString::from_utf8("x"), &input),
        Err(DebugError::InvalidMap)
    ));
}
