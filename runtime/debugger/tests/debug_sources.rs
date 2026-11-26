use coldmind_runtime::{
    debug::{
        DebugError, DebugIdentities, DebugLimits, DebugLocation, DebugReply, NativeScriptKey,
        NativeScriptMetadata, Page, ScriptKind, ScriptSelector, SourceRegistry,
    },
    engine::{
        ContextLimits, EngineIdentity, JsString, OriginalSource, ScriptSource, SourceMapAttachment,
        ValueStore,
    },
};
use std::num::NonZeroU64;

fn fixture() -> (ValueStore<()>, DebugIdentities) {
    let engine = EngineIdentity::new().unwrap();
    let store = ValueStore::new(&engine, ContextLimits::default()).unwrap();
    let ids = DebugIdentities::new(NonZeroU64::new(1).unwrap(), store.context(), 16).unwrap();
    (store, ids)
}
fn metadata(
    context: coldmind_runtime::engine::ContextId,
    key: u64,
    origin: &str,
    text: &str,
    map: Option<&str>,
) -> NativeScriptMetadata {
    let mut source = ScriptSource::new(JsString::from_utf8(text), origin.to_owned());
    source.source_map = map.map(|json| SourceMapAttachment {
        json: json.as_bytes().to_vec(),
        originals: vec![],
        preceding_maps: vec![],
    });
    NativeScriptMetadata {
        key: NativeScriptKey::new(context, NonZeroU64::new(key).unwrap()),
        source,
        kind: ScriptKind::Script,
    }
}
fn selector(origin: &str) -> ScriptSelector {
    ScriptSelector {
        origin: origin.to_owned(),
        expected_digest: None,
    }
}

#[test]
fn source_registration_deduplicates_without_an_attached_session_and_refuses_new_content_at_capacity()
 {
    let (store, ids) = fixture();
    let limits = DebugLimits {
        scripts: 1,
        ..DebugLimits::default()
    };
    let mut registry = SourceRegistry::new(store.context(), limits).unwrap();
    let first = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///app.js", "one", None),
        )
        .unwrap();
    let duplicate = registry
        .register(
            &ids,
            metadata(store.context(), 2, "file:///app.js", "one", None),
        )
        .unwrap();
    assert_eq!(first, duplicate);
    assert_eq!(
        registry.register(
            &ids,
            metadata(store.context(), 3, "file:///app.js", "two", None)
        ),
        Err(DebugError::Capacity)
    );
    assert_eq!(
        registry.select(&selector("file:///app.js")).unwrap(),
        Some(first)
    );
}

#[test]
fn source_pages_reassemble_more_than_common_reply_capacity_losslessly() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let mut units = vec![0x61; 180_000];
    units[31] = 0xd800;
    units[90_000] = 0xdc00;
    let source = ScriptSource::new(
        JsString::from_units(units.clone()),
        "file:///large.js".to_owned(),
    );
    let registration = registry
        .register(
            &ids,
            NativeScriptMetadata {
                key: NativeScriptKey::new(store.context(), NonZeroU64::new(1).unwrap()),
                source,
                kind: ScriptKind::Script,
            },
        )
        .unwrap();
    let mut rebuilt = vec![];
    while rebuilt.len() < units.len() {
        match registry
            .source(
                registration.id,
                registration.revision,
                Page {
                    start: rebuilt.len(),
                    count: 40_000,
                },
            )
            .unwrap()
        {
            DebugReply::Source { text, total_units } => {
                assert_eq!(total_units, units.len());
                rebuilt.extend(text.into_units());
            }
            _ => panic!("expected source page"),
        }
    }
    assert_eq!(rebuilt, units);
    assert_eq!(
        registry
            .source(
                registration.id,
                registration.revision,
                Page {
                    start: 0,
                    count: 180_000
                }
            )
            .unwrap_err(),
        DebugError::Capacity
    );
}

