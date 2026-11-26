//! Registry admission/retention boundaries use real identities and owned source buffers.
use super::*;
use crate::{
    debug::ScriptKind,
    engine::{ContextLimits, EngineIdentity, ValueStore},
};
use std::num::NonZeroU64;

fn fixture(limits: DebugLimits) -> (ValueStore<()>, DebugIdentities, SourceRegistry) {
    let engine = EngineIdentity::new().unwrap();
    let store = ValueStore::new(&engine, ContextLimits::default()).unwrap();
    let identities =
        DebugIdentities::new(NonZeroU64::new(1).unwrap(), store.context(), 16).unwrap();
    let registry = SourceRegistry::new(store.context(), limits).unwrap();
    (store, identities, registry)
}
fn metadata(
    context: ContextId,
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
fn zero_registry_budgets_and_a_foreign_reported_owner_reject_before_mutation() {
    let (store, _, mut registry) = fixture(DebugLimits::default());
    for field in 0..3 {
        let mut limits = DebugLimits::default();
        match field {
            0 => limits.scripts = 0,
            1 => limits.source_bytes = 0,
            _ => limits.reply_bytes = 0,
        }
        assert!(matches!(
            SourceRegistry::new(store.context(), limits),
            Err(DebugError::Capacity)
        ));
    }
    registry.owner = std::thread::spawn(|| std::thread::current().id())
        .join()
        .unwrap();
    assert!(matches!(
        registry.sources(Page { start: 0, count: 1 }),
        Err(DebugError::InvalidSource)
    ));
    assert_eq!(registry.retained_bytes(), 0);
    assert!(registry.records.is_empty() && registry.bindings.is_empty());
}

#[test]
fn decoded_map_storage_counts_against_pre_admission_and_registry_budgets() {
    let map = r#"{"version":3,"sources":["a.ts"],"sourcesContent":["a"],"mappings":"AAAA"}"#;
    let (store, ids, _) = fixture(DebugLimits::default());
    let candidate = metadata(store.context(), 1, "file:///out.js", "x", Some(map));
    let raw_bytes = candidate.source.text.capacity_bytes()
        + candidate.source.origin.capacity()
        + candidate
            .source
            .source_map
            .as_ref()
            .unwrap()
            .json
            .capacity();
    let limits = DebugLimits {
        source_bytes: raw_bytes,
        ..Default::default()
    };
    assert_eq!(
        validate_source(&candidate.source, &limits),
        Err(DebugError::Capacity)
    );
    let limits = DebugLimits {
        source_bytes: raw_bytes + size_of::<(NativeScriptKey, ScriptRegistration)>(),
        ..Default::default()
    };
    let mut registry = SourceRegistry::new(store.context(), limits).unwrap();
    assert_eq!(
        registry.register(&ids, candidate),
        Err(DebugError::Capacity)
    );
    assert_eq!(registry.retained_bytes(), 0);
    assert!(registry.records.is_empty() && registry.bindings.is_empty());
    let registration = registry
        .register(&ids, metadata(store.context(), 1, "x", "x", None))
        .unwrap();
    assert_eq!(registration.revision, 1);
}

#[test]
fn repeated_native_key_reuses_storage_but_an_over_budget_alias_is_transactionally_rejected() {
    let limits = DebugLimits {
        source_bytes: 2 * size_of::<(NativeScriptKey, ScriptRegistration)>() + 2,
        ..Default::default()
    };
    let (store, ids, mut registry) = fixture(limits);
    let first = registry
        .register(&ids, metadata(store.context(), 1, "x", "x", None))
        .unwrap();
    let bytes = registry.retained_bytes();
    assert_eq!(
        registry
            .register(&ids, metadata(store.context(), 1, "x", "x", None))
            .unwrap(),
        first
    );
    assert_eq!(registry.retained_bytes(), bytes);
    assert_eq!(
        registry.register(&ids, metadata(store.context(), 2, "x", "x", None)),
        Err(DebugError::Capacity)
    );
    assert_eq!(registry.retained_bytes(), bytes);
    assert_eq!(registry.bindings.len(), 1);
    assert_eq!(
        registry
            .native_registration(NativeScriptKey::new(
                store.context(),
                NonZeroU64::new(1).unwrap()
            ))
            .unwrap(),
        first
    );
}

#[test]
fn final_source_revision_rejects_new_content_without_wrapping_or_consuming_a_binding() {
    let (store, ids, mut registry) = fixture(DebugLimits::default());
    let first = registry
        .register(&ids, metadata(store.context(), 1, "x", "a", None))
        .unwrap();
    registry.records[0].registration.revision = u64::MAX;
    registry.bindings[0].1.revision = u64::MAX;
    let bytes = registry.retained_bytes();
    assert_eq!(
        registry.register(&ids, metadata(store.context(), 2, "x", "b", None)),
        Err(DebugError::Capacity)
    );
    assert_eq!(registry.records.len(), 1);
    assert_eq!(registry.bindings.len(), 1);
    assert_eq!(registry.retained_bytes(), bytes);
    let retained = registry.select(&selector("x")).unwrap().unwrap();
    assert_eq!(
        (retained.id, retained.revision, retained.digest),
        (first.id, u64::MAX, first.digest)
    );
}

#[test]
fn either_fallible_registration_allocation_can_fail_without_retaining_a_partial_source() {
    for allocation in 0..2 {
        let (store, ids, mut registry) = fixture(DebugLimits::default());
        let candidate = metadata(store.context(), 1, "x", "x", None);
        let (result, fired) =
            crate::vm::test_allocator::fail_nth(allocation, || registry.register(&ids, candidate));
        assert!(fired);
        assert_eq!(result, Err(DebugError::Capacity));
        assert_eq!(registry.retained_bytes(), 0);
        assert!(registry.records.is_empty() && registry.bindings.is_empty());
        assert_eq!(
            registry
                .register(&ids, metadata(store.context(), 1, "x", "x", None))
                .unwrap()
                .revision,
            1
        );
    }
}

#[test]
fn source_and_original_pages_enforce_reply_capacity_before_copying() {
    let limits = DebugLimits {
        reply_bytes: size_of::<DebugReply>() + 2,
        ..Default::default()
    };
    let (store, ids, mut registry) = fixture(limits);
    let map = r#"{"version":3,"sources":["a.ts","missing.ts"],"sourcesContent":["abc",null],"mappings":"AAAA"}"#;
    let registration = registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "abc", Some(map)),
        )
        .unwrap();
    registry
        .register(
            &ids,
            metadata(store.context(), 2, "file:///plain.js", "x", None),
        )
        .unwrap();
    assert!(matches!(
        registry.sources(Page { start: 0, count: 1 }),
        Err(DebugError::Capacity)
    ));
    assert!(matches!(
        registry.source(
            registration.id,
            registration.revision,
            Page { start: 0, count: 2 }
        ),
        Err(DebugError::Capacity)
    ));
    assert!(matches!(
        registry.original_source(&selector("file:///a.ts"), Page { start: 0, count: 2 }),
        Err(DebugError::Capacity)
    ));
    assert!(
        matches!(registry.source(registration.id, registration.revision, Page { start: 0, count: 1 }).unwrap(), DebugReply::Source { text, total_units: 3 } if text.as_units() == [b'a' as u16])
    );
    assert!(
        matches!(registry.original_source(&selector("file:///a.ts"), Page { start: 0, count: 1 }).unwrap(), Some(DebugReply::Source { text, total_units: 3 }) if text.as_units() == [b'a' as u16])
    );
    assert!(
        registry
            .original_source(&selector("file:///missing.ts"), Page { start: 0, count: 1 })
            .unwrap()
            .is_none()
    );
    let wrong_digest = ScriptSelector {
        origin: "file:///a.ts".into(),
        expected_digest: Some(SourceDigest::of_utf16(&JsString::from_utf8("other"))),
    };
    assert!(
        registry
            .original_source(&wrong_digest, Page { start: 0, count: 1 })
            .unwrap()
            .is_none()
    );
    assert!(
        registry
            .map_locations(&DebugLocation {
                script: registration.id,
                revision: registration.revision,
                line: 0,
                column: None
            })
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        registry.original_source(
            &selector("file:///a.ts"),
            Page {
                start: 1,
                count: usize::MAX
            }
        ),
        Err(DebugError::InvalidRequest)
    ));
}

