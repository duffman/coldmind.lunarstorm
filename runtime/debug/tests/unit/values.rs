use super::*;

fn store(capacity: usize) -> ValueStore<i32> {
    ValueStore::new(
        &EngineIdentity::new().unwrap(),
        ContextLimits {
            max_persistent_roots: capacity,
            ..Default::default()
        },
    )
    .unwrap()
}

fn token(context: ContextId, slot: usize, generation: u64) -> RootedValue {
    // Forged metadata must not enqueue a release for storage it never owned.
    let release: Weak<dyn ReleaseQueue> = Weak::<StoreState<i32>>::new();
    RootedValue {
        lease: Rc::new(RootLease {
            context,
            key: RootKey { slot, generation },
            release,
        }),
    }
}

#[test]
fn forged_scoped_context_scope_and_slot_fail_before_retention() {
    let store = store(1);
    let foreign = EngineIdentity::new().unwrap().context().unwrap();
    store
        .with_scope(|scope| {
            let valid = scope.insert(42)?;
            for (forged, code) in [
                (
                    ScopedValue {
                        context: foreign,
                        ..valid
                    },
                    FailureCode::WrongContext,
                ),
                (
                    ScopedValue {
                        scope: scope.id + 1,
                        ..valid
                    },
                    FailureCode::InvalidHandle,
                ),
                (
                    ScopedValue {
                        slot: usize::MAX,
                        ..valid
                    },
                    FailureCode::InvalidHandle,
                ),
            ] {
                assert_eq!(scope.read(forged).unwrap_err().code, code);
                assert_eq!(scope.retain(forged).unwrap_err().code, code);
                assert_eq!(store.live_roots(), 0);
                assert_eq!(scope.values.borrow().len(), 1);
            }
            assert_eq!(*scope.read(valid)?, 42);
            Ok(())
        })
        .unwrap();
}

#[test]
fn forged_root_context_slot_generation_and_released_slot_fail_without_mutation() {
    let store = store(1);
    let foreign = EngineIdentity::new().unwrap().context().unwrap();
    store
        .with_scope(|scope| {
            for (root, code) in [
                (token(foreign, 0, 1), FailureCode::WrongContext),
                (
                    token(store.context(), usize::MAX, 1),
                    FailureCode::InvalidHandle,
                ),
                (token(store.context(), 0, 2), FailureCode::InvalidHandle),
                (token(store.context(), 0, 1), FailureCode::InvalidHandle),
            ] {
                assert_eq!(scope.local(&root).unwrap_err().code, code);
                assert!(scope.values.borrow().is_empty());
                assert_eq!(store.live_roots(), 0);
            }
            Ok(())
        })
        .unwrap();
    assert!(store.state.release.borrow().is_empty());
}

#[test]
fn stale_or_duplicate_release_keys_cannot_release_a_reused_slot() {
    let store = store(1);
    let root = store
        .with_scope(|scope| scope.retain(scope.insert(11)?))
        .unwrap();
    let first_key = root.lease.key;
    let stale = token(store.context(), first_key.slot, first_key.generation);
    drop(root);
    store.flush_releases().unwrap();
    let vacant_key = RootKey {
        slot: first_key.slot,
        generation: first_key.generation + 1,
    };
    // An empty slot with the current generation is also ignored, without
    // underflowing live count or adding a duplicate free slot.
    store.state.release.borrow_mut().push_back(vacant_key);
    store.flush_releases().unwrap();
    assert_eq!(store.state.roots.borrow().free.len(), 1);
    let replacement = store
        .with_scope(|scope| scope.retain(scope.insert(22)?))
        .unwrap();
    store.state.release.borrow_mut().push_back(first_key);
    store.flush_releases().unwrap();
    assert_eq!(store.live_roots(), 1);
    store
        .with_scope(|scope| {
            assert_eq!(
                scope.local(&stale).unwrap_err().code,
                FailureCode::InvalidHandle
            );
            assert_eq!(*scope.read(scope.local(&replacement)?)?, 22);
            Ok(())
        })
        .unwrap();
    drop(replacement);
    store.flush_releases().unwrap();
    assert_eq!(store.live_roots(), 0);
    assert_eq!(store.state.roots.borrow().free.len(), 1);
}

#[test]
fn final_root_generation_retires_instead_of_reusing_the_slot() {
    let store = store(1);
    store.state.roots.borrow_mut().slots[0].generation = u64::MAX;
    let root = store
        .with_scope(|scope| scope.retain(scope.insert(7)?))
        .unwrap();
    assert_eq!(root.lease.key.generation, u64::MAX);
    drop(root);
    store.flush_releases().unwrap();
    assert_eq!(store.live_roots(), 0);
    {
        let roots = store.state.roots.borrow();
        assert!(roots.free.is_empty());
        assert!(roots.slots[0].value.is_none());
        assert_eq!(roots.slots[0].generation, u64::MAX);
    }
    store
        .with_scope(|scope| {
            let value = scope.insert(9)?;
            assert_eq!(scope.retain(value).unwrap_err().code, FailureCode::Capacity);
            assert_eq!(*scope.read(value)?, 9);
            Ok(())
        })
        .unwrap();
}