#[test]
fn revisions_keep_old_source_and_digest_selection_stable() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let a = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///same.js", "a", None),
        )
        .unwrap();
    let b = registry
        .register(
            &ids,
            metadata(store.context(), 2, "file:///same.js", "b", None),
        )
        .unwrap();
    assert_ne!(a.id, b.id);
    assert!(b.revision > a.revision);
    assert_eq!(
        registry.select(&selector("file:///same.js")).unwrap(),
        Some(b)
    );
    assert_eq!(
        registry
            .select(&ScriptSelector {
                origin: "file:///same.js".to_owned(),
                expected_digest: Some(a.digest)
            })
            .unwrap(),
        Some(a)
    );
    match registry
        .source(a.id, a.revision, Page { start: 0, count: 1 })
        .unwrap()
    {
        DebugReply::Source { text, .. } => assert_eq!(text.to_utf8().unwrap(), "a"),
        _ => panic!(),
    }
}

#[test]
fn basic_source_map_resolves_unicode_columns_and_original_breakpoints() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    // AAAA = generated 0 -> source 0 line 0 column 0; EAAE = delta +2 -> original +2.
    let map = r#"{"version":3,"file":"out.js","sourceRoot":"../src/","sources":["a.ts"],"sourcesContent":["🔥x"],"names":[],"mappings":"AAAA,EAAE"}"#;
    let reg = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///build/out.js", "🔥x", Some(map)),
        )
        .unwrap();
    let mapped = registry
        .map_location(&DebugLocation {
            script: reg.id,
            revision: reg.revision,
            line: 0,
            column: Some(2),
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        (mapped.origin.as_str(), mapped.line, mapped.column),
        ("file:///src/a.ts", 0, 2)
    );
    assert!(mapped.digest.is_some());
    let candidates = registry
        .resolve_breakpoint(&selector("file:///src/a.ts"), 0, Some(2))
        .unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].column, Some(2));
}

#[test]
fn indexed_map_offsets_only_the_first_generated_line_column() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map = r#"{"version":3,"sections":[{"offset":{"line":0,"column":2},"map":{"version":3,"sources":["a.ts"],"names":[],"mappings":"AAAA;AACA"}},{"offset":{"line":2,"column":0},"map":{"version":3,"sources":["b.ts"],"names":[],"mappings":"AAAA"}}]}"#;
    let reg = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "  a\nb\nc", Some(map)),
        )
        .unwrap();
    let a = registry
        .resolve_breakpoint(&selector("file:///a.ts"), 1, Some(0))
        .unwrap();
    assert_eq!((a[0].line, a[0].column), (1, Some(0)));
    let mapped = registry
        .map_location(&DebugLocation {
            script: reg.id,
            revision: reg.revision,
            line: 2,
            column: Some(0),
        })
        .unwrap()
        .unwrap();
    assert_eq!(mapped.origin, "file:///b.ts");
    assert_eq!(mapped.digest, None);
}