#[test]
fn original_breakpoints_ignore_superseded_revisions_and_choose_the_nearest_original_column() {
    let (store, ids, mut registry) = fixture(DebugLimits::default());
    let old = r#"{"version":3,"sources":["a.ts"],"sourcesContent":["abc"],"mappings":"AAAA"}"#;
    registry
        .register(
            &ids,
            metadata(store.context(), 1, "file:///out.js", "old", Some(old)),
        )
        .unwrap();
    // Generated columns0,1,2 map to original columns2,1,2. The nearest
    // original column clears the earlier candidate and excludes later ones.
    let next =
        r#"{"version":3,"sources":["a.ts"],"sourcesContent":["abc"],"mappings":"AAAE,CAAD,CAAC"}"#;
    let latest = registry
        .register(
            &ids,
            metadata(store.context(), 2, "file:///out.js", "new", Some(next)),
        )
        .unwrap();
    registry
        .register(
            &ids,
            metadata(store.context(), 3, "file:///plain.js", "plain", None),
        )
        .unwrap();
    let resolved = registry
        .resolve_breakpoint(&selector("file:///a.ts"), 0, Some(0))
        .unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(
        (
            resolved[0].script,
            resolved[0].revision,
            resolved[0].line,
            resolved[0].column
        ),
        (latest.id, latest.revision, 0, Some(1))
    );
    assert!(
        registry
            .resolve_breakpoint(&selector("file:///never.ts"), 0, None)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn attachment_aliases_compare_supplied_originals_and_each_preceding_map_semantically() {
    let (store, ids, mut registry) = fixture(DebugLimits::default());
    let mut candidate = metadata(
        store.context(),
        1,
        "file:///out.js",
        "x",
        Some(r#"{"version":3,"sources":["middle.js"],"mappings":"AAAA"}"#),
    );
    let attachment = candidate.source.source_map.as_mut().unwrap();
    attachment
        .preceding_maps
        .push(br#"{"version":3,"file":"middle.js","sources":["a.ts"],"mappings":"AAAA"}"#.to_vec());
    attachment.originals.push(OriginalSource {
        origin: "file:///a.ts".into(),
        text: Some(JsString::from_utf8("a")),
    });
    let source = candidate.source.clone();
    let first = registry.register(&ids, candidate).unwrap();
    let mut equivalent = source.clone();
    equivalent.source_map.as_mut().unwrap().preceding_maps[0] =
        br#"{ "mappings":"AAAA", "sources":["a.ts"], "file":"middle.js", "version":3 }"#.to_vec();
    assert_eq!(
        registry
            .register(
                &ids,
                NativeScriptMetadata {
                    key: NativeScriptKey::new(store.context(), NonZeroU64::new(2).unwrap()),
                    source: equivalent,
                    kind: ScriptKind::Script
                }
            )
            .unwrap(),
        first
    );
    for fault in 0..5 {
        let mut altered = source.clone();
        let attachment = altered.source_map.as_mut().unwrap();
        match fault {
            0 => attachment.originals.clear(),
            1 => attachment.originals[0].origin = "file:///other.ts".into(),
            2 => attachment.originals[0].text = None,
            3 => attachment.preceding_maps[0] = b"invalid JSON".to_vec(),
            _ => attachment.preceding_maps.clear(),
        }
        assert_eq!(
            registry.register(
                &ids,
                NativeScriptMetadata {
                    key: NativeScriptKey::new(store.context(), NonZeroU64::new(3).unwrap()),
                    source: altered,
                    kind: ScriptKind::Script
                }
            ),
            Err(DebugError::InvalidMap)
        );
    }
    assert_eq!(registry.records.len(), 1);
    assert_eq!(registry.bindings.len(), 2);
}

#[test]
fn valid_pre_admission_generated_breakpoints_metadata_pages_and_digest_selection_agree() {
    let (store, ids, mut registry) = fixture(DebugLimits::default());
    let source = metadata(
        store.context(),
        1,
        "file:///out.js",
        "x",
        Some(r#"{"version":3,"sources":["a.ts"],"sourcesContent":["a"],"mappings":"AAAA"}"#),
    );
    validate_source(&source.source, &DebugLimits::default()).unwrap();
    validate_source(
        &ScriptSource::new(JsString::from_utf8("plain"), "file:///plain.js".into()),
        &DebugLimits::default(),
    )
    .unwrap();
    let registration = registry.register(&ids, source).unwrap();
    let exact = ScriptSelector {
        origin: "file:///out.js".into(),
        expected_digest: Some(registration.digest),
    };
    assert_eq!(registry.select(&exact).unwrap(), Some(registration));
    let generated = registry.resolve_breakpoint(&exact, 0, Some(1)).unwrap();
    assert_eq!(generated.len(), 1);
    assert_eq!(
        (
            generated[0].script,
            generated[0].revision,
            generated[0].line,
            generated[0].column
        ),
        (registration.id, 1, 0, Some(1))
    );
    assert!(matches!(
        registry.resolve_breakpoint(&exact, 0, Some(2)),
        Err(DebugError::InvalidSource)
    ));
    let wrong = ScriptSelector {
        origin: exact.origin.clone(),
        expected_digest: Some(SourceDigest::of_utf16(&JsString::from_utf8("other"))),
    };
    assert!(registry.select(&wrong).unwrap().is_none());
    match registry.sources(Page { start: 0, count: 1 }).unwrap() {
        DebugReply::Sources { scripts, total } => {
            assert_eq!(total, 1);
            assert_eq!(scripts.len(), 1);
            assert_eq!(scripts[0].registration, registration);
            assert_eq!(scripts[0].origin, "file:///out.js");
            assert_eq!(scripts[0].length_units, 1);
        }
        _ => panic!("metadata page must preserve its reply kind"),
    }
    let mapped = registry
        .map_location(&DebugLocation {
            script: registration.id,
            revision: registration.revision,
            line: 0,
            column: Some(0),
        })
        .unwrap()
        .unwrap();
    assert_eq!(mapped.origin, "file:///a.ts");
    registry.close();
    registry.close();
    assert_eq!(registry.retained_bytes(), 0);
}

#[test]
fn a_foreign_native_key_cannot_admit_source_into_the_current_context() {
    let (store, ids, mut registry) = fixture(DebugLimits::default());
    let (foreign, _, _) = fixture(DebugLimits::default());
    assert_eq!(
        registry.register(
            &ids,
            metadata(foreign.context(), 1, "file:///out.js", "x", None)
        ),
        Err(DebugError::InvalidSource)
    );
    assert_eq!(registry.retained_bytes(), 0);
    assert!(registry.records.is_empty() && registry.bindings.is_empty());
    assert!(
        registry
            .register(
                &ids,
                metadata(store.context(), 1, "file:///out.js", "x", None)
            )
            .is_ok()
    );
}

// The same public contract cases also run inside the library's cfg(test) build,
// so their counters combine with private guard tests in one implementation.
#[path = "../debug_sources.rs"]
mod public_contracts;

#[test]
fn retained_byte_counter_underflow_or_overflow_cannot_commit_a_new_alias() {
    let (store, ids, mut registry) = fixture(DebugLimits::default());
    let first = registry
        .register(&ids, metadata(store.context(), 1, "x", "x", None))
        .unwrap();
    let actual_bytes = registry.retained_bytes();
    // Exercise the checked counter boundary without forging a container length
    // or capacity; rejection must leave every owned record/binding unchanged.
    for boundary in [0, usize::MAX] {
        registry.bytes = boundary;
        assert_eq!(
            registry.register(&ids, metadata(store.context(), 2, "x", "x", None)),
            Err(DebugError::Capacity)
        );
        assert_eq!(registry.bytes, boundary);
        assert_eq!(registry.records.len(), 1);
        assert_eq!(registry.bindings.len(), 1);
        assert_eq!(
            registry
                .native_registration(NativeScriptKey::new(
                    store.context(),
                    NonZeroU64::new(1).unwrap()
                ))
                .unwrap(),
            first
        );
    }
    registry.bytes = actual_bytes;
    assert_eq!(registry.retained_bytes(), actual_bytes);
}

#[test]
fn source_allocation_arithmetic_and_foreign_script_queries_fail_without_copying() {
    assert_eq!(allocation_bytes(usize::MAX, 2), Err(DebugError::Capacity));
    assert_eq!(allocation_bytes(3, 2), Ok(6));
    let (store, ids, mut registry) = fixture(DebugLimits::default());
    let registration = registry
        .register(&ids, metadata(store.context(), 1, "x", "x", None))
        .unwrap();
    for field in 0..2 {
        let mut foreign = registration.id;
        match field {
            0 => foreign.engine_epoch += 1,
            _ => foreign.context_generation += 1,
        }
        assert!(matches!(
            registry.source(foreign, registration.revision, Page { start: 0, count: 1 }),
            Err(DebugError::InvalidSource)
        ));
    }
    assert!(matches!(
        registry.source(
            registration.id,
            registration.revision,
            Page { start: 0, count: 1 }
        ),
        Ok(DebugReply::Source { total_units: 1, .. })
    ));
}