#[test]
fn module_and_scope_identity_exhaustion_leave_storage_inactive_and_unchanged() {
    let store = store(1);
    store.state.module_sequence.set(u64::MAX - 1);
    assert_eq!(store.issue_module_id().unwrap().slot(), u64::MAX);
    for _ in 0..2 {
        assert_eq!(
            store.issue_module_id().unwrap_err().code,
            FailureCode::Capacity
        );
        assert_eq!(store.state.module_sequence.get(), u64::MAX);
    }
    store.state.sequence.set(u64::MAX - 1);
    let calls = Cell::new(0);
    let body = |scope: &ValueScope<'_, i32>| {
        calls.set(calls.get() + 1);
        assert_eq!(scope.id, u64::MAX);
        scope.insert(4).map(|_| ())
    };
    store.with_scope(body).unwrap();
    let frame_capacity = store.state.frames.borrow()[0].capacity();
    for _ in 0..2 {
        assert_eq!(
            store.with_scope(body).unwrap_err().code,
            FailureCode::Capacity
        );
        assert_eq!(store.state.sequence.get(), u64::MAX);
        assert_eq!(store.state.active.get(), 0);
        assert_eq!(store.state.frames.borrow().len(), 1);
        assert_eq!(store.state.frames.borrow()[0].capacity(), frame_capacity);
        assert_eq!(calls.get(), 1);
    }
}

#[test]
fn rejected_depth_does_not_consume_sequence_or_take_the_reusable_frame() {
    let store = store(1);
    let calls = Cell::new(0);
    let body = |scope: &ValueScope<'_, i32>| {
        calls.set(calls.get() + 1);
        scope.insert(5).map(|_| ())
    };
    store.with_scope(body).unwrap();
    let sequence = store.state.sequence.get();
    store.state.active.set(store.state.limits.max_host_depth);
    assert_eq!(
        store.with_scope(body).unwrap_err().code,
        FailureCode::CallbackDepth
    );
    assert_eq!(store.state.sequence.get(), sequence);
    assert_eq!(store.state.frames.borrow().len(), 1);
    assert_eq!(store.state.active.get(), store.state.limits.max_host_depth);
    assert_eq!(calls.get(), 1);
    store.state.active.set(0);
    store.with_scope(body).unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(store.state.sequence.get(), sequence + 1);
}

#[test]
fn owner_validation_rejects_store_and_scope_access_without_cross_thread_rc_transfer() {
    let mut store = store(1);
    let calls = Cell::new(0);
    let body = |_: &ValueScope<'_, i32>| {
        calls.set(calls.get() + 1);
        Ok(())
    };
    store.with_scope(body).unwrap();
    let owner = store.state.owner;
    let foreign = thread::spawn(|| thread::current().id()).join().unwrap();
    Rc::get_mut(&mut store.state).unwrap().owner = foreign;
    assert_eq!(
        store.with_scope(body).unwrap_err().code,
        FailureCode::WrongOwner
    );
    assert_eq!(
        store.issue_module_id().unwrap_err().code,
        FailureCode::WrongOwner
    );
    assert_eq!(
        store.flush_releases().unwrap_err().code,
        FailureCode::WrongOwner
    );
    assert_eq!(store.close().unwrap_err().code, FailureCode::WrongOwner);
    assert!(!store.is_closed());
    assert_eq!(calls.get(), 1);
    store.state.active.set(1);
    {
        let scope = ValueScope {
            state: &store.state,
            id: 1,
            values: RefCell::new(vec![Rc::new(7)]),
        };
        let value = ScopedValue {
            context: store.context(),
            scope: 1,
            slot: 0,
            lifetime: PhantomData,
        };
        let root = token(store.context(), 0, 1);
        assert_eq!(scope.insert(8).unwrap_err().code, FailureCode::WrongOwner);
        assert_eq!(scope.read(value).unwrap_err().code, FailureCode::WrongOwner);
        assert_eq!(
            scope.local(&root).unwrap_err().code,
            FailureCode::WrongOwner
        );
        assert_eq!(scope.values.borrow().len(), 1);
    }
    assert_eq!(store.state.active.get(), 0);
    Rc::get_mut(&mut store.state).unwrap().owner = owner;
    store.with_scope(body).unwrap();
    assert_eq!(calls.get(), 2);
    store.state.active.set(store.state.limits.max_host_depth);
    assert_eq!(
        store.with_scope(body).unwrap_err().code,
        FailureCode::CallbackDepth
    );
    store.state.active.set(0);
    store.state.sequence.set(u64::MAX);
    assert_eq!(
        store.with_scope(body).unwrap_err().code,
        FailureCode::Capacity
    );
    assert_eq!(
        calls.get(),
        2,
        "all rejected entries leave the body unentered"
    );
    store.close().unwrap();
}