#[test]
fn preceding_maps_compose_only_through_matching_intermediate_source_identities() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map = r#"{"version":3,"sources":["middle.js"],"sourcesContent":["  x"],"names":[],"mappings":"AAAE"}"#;
    let preceding = r#"{"version":3,"file":"middle.js","sources":["original.ts"],"sourcesContent":[" x"],"names":[],"mappings":"EAAC"}"#;
    let mut meta = metadata(store.context(), 1, "file:///out.js", "x", Some(map));
    meta.source
        .source_map
        .as_mut()
        .unwrap()
        .preceding_maps
        .push(preceding.as_bytes().to_vec());
    let reg = registry.register(&ids, meta).unwrap();
    let mapped = registry
        .map_location(&DebugLocation {
            script: reg.id,
            revision: reg.revision,
            line: 0,
            column: Some(0),
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        (mapped.origin.as_str(), mapped.line, mapped.column),
        ("file:///original.ts", 0, 1)
    );
    assert_eq!(
        registry
            .resolve_breakpoint(&selector("file:///original.ts"), 0, Some(1))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn duplicate_without_map_preserves_staged_mapping_and_conflicting_map_is_rejected() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map = r#"{"version":3,"sources":["a.ts"],"names":[],"mappings":"AAAA"}"#;
    let reg = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "x", Some(map)),
        )
        .unwrap();
    assert_eq!(
        registry
            .register(
                &ids,
                metadata(store.context(), 2, "file:///out.js", "x", None)
            )
            .unwrap(),
        reg
    );
    let conflicting = map.replace("a.ts", "b.ts");
    assert_eq!(
        registry.register(
            &ids,
            metadata(
                store.context(),
                3,
                "file:///out.js",
                "x",
                Some(&conflicting)
            )
        ),
        Err(DebugError::InvalidMap)
    );
    assert_eq!(
        registry
            .resolve_breakpoint(&selector("file:///a.ts"), 0, None)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn malformed_maps_and_out_of_bounds_coordinates_fail_before_registration() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let invalid = [
        r#"{"version":2,"sources":[],"names":[],"mappings":""}"#,
        r#"{"version":3,"sources":["a.ts"],"names":[],"mappings":"g"}"#,
        r#"{"version":3,"sources":["a.ts"],"names":[],"mappings":"!AAA"}"#,
        r#"{"version":3,"sources":["a.ts"],"names":[],"mappings":"ACAA"}"#,
        r#"{"version":3,"sources":["a.ts"],"names":[],"mappings":"AAAAA"}"#,
        r#"{"version":3,"sources":["a.ts"],"names":[],"mappings":"EAAA"}"#,
        r#"{"version":3,"sources":["a.ts"],"sourcesContent":["x"],"names":[],"mappings":"AAAE"}"#,
        r#"{"version":3,"sources":["http://[bad"],"names":[],"mappings":"AAAA"}"#,
        r#"{"version":3,"sections":[{"offset":{"line":0,"column":0},"url":"part.map"}]}"#,
        r#"{"version":3,"sections":[{"offset":{"line":0,"column":0},"map":{"version":3,"sources":["a.ts"],"names":[],"mappings":"CAAA"}},{"offset":{"line":0,"column":1},"map":{"version":3,"sources":["b.ts"],"names":[],"mappings":"AAAA"}}]}"#,
    ];
    for (i, map) in invalid.iter().enumerate() {
        assert_eq!(
            registry.register(
                &ids,
                metadata(
                    store.context(),
                    i as u64 + 1,
                    "file:///out.js",
                    "x",
                    Some(map)
                )
            ),
            Err(DebugError::InvalidMap),
            "map {map}"
        );
    }
    match registry
        .sources(Page {
            start: 0,
            count: 16,
        })
        .unwrap()
    {
        DebugReply::Sources { scripts, total } => {
            assert!(scripts.is_empty());
            assert_eq!(total, 0);
        }
        _ => panic!(),
    }
}

#[test]
fn capacity_accounts_owned_source_capacity_and_supplied_originals() {
    let (store, ids) = fixture();
    let limits = DebugLimits {
        source_bytes: 1000,
        ..DebugLimits::default()
    };
    let mut registry = SourceRegistry::new(store.context(), limits).unwrap();
    let mut text = Vec::with_capacity(1000);
    text.push(b'x' as u16);
    let source = ScriptSource::new(JsString::from_units(text), "file:///out.js".to_owned());
    assert_eq!(
        registry.register(
            &ids,
            NativeScriptMetadata {
                key: NativeScriptKey::new(store.context(), NonZeroU64::new(1).unwrap()),
                source,
                kind: ScriptKind::Script
            }
        ),
        Err(DebugError::Capacity)
    );
    let map = r#"{"version":3,"sources":["a.ts"],"names":[],"mappings":"AAAA"}"#;
    let mut meta = metadata(store.context(), 2, "file:///out.js", "x", Some(map));
    meta.source
        .source_map
        .as_mut()
        .unwrap()
        .originals
        .push(OriginalSource {
            origin: "file:///a.ts".to_owned(),
            text: Some(JsString::from_units(vec![0x61; 1000])),
        });
    assert_eq!(registry.register(&ids, meta), Err(DebugError::Capacity));
}

