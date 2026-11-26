//! Unit tests of engine and context identities: epochs and generations never wrap,
//! foreign owners are rejected, invalid context limits change nothing.

use super::*;
use crate::engine::{ContextLimits, ValueStore};

#[test]
fn epoch_exhaustion_never_wraps_or_reuses_an_issued_epoch() {
    // A local allocator exercises the constructor's production path without
    // changing a global counter shared by concurrently running tests.
    let counter = AtomicU64::new(u64::MAX - 1);
    assert_eq!(allocate_engine_epoch(&counter).unwrap(), u64::MAX - 1);
    for _ in 0..2 {
        assert_eq!(
            allocate_engine_epoch(&counter).unwrap_err().code,
            FailureCode::Capacity
        );
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    }
}

#[test]
fn context_generation_exhaustion_preserves_the_last_identity() {
    let identity = EngineIdentity::new().unwrap();
    identity.generation.set(u64::MAX - 1);
    let last = identity.context().unwrap();
    assert_eq!(last.generation(), u64::MAX);
    assert_eq!(last.engine_epoch(), identity.epoch());
    let cloned = identity.clone();
    assert_eq!(cloned.context().unwrap_err().code, FailureCode::Capacity);
    assert_eq!(identity.generation.get(), u64::MAX);
    assert!(matches!(
        ValueStore::<()>::new(&identity, ContextLimits::default()),
        Err(FailureReport {
            code: FailureCode::Capacity,
            ..
        })
    ));
}

#[test]
fn foreign_owner_rejection_happens_before_context_generation_changes() {
    let mut identity = EngineIdentity::new().unwrap();
    let original = identity.owner;
    // Only a ThreadId crosses threads. Rc-backed identity state stays here.
    identity.owner = thread::spawn(|| thread::current().id()).join().unwrap();
    assert_eq!(
        identity.context().unwrap_err().code,
        FailureCode::WrongOwner
    );
    assert!(matches!(
        ValueStore::<()>::new(&identity, ContextLimits::default()),
        Err(FailureReport {
            code: FailureCode::WrongOwner,
            ..
        })
    ));
    assert_eq!(identity.generation.get(), 0);
    identity.owner = original;
    assert_eq!(identity.context().unwrap().generation(), 1);
}

#[test]
fn invalid_context_limits_do_not_consume_a_context_generation() {
    let identity = EngineIdentity::new().unwrap();
    assert!(
        ValueStore::<()>::new(
            &identity,
            ContextLimits {
                max_host_depth: 0,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert_eq!(identity.generation.get(), 0);
    let store = ValueStore::<()>::new(&identity, ContextLimits::default()).unwrap();
    assert_eq!(store.context().generation(), 1);
}
