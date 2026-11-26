use super::*;
#[test]
fn exhausted_slot_retires_instead_of_wrapping() {
    let mut arena = Arena::new(2).unwrap();
    arena.slots[0].generation = u64::MAX - 1;
    let last = arena.insert(1).unwrap();
    assert_eq!(last.generation, u64::MAX);
    assert_eq!(arena.remove(last), Some(1));
    let next = arena.insert(2).unwrap();
    assert_eq!(next.slot, 1);
    assert_eq!(arena.insert(3), Err(3));
    assert_eq!(arena.get(last), None);
}
#[test]
fn oversized_capacity_and_a_retired_free_entry_are_rejected_without_reuse() {
    if let Some(capacity) = (u32::MAX as usize).checked_add(1) {
        assert!(Arena::<u8>::new(capacity).is_err());
    }
    let mut arena = Arena::new(2).unwrap();
    // Defensive allocator boundary: an exhausted slot must be skipped even
    // when it remains in a stale free-list entry.
    arena.slots[0].generation = u64::MAX;
    let live = arena.insert(7).unwrap();
    assert_eq!(live.slot, 1);
    assert_eq!(arena.insert(9), Err(9));
    assert!(arena.slots[0].value.is_none());
    assert_eq!(arena.len(), 1);
    assert_eq!(arena.get(live), Some(&7));
    assert!(
        arena
            .get(Key {
                slot: u32::MAX,
                generation: 1
            })
            .is_none()
    );
    assert!(
        arena
            .get_mut(Key {
                generation: live.generation + 1,
                ..live
            })
            .is_none()
    );
    assert_eq!(
        arena.remove(Key {
            generation: live.generation + 1,
            ..live
        }),
        None
    );
    assert_eq!(arena.remove(live), Some(7));
}
#[test]
fn either_backing_allocation_can_fail_without_leaking_a_partial_arena() {
    for allocation in 0..2 {
        let (result, fired) =
            crate::vm::test_allocator::fail_nth(allocation, || Arena::<u64>::new(2));
        assert!(fired && result.is_err());
        let mut healthy = Arena::new(2).unwrap();
        let key = healthy.insert(42).unwrap();
        assert_eq!(healthy.remove(key), Some(42));
        assert_eq!(healthy.len(), 0);
    }
}