#[test]
fn valid_duplicate_generated_points_keep_every_original_breakpoint_candidate() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    // Both original files map to the same generated point, a valid v3 many-to-one case.
    let map = r#"{"version":3,"sources":["a.ts","b.ts"],"names":[],"mappings":"AAAA,ACAA"}"#;
    let reg = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "x", Some(map)),
        )
        .unwrap();
    for origin in ["file:///a.ts", "file:///b.ts"] {
        let candidates = registry
            .resolve_breakpoint(&selector(origin), 0, Some(0))
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].script, reg.id);
    }
}

#[test]
fn unmapped_intermediate_span_is_valid_but_has_no_fabricated_original_location() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map = r#"{"version":3,"sources":["middle.js"],"sourcesContent":["x"],"names":[],"mappings":"AAAA"}"#;
    let mut meta = metadata(store.context(), 1, "file:///out.js", "x", Some(map));
    meta.source
        .source_map
        .as_mut()
        .unwrap()
        .preceding_maps
        .push(
            br#"{"version":3,"file":"middle.js","sources":[],"names":[],"mappings":"A"}"#.to_vec(),
        );
    let reg = registry.register(&ids, meta).unwrap();
    assert_eq!(
        registry
            .map_location(&DebugLocation {
                script: reg.id,
                revision: reg.revision,
                line: 0,
                column: Some(0)
            })
            .unwrap(),
        None
    );
}

#[test]
fn vlq_reserved_minimum_and_more_than_32_value_bits_are_rejected() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    for (key, mappings) in [(1, "BAAA"), (2, "AAggggggEA")] {
        let json =
            format!(r#"{{"version":3,"sources":["a.ts"],"names":[],"mappings":"{mappings}"}}"#);
        assert_eq!(
            registry.register(
                &ids,
                metadata(store.context(), key, "file:///out.js", "x", Some(&json))
            ),
            Err(DebugError::InvalidMap)
        );
    }
}

#[test]
fn relative_source_paths_resolve_against_source_root_and_missing_text_stays_unavailable() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map = r#"{"version":3,"sourceRoot":"https://example.test/src","sources":["../types/a%20b.ts"],"names":[],"mappings":"AAAA"}"#;
    let reg = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "x", Some(map)),
        )
        .unwrap();
    let mapped = registry
        .map_location(&DebugLocation {
            script: reg.id,
            revision: reg.revision,
            line: 0,
            column: Some(0),
        })
        .unwrap()
        .unwrap();
    assert_eq!(mapped.origin, "https://example.test/types/a%20b.ts");
    assert!(mapped.digest.is_none());
}

#[test]
fn supplied_original_content_conflicts_and_cyclic_or_unlinked_chains_fail_transactionally() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map =
        r#"{"version":3,"sources":["a.ts"],"sourcesContent":["a"],"names":[],"mappings":"AAAA"}"#;
    let mut meta = metadata(store.context(), 1, "file:///out.js", "x", Some(map));
    meta.source
        .source_map
        .as_mut()
        .unwrap()
        .originals
        .push(OriginalSource {
            origin: "file:///a.ts".to_owned(),
            text: Some(JsString::from_utf8("b")),
        });
    assert_eq!(registry.register(&ids, meta), Err(DebugError::InvalidMap));
    let mut meta = metadata(store.context(), 2, "file:///out.js", "x", Some(map));
    meta.source
        .source_map
        .as_mut()
        .unwrap()
        .preceding_maps
        .push(
        br#"{"version":3,"file":"unrelated.js","sources":["a.ts"],"names":[],"mappings":"AAAA"}"#
            .to_vec(),
    );
    assert_eq!(registry.register(&ids, meta), Err(DebugError::InvalidMap));
    let mut meta = metadata(store.context(), 3, "file:///out.js", "x", Some(map));
    meta.source
        .source_map
        .as_mut()
        .unwrap()
        .preceding_maps
        .push(
            br#"{"version":3,"file":"a.ts","sources":["a.ts"],"names":[],"mappings":"AAAA"}"#
                .to_vec(),
        );
    assert_eq!(registry.register(&ids, meta), Err(DebugError::InvalidMap));
}

