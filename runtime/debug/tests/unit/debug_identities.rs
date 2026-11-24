use super::*;
use crate::engine::{ContextLimits, EngineIdentity, ValueStore};

fn issuer() -> DebugIdentities {
    let engine = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    DebugIdentities::new(NonZeroU64::new(7).unwrap(), store.context(), 3).unwrap()
}

#[test]
fn owner_and_transferable_request_issuers_share_one_counter_and_revocation() {
    let ids = issuer();
    let session = ids.open_session().unwrap();
    let sender = ids.request_issuer(session).unwrap();
    let worker = std::thread::spawn(move || {
        let first = sender.issue().unwrap();
        (sender, first)
    });
    let (sender, first) = worker.join().unwrap();
    let second = ids.next_request(session).unwrap();
    assert_ne!(first, second);
    ids.close_session(session).unwrap();
    assert_eq!(sender.issue(), Err(DebugError::Closed));
    let replacement = ids.open_session().unwrap();
    assert_ne!(ids.next_request(replacement).unwrap(), first);
    assert_eq!(sender.issue(), Err(DebugError::Closed));
}

#[test]
fn issuer_epoch_exhaustion_is_sticky_without_mutating_the_global_allocator() {
    let counter = AtomicU64::new(u64::MAX - 1);
    assert_eq!(allocate_issuer_epoch(&counter).unwrap(), u64::MAX - 1);
    for _ in 0..2 {
        assert_eq!(allocate_issuer_epoch(&counter), Err(DebugError::Capacity));
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    }
}

#[test]
fn forged_reference_kind_generation_and_slot_cannot_resolve_native_keys() {
    let ids = issuer();
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    let key = NonZeroU64::new(42).unwrap();
    let valid = ids.register(pause, ReferenceKind::Frame, key).unwrap();
    for reference in [
        DebugReference::Frame(FrameRef {
            pause,
            generation: 0,
            slot: 0,
        }),
        DebugReference::Frame(FrameRef {
            pause,
            generation: 2,
            slot: 0,
        }),
        DebugReference::Frame(FrameRef {
            pause,
            generation: 1,
            slot: u64::MAX,
        }),
        DebugReference::Scope(ScopeRef {
            pause,
            generation: 1,
            slot: 0,
        }),
        DebugReference::Object(ObjectRef {
            pause,
            generation: 1,
            slot: 0,
        }),
    ] {
        assert_eq!(ids.resolve(reference), Err(DebugError::InvalidReference));
    }
    assert_eq!(ids.resolve(valid).unwrap(), key);
    assert_eq!(ids.state.borrow().references.len(), 1);
}

#[test]
fn every_session_identity_field_is_checked_before_pause_or_request_mutation() {
    let ids = issuer();
    let session = ids.open_session().unwrap();
    for field in 0..5 {
        let mut forged = session;
        match field {
            0 => forged.issuer_epoch += 1,
            1 => forged.vm_epoch += 1,
            2 => forged.engine_epoch += 1,
            3 => forged.context_generation += 1,
            _ => forged.generation += 1,
        }
        assert_eq!(ids.begin_pause(forged), Err(DebugError::StaleSession));
        assert_eq!(ids.next_request(forged), Err(DebugError::StaleSession));
        assert_eq!(ids.close_session(forged), Err(DebugError::StaleSession));
    }
    let state = ids.state.borrow();
    assert_eq!(state.session, Some(session));
    assert!(state.pause.is_none());
    assert_eq!(
        (
            state.pause_counter,
            state.request_counter.load(Ordering::Relaxed)
        ),
        (0, 0)
    );
}

