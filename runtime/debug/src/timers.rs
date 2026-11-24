//! Monotonic clock contract and indexed, stable timer scheduling internals.

use crate::arena::Key;
use std::time::{Duration, Instant};
/// Owner-provided clocks must be monotonic. Ordinary hosts use MonotonicClock.
///
/// The owner reads the same clock for admission, due-timer checks, and interval
/// rearming. A test implementation may advance virtual time explicitly; it must
/// not move backwards or assume that an operating-system poll advances it.
pub trait Clock {
    /// Returns the current monotonic instant without dispatching application work.
    fn now(&self) -> Instant;
}
/// System monotonic clock backed by [`Instant::now`].
///
/// Used by [`crate::scheduler::EventLoop::new`]; its instants are unrelated to
/// wall-clock dates and are suitable for timer deadlines and elapsed durations.
pub struct MonotonicClock;
impl Clock for MonotonicClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Schedule {
    pub deadline: Instant,
    pub period: Option<Duration>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    key: Key,
    deadline: Instant,
    sequence: u64,
}
pub(crate) struct TimerHeap {
    heap: Vec<Entry>,
    positions: Vec<Option<usize>>,
    sequence: u64,
}
impl TimerHeap {
    pub fn new(capacity: usize) -> Result<Self, ()> {
        let mut heap = Vec::new();
        let mut positions = Vec::new();
        heap.try_reserve_exact(capacity).map_err(|_| ())?;
        positions.try_reserve_exact(capacity).map_err(|_| ())?;
        // Fix: every position was written here (16 bytes per job slot per VM). Decision:
        // the allocation keeps its full size, and `insert` extends the written prefix to
        // the highest slot seen; job slots are handed out densely from zero.
        Ok(Self {
            heap,
            positions,
            sequence: 0,
        })
    }
    fn before(&self, a: usize, b: usize) -> bool {
        let (a, b) = (self.heap[a], self.heap[b]);
        (a.deadline, a.sequence) < (b.deadline, b.sequence)
    }
    fn swap(&mut self, a: usize, b: usize) {
        self.heap.swap(a, b);
        self.positions[self.heap[a].key.slot as usize] = Some(a);
        self.positions[self.heap[b].key.slot as usize] = Some(b);
    }
    fn up(&mut self, mut at: usize) {
        while at > 0 {
            let parent = (at - 1) / 2;
            if !self.before(at, parent) {
                break;
            }
            self.swap(at, parent);
            at = parent;
        }
    }
    fn down(&mut self, mut at: usize) {
        loop {
            let left = at * 2 + 1;
            if left >= self.heap.len() {
                break;
            }
            let right = left + 1;
            let child = if right < self.heap.len() && self.before(right, left) {
                right
            } else {
                left
            };
            if !self.before(child, at) {
                break;
            }
            self.swap(child, at);
            at = child;
        }
    }
    pub fn insert(&mut self, key: Key, deadline: Instant) -> Result<(), ()> {
        let slot = key.slot as usize;
        if slot >= self.positions.capacity() || self.heap.len() == self.heap.capacity() {
            return Err(());
        }
        if slot >= self.positions.len() {
            // Within the capacity reserved by `new`: no reallocation.
            self.positions.resize(slot + 1, None);
        }
        if self.positions[slot].is_some() {
            return Err(());
        }
        let sequence = self.sequence.checked_add(1).ok_or(())?;
        self.sequence = sequence;
        let at = self.heap.len();
        self.heap.push(Entry {
            key,
            deadline,
            sequence,
        });
        self.positions[key.slot as usize] = Some(at);
        self.up(at);
        Ok(())
    }
    pub fn remove(&mut self, key: Key) -> bool {
        let Some(at) = self.positions.get(key.slot as usize).copied().flatten() else {
            return false;
        };
        if self.heap[at].key != key {
            return false;
        }
        self.positions[key.slot as usize] = None;
        let last = self.heap.pop().expect("indexed entry");
        if at < self.heap.len() {
            self.heap[at] = last;
            self.positions[last.key.slot as usize] = Some(at);
            if at > 0 && self.before(at, (at - 1) / 2) {
                self.up(at);
            } else {
                self.down(at);
            }
        }
        true
    }
    pub fn pop_due(&mut self, now: Instant) -> Option<Key> {
        let first = *self.heap.first()?;
        if first.deadline > now {
            return None;
        }
        self.remove(first.key);
        Some(first.key)
    }
    pub fn next(&self) -> Option<Instant> {
        self.heap.first().map(|entry| entry.deadline)
    }
}
pub(crate) fn next_period(schedule: Schedule, now: Instant) -> Option<Instant> {
    let period = schedule.period?;
    let elapsed = now.saturating_duration_since(schedule.deadline).as_nanos();
    let steps = elapsed.checked_div(period.as_nanos())?.checked_add(1)?;
    let nanos = period.as_nanos().checked_mul(steps)?;
    let seconds = u64::try_from(nanos / 1_000_000_000).ok()?;
    schedule
        .deadline
        .checked_add(Duration::new(seconds, (nanos % 1_000_000_000) as u32))
}
#[cfg(test)]
#[path = "../tests/unit/timers.rs"]
mod tests;