#[test]
fn active_scope_blocks_close_and_release_flush_until_values_unwind() {
    let store = store(1);
    let root = store
        .with_scope(|scope| {
            let value = scope.insert(42)?;
            assert_eq!(store.close().unwrap_err().code, FailureCode::ActiveContext);
            assert_eq!(
                store.flush_releases().unwrap_err().code,
                FailureCode::ActiveContext
            );
            assert!(!store.is_closed());
            assert_eq!(*scope.read(value)?, 42);
            scope.retain(value)
        })
        .unwrap();
    assert_eq!(store.live_roots(), 1);
    drop(root);
    store.flush_releases().unwrap();
    assert_eq!(store.live_roots(), 0);
    store.close().unwrap();
    store.close().unwrap();
}

#[test]
fn closed_state_rejects_native_scope_access_before_touching_tokens() {
    let store = store(1);
    store.state.active.set(1);
    {
        let scope = ValueScope {
            state: &store.state,
            id: 1,
            values: RefCell::new(vec![Rc::new(7)]),
        };
        let value = ScopedValue {
            context: store.context(),
            scope: 1,
            slot: 0,
            lifetime: PhantomData,
        };
        let root = token(store.context(), 0, 1);
        store.state.closed.set(true);
        assert_eq!(
            scope.insert(8).unwrap_err().code,
            FailureCode::ClosedContext
        );
        assert_eq!(
            scope.read(value).unwrap_err().code,
            FailureCode::ClosedContext
        );
        assert_eq!(
            scope.local(&root).unwrap_err().code,
            FailureCode::ClosedContext
        );
        assert_eq!(scope.values.borrow().len(), 1);
    }
    assert_eq!(store.state.active.get(), 0);
}
#[test]
fn root_drop_after_close_does_not_enqueue_another_native_release() {
    let store = store(1);
    let root = store
        .with_scope(|scope| scope.retain(scope.insert(7)?))
        .unwrap();
    store.close().unwrap();
    assert_eq!(store.live_roots(), 0);
    drop(root);
    assert!(store.state.release.borrow().is_empty());
    assert_eq!(store.live_roots(), 0);
}
#[test]
fn every_preallocated_root_table_failure_returns_capacity_and_releases_partial_storage() {
    for allocation in 0..3 {
        let owner = EngineIdentity::new().unwrap();
        let limits = ContextLimits {
            max_persistent_roots: 2,
            ..Default::default()
        };
        let (result, fired) = crate::vm::test_allocator::fail_nth(allocation, || {
            ValueStore::<i32>::new(&owner, limits)
        });
        assert!(fired, "the selected real storage allocation must fail");
        assert!(matches!(
            result,
            Err(FailureReport {
                code: FailureCode::Capacity,
                ..
            })
        ));
        let healthy = ValueStore::<i32>::new(&owner, limits).unwrap();
        let root = healthy
            .with_scope(|scope| scope.retain(scope.insert(42)?))
            .unwrap();
        assert_eq!(healthy.live_roots(), 1);
        drop(root);
        healthy.flush_releases().unwrap();
        assert_eq!(healthy.live_roots(), 0);
    }
}

struct ScopeDropProbe {
    panic_on_drop: bool,
    drops: Rc<Cell<usize>>,
}
impl Drop for ScopeDropProbe {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
        assert!(
            !self.panic_on_drop,
            "one normal-exit native wrapper destructor panicked"
        );
    }
}
#[test]
fn wrapper_destructor_panic_at_normal_scope_exit_does_not_strand_active_depth() {
    let drops = Rc::new(Cell::new(0));
    let values =
        ValueStore::new(&EngineIdentity::new().unwrap(), ContextLimits::default()).unwrap();
    let root = values
        .with_scope(|scope| {
            scope.retain(scope.insert(ScopeDropProbe {
                panic_on_drop: false,
                drops: drops.clone(),
            })?)
        })
        .unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        values.with_scope(|scope| {
            scope.insert(ScopeDropProbe {
                panic_on_drop: true,
                drops: drops.clone(),
            })?;
            Ok(())
        })
    }));
    assert!(result.is_err());
    assert_eq!(
        values.state.active.get(),
        0,
        "all scopes have unwound after the contained wrapper panic"
    );
    values.close().unwrap();
    assert!(values.is_closed());
    assert_eq!(values.live_roots(), 0);
    assert_eq!(drops.get(), 2);
    assert_eq!(
        values
            .with_scope(|scope| scope.local(&root).map(|_| ()))
            .unwrap_err()
            .code,
        FailureCode::ClosedContext
    );
    drop(root);
}
