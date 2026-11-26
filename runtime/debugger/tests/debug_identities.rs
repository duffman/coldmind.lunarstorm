//! Debugger reference identities across sessions: independent issuers, pause
//! references that expire, shared cursors, and invalidation when a context closes.

use coldmind_runtime::{debug::*, engine::*};
use std::num::NonZeroU64;

#[test]
fn independent_issuers_for_the_same_context_cannot_forge_each_others_ids() {
    let engine = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    let vm = NonZeroU64::new(1).unwrap();
    let first = DebugIdentities::new(vm, store.context(), 1).unwrap();
    let second = DebugIdentities::new(vm, store.context(), 1).unwrap();
    let a = first.open_session().unwrap();
    let b = second.open_session().unwrap();
    assert_ne!(
        a, b,
        "context identity alone must not authorize a different issuer"
    );
    let pause = second.begin_pause(b).unwrap();
    let object = second
        .register(pause, ReferenceKind::Object, NonZeroU64::new(1).unwrap())
        .unwrap();
    assert!(matches!(
        first.resolve(object),
        Err(DebugError::StaleSession)
    ));
}

#[test]
fn pause_references_expire_and_cannot_resolve_in_a_new_session() {
    let engine = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    let ids = DebugIdentities::new(NonZeroU64::new(7).unwrap(), store.context(), 8).unwrap();
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    let frame = ids
        .register(pause, ReferenceKind::Frame, NonZeroU64::new(4).unwrap())
        .unwrap();
    assert_eq!(ids.resolve(frame).unwrap().get(), 4);
    ids.end_pause(pause).unwrap();
    assert!(matches!(ids.resolve(frame), Err(DebugError::StalePause)));
    let second = ids.begin_pause(session).unwrap();
    assert_ne!(pause, second);
    assert!(matches!(ids.resolve(frame), Err(DebugError::StalePause)));
    ids.close_session(session).unwrap();
    let new_session = ids.open_session().unwrap();
    assert_ne!(session, new_session);
    assert!(matches!(ids.resolve(frame), Err(DebugError::StaleSession)));
}

#[test]
fn duplicate_references_share_capacity_and_exhaustion_does_not_block_resume() {
    let engine = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    let ids = DebugIdentities::new(NonZeroU64::new(1).unwrap(), store.context(), 1).unwrap();
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    let key = NonZeroU64::new(9).unwrap();
    let first = ids.register(pause, ReferenceKind::Object, key).unwrap();
    assert_eq!(
        ids.register(pause, ReferenceKind::Object, key).unwrap(),
        first
    );
    assert!(matches!(
        ids.register(pause, ReferenceKind::Scope, key),
        Err(DebugError::Capacity)
    ));
    ids.end_pause(pause).unwrap();
    let next = ids.begin_pause(session).unwrap();
    assert!(ids.register(next, ReferenceKind::Scope, key).is_ok());
}

#[test]
fn context_close_invalidates_all_metadata_without_native_state() {
    let engine = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    let ids = DebugIdentities::new(NonZeroU64::new(1).unwrap(), store.context(), 2).unwrap();
    let first_script = ids.next_script().unwrap();
    assert_ne!(first_script, ids.next_script().unwrap());
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    let frame = ids
        .register(pause, ReferenceKind::Frame, NonZeroU64::new(1).unwrap())
        .unwrap();
    ids.close();
    assert!(matches!(ids.resolve(frame), Err(DebugError::Closed)));
    assert!(matches!(ids.open_session(), Err(DebugError::Closed)));
    assert!(matches!(ids.next_script(), Err(DebugError::Closed)));
}

#[test]
fn write_references_cannot_cross_scope_session_or_context() {
    let engine = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    let ids = DebugIdentities::new(NonZeroU64::new(1).unwrap(), store.context(), 8).unwrap();
    let other = DebugIdentities::new(NonZeroU64::new(1).unwrap(), store.context(), 8).unwrap();
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    other.open_session().unwrap();
    let other_store = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    let foreign =
        DebugIdentities::new(NonZeroU64::new(1).unwrap(), other_store.context(), 8).unwrap();
    foreign.open_session().unwrap();
    for kind in [ReferenceKind::Binding, ReferenceKind::Property] {
        let reference = ids
            .register(pause, kind, NonZeroU64::new(2).unwrap())
            .unwrap();
        assert_eq!(ids.resolve(reference).unwrap().get(), 2);
        assert_eq!(other.resolve(reference), Err(DebugError::StaleSession));
        assert_eq!(foreign.resolve(reference), Err(DebugError::StaleSession));
        ids.invalidate_inspection(pause).unwrap();
        assert_eq!(ids.resolve(reference), Err(DebugError::StaleInspection));
    }
    ids.end_pause(pause).unwrap();
    let next = ids.begin_pause(session).unwrap();
    let reference = ids
        .register(next, ReferenceKind::Binding, NonZeroU64::new(3).unwrap())
        .unwrap();
    ids.close_session(session).unwrap();
    ids.open_session().unwrap();
    assert_eq!(ids.resolve(reference), Err(DebugError::StaleSession));
}

#[test]
fn cursor_generation_changes_without_dropping_pause_objects() {
    let engine = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    let ids = DebugIdentities::new(NonZeroU64::new(1).unwrap(), store.context(), 2).unwrap();
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    let key = NonZeroU64::new(1).unwrap();
    let object = ids.register(pause, ReferenceKind::Object, key).unwrap();
    let cursor = ids.register(pause, ReferenceKind::Cursor, key).unwrap();
    for _ in 0..32 {
        ids.invalidate_inspection(pause).unwrap();
        assert_eq!(ids.resolve(cursor), Err(DebugError::StaleInspection));
        assert_eq!(ids.resolve(object), Ok(key));
        let next = ids.register(pause, ReferenceKind::Cursor, key).unwrap();
        assert_ne!(next, cursor);
        assert_eq!(ids.resolve(next), Ok(key));
    }
}

#[test]
fn completed_cursor_reuse_does_not_revive_or_release_replacement() {
    let engine = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    let ids = DebugIdentities::new(NonZeroU64::new(1).unwrap(), store.context(), 1).unwrap();
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    let key = NonZeroU64::new(1).unwrap();
    let DebugReference::Cursor(old) = ids.register(pause, ReferenceKind::Cursor, key).unwrap()
    else {
        panic!()
    };
    ids.release_cursor(old).unwrap();
    let DebugReference::Cursor(new) = ids.register(pause, ReferenceKind::Cursor, key).unwrap()
    else {
        panic!()
    };
    assert_ne!(old, new);
    assert_eq!(
        ids.resolve(DebugReference::Cursor(old)),
        Err(DebugError::StaleInspection)
    );
    assert_eq!(ids.release_cursor(old), Err(DebugError::StaleInspection));
    assert_eq!(ids.resolve(DebugReference::Cursor(new)), Ok(key));
}