#[test]
fn forged_pause_generation_or_session_never_clears_existing_references() {
    let ids = issuer();
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    let key = NonZeroU64::new(2).unwrap();
    let valid = ids.register(pause, ReferenceKind::Object, key).unwrap();
    let forged_pause = PauseId {
        generation: pause.generation + 1,
        ..pause
    };
    let mut forged_session = session;
    forged_session.generation += 1;
    let foreign_pause = PauseId {
        session: forged_session,
        ..pause
    };
    for (forged, expected) in [
        (forged_pause, DebugError::StalePause),
        (foreign_pause, DebugError::StaleSession),
    ] {
        assert_eq!(ids.end_pause(forged), Err(expected.clone()));
        assert_eq!(
            ids.register(forged, ReferenceKind::Frame, key),
            Err(expected.clone())
        );
        assert_eq!(
            ids.resolve(DebugReference::Object(ObjectRef {
                pause: forged,
                generation: 1,
                slot: 0
            })),
            Err(expected)
        );
        assert_eq!(ids.resolve(valid).unwrap(), key);
    }
    assert_eq!(ids.state(), DebugExecutionState::Paused(pause));
}

#[test]
fn all_local_counters_issue_the_final_value_then_fail_without_wrapping() {
    let ids = issuer();
    ids.state.borrow_mut().session_counter = u64::MAX - 1;
    let session = ids.open_session().unwrap();
    assert_eq!(session.generation, u64::MAX);
    ids.state.borrow_mut().pause_counter = u64::MAX - 1;
    let pause = ids.begin_pause(session).unwrap();
    assert_eq!(pause.generation, u64::MAX);
    ids.end_pause(pause).unwrap();
    assert_eq!(ids.begin_pause(session), Err(DebugError::Capacity));
    assert_eq!(ids.state(), DebugExecutionState::Running);
    {
        let mut state = ids.state.borrow_mut();
        state.script_counter = u64::MAX - 1;
        state.request_counter.store(u64::MAX - 1, Ordering::Relaxed);
        state.update_counter = u64::MAX - 1;
    }
    assert_eq!(ids.next_script().unwrap().sequence, u64::MAX);
    assert_eq!(ids.next_request(session).unwrap().sequence, u64::MAX);
    assert_eq!(ids.next_update_sequence().unwrap(), u64::MAX);
    for _ in 0..2 {
        assert_eq!(ids.next_script(), Err(DebugError::Capacity));
        assert_eq!(ids.next_request(session), Err(DebugError::Capacity));
        assert_eq!(ids.next_update_sequence(), Err(DebugError::Capacity));
    }
    ids.close_session(session).unwrap();
    assert_eq!(ids.open_session(), Err(DebugError::Capacity));
    let state = ids.state.borrow();
    assert!(state.session.is_none() && state.pause.is_none());
    assert!(state.references.is_empty());
    assert_eq!(
        [
            state.session_counter,
            state.pause_counter,
            state.script_counter,
            state.request_counter.load(Ordering::Relaxed),
            state.update_counter
        ],
        [u64::MAX; 5]
    );
}

fn assert_wrong_owner<T>(result: Result<T, DebugError>) {
    match result {
        Err(DebugError::EngineFailure(report)) => {
            assert_eq!(report.code, FailureCode::WrongOwner)
        }
        _ => panic!("foreign metadata owner must fail before mutation"),
    }
}

#[test]
fn foreign_owner_is_rejected_without_moving_owner_local_state_between_threads() {
    let mut ids = issuer();
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    let key = NonZeroU64::new(9).unwrap();
    let reference = ids.register(pause, ReferenceKind::Scope, key).unwrap();
    let owner = ids.owner;
    ids.owner = thread::spawn(|| thread::current().id()).join().unwrap();
    assert_wrong_owner(ids.open_session());
    assert_wrong_owner(ids.begin_pause(session));
    assert_wrong_owner(ids.end_pause(pause));
    assert_wrong_owner(ids.register(pause, ReferenceKind::Object, key));
    assert_wrong_owner(ids.resolve(reference));
    assert_wrong_owner(ids.next_script());
    assert_wrong_owner(ids.next_request(session));
    assert_wrong_owner(ids.next_update_sequence());
    assert_wrong_owner(ids.close_session(session));
    ids.owner = owner;
    assert_eq!(ids.resolve(reference).unwrap(), key);
    let state = ids.state.borrow();
    assert_eq!(
        (
            state.script_counter,
            state.request_counter.load(Ordering::Relaxed),
            state.update_counter
        ),
        (0, 0, 0)
    );
    assert_eq!(state.references.len(), 1);
    assert_eq!(state.pause, Some(pause));
}