#[test]
fn native_key_reuse_and_closed_or_wrong_revision_queries_cannot_alias_live_sources() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let reg = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "x", None),
        )
        .unwrap();
    assert_eq!(
        registry
            .native_registration(NativeScriptKey::new(
                store.context(),
                NonZeroU64::new(1).unwrap()
            ))
            .unwrap(),
        reg
    );
    assert_eq!(
        registry.register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "y", None)
        ),
        Err(DebugError::InvalidSource)
    );
    assert!(matches!(
        registry.source(reg.id, reg.revision + 1, Page { start: 0, count: 1 }),
        Err(DebugError::InvalidSource)
    ));
    assert!(matches!(
        registry.source(
            reg.id,
            reg.revision,
            Page {
                start: usize::MAX,
                count: 1
            }
        ),
        Err(DebugError::InvalidRequest)
    ));
    registry.close();
    assert_eq!(
        registry.register(
            &ids,
            metadata(store.context(), 2, "file:///out.js", "z", None)
        ),
        Err(DebugError::Closed)
    );
    assert!(matches!(
        registry.sources(Page { start: 0, count: 1 }),
        Err(DebugError::Closed)
    ));
}

#[test]
fn mapping_all_originals_and_original_text_pages_preserve_content_identity() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map = r#"{"version":3,"sources":["a.ts","b.ts"],"sourcesContent":["a","b"],"names":[],"mappings":"AAAA,ACAA"}"#;
    let reg = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "x", Some(map)),
        )
        .unwrap();
    let originals = registry
        .map_locations(&DebugLocation {
            script: reg.id,
            revision: reg.revision,
            line: 0,
            column: Some(0),
        })
        .unwrap();
    assert_eq!(originals.len(), 2);
    assert_eq!(originals[0].origin, "file:///a.ts");
    match registry
        .original_source(&selector("file:///a.ts"), Page { start: 0, count: 1 })
        .unwrap()
        .unwrap()
    {
        DebugReply::Source { text, total_units } => {
            assert_eq!(text.to_utf8().unwrap(), "a");
            assert_eq!(total_units, 1);
        }
        _ => panic!(),
    }
    assert!(
        registry
            .original_source(
                &selector("file:///unavailable.ts"),
                Page { start: 0, count: 1 }
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn equivalent_map_json_is_not_a_conflicting_source_revision() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map = r#"{"version":3,"sources":["a.ts"],"names":[],"mappings":"AAAA"}"#;
    let reg = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "x", Some(map)),
        )
        .unwrap();
    let alternate = r#"{ "mappings": "AAAA", "names": [], "sources": ["a.ts"], "version": 3 }"#;
    assert_eq!(
        registry
            .register(
                &ids,
                metadata(store.context(), 2, "file:///out.js", "x", Some(alternate))
            )
            .unwrap(),
        reg
    );
}

#[test]
fn indexed_unmapped_prefix_and_explicit_unmapped_segment_do_not_inherit_prior_mapping() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map = r#"{"version":3,"sections":[{"offset":{"line":0,"column":0},"map":{"version":3,"sources":["a.ts"],"names":[],"mappings":"AAAA"}},{"offset":{"line":0,"column":2},"map":{"version":3,"sources":["b.ts"],"names":[],"mappings":"CAAA,C"}}]}"#;
    let reg = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "abcde", Some(map)),
        )
        .unwrap();
    for column in [2, 4] {
        assert!(
            registry
                .map_location(&DebugLocation {
                    script: reg.id,
                    revision: reg.revision,
                    line: 0,
                    column: Some(column)
                })
                .unwrap()
                .is_none()
        );
    }
    let mapped = registry
        .map_location(&DebugLocation {
            script: reg.id,
            revision: reg.revision,
            line: 0,
            column: Some(3),
        })
        .unwrap()
        .unwrap();
    assert_eq!(mapped.origin, "file:///b.ts");
}

#[test]
fn original_digest_can_target_retained_older_generated_revision() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let a = r#"{"version":3,"sources":["a.ts"],"sourcesContent":["a"],"mappings":"AAAA"}"#;
    let b = a.replace("[\"a\"]", "[\"b\"]");
    let first = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "x", Some(a)),
        )
        .unwrap();
    registry
        .register(
            &ids,
            metadata(store.context(), 2, "file:///out.js", "y", Some(&b)),
        )
        .unwrap();
    let expected = coldmind_runtime::engine::SourceDigest::of_utf16(&JsString::from_utf8("a"));
    let locations = registry
        .resolve_breakpoint(
            &ScriptSelector {
                origin: "file:///a.ts".to_owned(),
                expected_digest: Some(expected),
            },
            0,
            Some(0),
        )
        .unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].script, first.id);
}

