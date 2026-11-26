//! Unit tests of the ready-item queue: backing capacity failure and reuse of a removed
//! middle node.

use super::*;
#[test]
fn impossible_backing_capacity_is_a_checked_allocation_error() {
    assert!(ReadyQueue::<u64>::new(usize::MAX).is_err());
}
#[test]
fn removing_and_reusing_a_middle_slot_keeps_fifo_links_consistent() {
    let mut queue = ReadyQueue::new(3).unwrap();
    queue.push(0, 10);
    queue.push(1, 20);
    queue.push(2, 30);
    assert_eq!(queue.remove(1), Some(20));
    assert_eq!(queue.remove(1), None);
    queue.push(1, 40);
    assert_eq!(
        [queue.pop(), queue.pop(), queue.pop(), queue.pop()],
        [Some(10), Some(30), Some(40), None]
    );
    assert_eq!(queue.len(), 0);
    assert!(queue.head.is_none() && queue.tail.is_none());
}