#[test]
fn write_target_kind_and_inspection_generation_are_checked_before_resolution() {
    let ids = issuer();
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    let key = NonZeroU64::new(42).unwrap();
    let binding = ids.register(pause, ReferenceKind::Binding, key).unwrap();
    let DebugReference::Binding(issued) = binding else {
        panic!()
    };
    let property = DebugReference::Property(PropertyRef {
        pause,
        generation: issued.generation,
        slot: 0,
    });
    assert_eq!(ids.resolve(property), Err(DebugError::InvalidReference));
    let out_of_bounds = DebugReference::Binding(BindingRef {
        pause,
        generation: issued.generation,
        slot: u64::MAX,
    });
    assert_eq!(
        ids.resolve(out_of_bounds),
        Err(DebugError::InvalidReference)
    );
    ids.invalidate_inspection(pause).unwrap();
    assert_eq!(ids.resolve(binding), Err(DebugError::StaleInspection));
    let current = ids.register(pause, ReferenceKind::Property, key).unwrap();
    assert_eq!(ids.resolve(binding), Err(DebugError::StaleInspection));
    assert_eq!(ids.resolve(current), Ok(key));
}

#[test]
fn inspection_generation_exhaustion_never_revalidates_old_references() {
    let ids = issuer();
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    let key = NonZeroU64::new(1).unwrap();
    let object = ids.register(pause, ReferenceKind::Object, key).unwrap();
    let old = ids.register(pause, ReferenceKind::Cursor, key).unwrap();
    ids.state.borrow_mut().inspection_generation = u64::MAX - 1;
    ids.invalidate_inspection(pause).unwrap();
    let last = ids.register(pause, ReferenceKind::Cursor, key).unwrap();
    assert_eq!(ids.invalidate_inspection(pause), Err(DebugError::Capacity));
    assert_eq!(ids.resolve(last), Ok(key));
    assert_eq!(ids.resolve(old), Err(DebugError::StaleInspection));
    assert_eq!(ids.resolve(object), Ok(key));
}

#[test]
fn write_and_inspection_live_quotas_are_independent_and_reuse_retired_slots() {
    let engine = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    let ids = DebugIdentities::new_with_targets(NonZeroU64::new(7).unwrap(), store.context(), 2, 1)
        .unwrap();
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    let key = |i| NonZeroU64::new(i).unwrap();
    let target = ids
        .register(pause, ReferenceKind::Property, key(1))
        .unwrap();
    ids.register(pause, ReferenceKind::Frame, key(2)).unwrap();
    let cursor = ids.register(pause, ReferenceKind::Cursor, key(3)).unwrap();
    assert_eq!(
        ids.register(pause, ReferenceKind::Binding, key(4)),
        Err(DebugError::Capacity)
    );
    assert_eq!(
        ids.register(pause, ReferenceKind::Scope, key(5)),
        Err(DebugError::Capacity)
    );
    ids.release_target(target).unwrap();
    let successor = ids.register(pause, ReferenceKind::Binding, key(4)).unwrap();
    assert_eq!(ids.target_for_release(target), Ok(None));
    assert!(ids.resolve(successor).is_ok());
    assert!(ids.resolve(cursor).is_ok());
    assert_eq!(ids.state.borrow().references.len(), 3);
    let DebugReference::Binding(binding) = successor else {
        panic!()
    };
    for generation in [0, 1, u64::MAX] {
        let forged = DebugReference::Binding(BindingRef {
            generation,
            ..binding
        });
        assert_eq!(
            ids.target_for_release(forged),
            Err(DebugError::InvalidReference)
        );
    }
    let wrong_kind = DebugReference::Property(PropertyRef {
        pause: binding.pause,
        slot: binding.slot,
        generation: binding.generation,
    });
    assert_eq!(
        ids.target_for_release(wrong_kind),
        Err(DebugError::InvalidReference)
    );
}