#[test]
fn signed_generated_deltas_and_partial_sources_content_follow_v3_decoding() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    // Generated col 2 -> original col 2, then signed delta -1 -> original col 1.
    let map = r#"{"version":3,"sources":["a.ts","missing.ts"],"sourcesContent":["abc"],"mappings":"EAAE,DAAD"}"#;
    let reg = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "abc", Some(map)),
        )
        .unwrap();
    let location = registry
        .map_location(&DebugLocation {
            script: reg.id,
            revision: reg.revision,
            line: 0,
            column: Some(1),
        })
        .unwrap()
        .unwrap();
    assert_eq!(location.column, 1);
}

#[test]
fn crlf_and_unicode_line_terminators_are_counted_as_native_source_lines() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map = r#"{"version":3,"sources":["a.ts"],"sourcesContent":["a\r\nb\u2028c\u2029d"],"mappings":"AAAA;AACA;AACA;AACA"}"#;
    let reg = registry
        .register(
            &ids,
            metadata(
                store.context(),
                1,
                "file:///out.js",
                "a\r\nb\u{2028}c\u{2029}d",
                Some(map),
            ),
        )
        .unwrap();
    let location = registry
        .map_location(&DebugLocation {
            script: reg.id,
            revision: reg.revision,
            line: 3,
            column: Some(0),
        })
        .unwrap()
        .unwrap();
    assert_eq!(location.line, 3);
    assert!(matches!(
        registry.map_location(&DebugLocation {
            script: reg.id,
            revision: reg.revision,
            line: 4,
            column: Some(0)
        }),
        Err(DebugError::InvalidSource)
    ));
}

#[test]
fn source_root_keeps_custom_scheme_authority_slashes() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map =
        r#"{"version":3,"sourceRoot":"webpack:///","sources":["src/a.ts"],"mappings":"AAAA"}"#;
    let reg = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "x", Some(map)),
        )
        .unwrap();
    let original = registry
        .map_location(&DebugLocation {
            script: reg.id,
            revision: reg.revision,
            line: 0,
            column: Some(0),
        })
        .unwrap()
        .unwrap();
    assert_eq!(original.origin, "webpack:///src/a.ts");
}

#[test]
fn unused_source_map_chain_cycles_are_rejected_before_admission() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map = r#"{"version":3,"sources":["outside.ts","a.js"],"mappings":"AAAA"}"#;
    let mut meta = metadata(store.context(), 1, "file:///out.js", "x", Some(map));
    meta.source.source_map.as_mut().unwrap().preceding_maps = vec![
        br#"{"version":3,"file":"a.js","sources":["b.js"],"mappings":"AAAA"}"#.to_vec(),
        br#"{"version":3,"file":"b.js","sources":["a.js"],"mappings":"AAAA"}"#.to_vec(),
    ];
    assert_eq!(registry.register(&ids, meta), Err(DebugError::InvalidMap));
}

#[test]
fn a_foreign_metadata_issuer_cannot_rebind_an_existing_source_revision() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "x", None),
        )
        .unwrap();
    let foreign = DebugIdentities::new(NonZeroU64::new(2).unwrap(), store.context(), 16).unwrap();
    assert_eq!(
        registry.register(
            &foreign,
            metadata(store.context(), 2, "file:///out.js", "x", None)
        ),
        Err(DebugError::InvalidSource)
    );
    ids.close();
    assert_eq!(
        registry.register(
            &ids,
            metadata(store.context(), 3, "file:///out.js", "x", None)
        ),
        Err(DebugError::Closed)
    );
}

#[test]
fn json_escaped_lone_surrogates_in_original_content_remain_lossless_utf16() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map =
        r#"{"version":3,"sources":["a.ts"],"sourcesContent":["\ud800x\udc00"],"mappings":"AAAA"}"#;
    registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "x", Some(map)),
        )
        .unwrap();
    match registry
        .original_source(&selector("file:///a.ts"), Page { start: 0, count: 3 })
        .unwrap()
        .unwrap()
    {
        DebugReply::Source { text, total_units } => {
            assert_eq!(text.as_units(), &[0xd800, 0x78, 0xdc00]);
            assert_eq!(total_units, 3);
        }
        _ => panic!(),
    }
}

#[test]
fn source_map_symbol_names_are_lossless_json_strings_even_when_not_displayed() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map = r#"{"version":3,"sources":["a.ts"],"names":["\ud800"],"mappings":"AAAAA"}"#;
    let reg = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "x", Some(map)),
        )
        .unwrap();
    assert_eq!(
        registry
            .resolve_breakpoint(&selector("file:///a.ts"), 0, None)
            .unwrap()[0]
            .script,
        reg.id
    );
}

#[test]
fn nested_index_maps_and_two_preceding_transformations_compose_exact_coordinates() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map = r#"{"version":3,"sections":[{"offset":{"line":1,"column":2},"map":{"version":3,"sections":[{"offset":{"line":0,"column":1},"map":{"version":3,"sources":["middle.js"],"sourcesContent":["x"],"mappings":"AAAA"}}]}}]}"#;
    let mut meta = metadata(store.context(), 1, "file:///out.js", "\n   x", Some(map));
    meta.source.source_map.as_mut().unwrap().preceding_maps = vec![
        br#"{"version":3,"file":"middle.js","sources":["earlier.js"],"sourcesContent":[" x"],"mappings":"AAAC"}"#.to_vec(),
        br#"{"version":3,"file":"earlier.js","sources":["original.ts"],"sourcesContent":["  x"],"mappings":"CAAE"}"#.to_vec(),
    ];
    let reg = registry.register(&ids, meta).unwrap();
    let location = registry
        .map_location(&DebugLocation {
            script: reg.id,
            revision: reg.revision,
            line: 1,
            column: Some(3),
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        (location.origin.as_str(), location.line, location.column),
        ("file:///original.ts", 0, 2)
    );
    let reverse = registry
        .resolve_breakpoint(&selector("file:///original.ts"), 0, Some(2))
        .unwrap();
    assert_eq!((reverse[0].line, reverse[0].column), (1, Some(3)));
}

#[test]
fn supplied_root_map_file_cannot_claim_another_generated_source() {
    let (store, ids) = fixture();
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let map = r#"{"version":3,"file":"other.js","sources":["a.ts"],"mappings":"AAAA"}"#;
    assert_eq!(
        registry.register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "x", Some(map))
        ),
        Err(DebugError::InvalidMap)
    );
}

#[test]
fn pure_source_pre_admission_uses_the_same_parser_without_consuming_any_identity() {
    let (store, ids) = fixture();
    let invalid = ScriptSource {
        text: JsString::from_utf8("x"),
        origin: "file:///out.js".to_owned(),
        source_map: Some(SourceMapAttachment {
            json: br#"{"version":3,"sources":["a.ts"],"mappings":"!AAA"}"#.to_vec(),
            originals: vec![],
            preceding_maps: vec![],
        }),
    };
    assert_eq!(
        coldmind_runtime::debug::validate_source(&invalid, &DebugLimits::default()),
        Err(DebugError::InvalidMap)
    );
    let mut registry = SourceRegistry::new(store.context(), DebugLimits::default()).unwrap();
    let accepted = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "x", None),
        )
        .unwrap();
    assert_eq!(
        registry.select(&selector("file:///out.js")).unwrap(),
        Some(accepted)
    );
    assert_eq!(
        coldmind_runtime::debug::validate_source(
            &ScriptSource::new(
                JsString::from_utf8("x".repeat(100).as_str()),
                "file:///large.js".to_owned()
            ),
            &DebugLimits {
                source_bytes: 2,
                ..DebugLimits::default()
            }
        ),
        Err(DebugError::Capacity)
    );
}
