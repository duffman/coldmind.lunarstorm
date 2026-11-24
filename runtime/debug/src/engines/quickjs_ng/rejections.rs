//! Owner-local rejection bookkeeping; native hooks supply already-owned roots.
use super::NativeValue;
use crate::debug::{AsyncStackId, DebugError, DebugLocation};
use std::{mem::size_of, rc::Rc};

const ROOT_BYTES: usize = 2 * (size_of::<NativeValue>() + 2 * size_of::<usize>());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Status {
    Pending,
    Unhandled,
    HandledLater,
}

pub(super) struct Record {
    pub serial: u64,
    pub key: usize,
    pub promise: Rc<NativeValue>,
    pub reason: Rc<NativeValue>,
    pub location: Option<DebugLocation>,
    pub history: Option<AsyncStackId>,
    pub status: Status,
    pub pause_reported: bool,
}

struct Entry {
    record: Record,
    handled_reported: bool,
}

/// Fixed-capacity FIFO table. Retained native values must belong to the owning
/// context; this table neither validates nor executes them. Dropping/releasing
/// their final Rc uses NativeValue's existing owner-thread destructor.
pub(super) struct Store {
    entries: Vec<Entry>,
    limit: usize,
    next_serial: u64,
    dropped: u64,
}

impl Store {
    pub fn new(limit: usize) -> Result<Self, DebugError> {
        if limit == 0 {
            return Err(DebugError::Capacity);
        }
        limit
            .checked_mul(size_of::<Entry>() + ROOT_BYTES)
            .ok_or(DebugError::Capacity)?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(limit)
            .map_err(|_| DebugError::Capacity)?;
        entries
            .capacity()
            .checked_mul(size_of::<Entry>())
            .and_then(|bytes| {
                limit
                    .checked_mul(ROOT_BYTES)
                    .and_then(|roots| bytes.checked_add(roots))
            })
            .ok_or(DebugError::Capacity)?;
        Ok(Self {
            entries,
            limit,
            next_serial: 0,
            dropped: 0,
        })
    }

    /// Duplicate keys retain their original identity, roots, and metadata.
    pub fn observe(
        &mut self,
        key: usize,
        promise: Rc<NativeValue>,
        reason: Rc<NativeValue>,
        location: Option<DebugLocation>,
        history: Option<AsyncStackId>,
    ) -> Result<u64, DebugError> {
        if let Some(record) = self.by_key(key) {
            return Ok(record.serial);
        }
        let serial = self
            .next_serial
            .checked_add(1)
            .ok_or(DebugError::Capacity)?;
        if self.entries.len() == self.limit {
            self.entries.remove(0);
            self.dropped = self.dropped.saturating_add(1);
        }
        self.next_serial = serial;
        self.entries.push(Entry {
            record: Record {
                serial,
                key,
                promise,
                reason,
                location,
                history,
                status: Status::Pending,
                pause_reported: false,
            },
            handled_reported: false,
        });
        Ok(serial)
    }

    /// Early handling suppresses qualification. Already-qualified rejections
    /// transition once; the next checkpoint reports handling even if not drained.
    pub fn handle(&mut self, key: usize) -> bool {
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.record.key == key)
        else {
            return false;
        };
        match self.entries[index].record.status {
            Status::Pending => {
                self.entries.remove(index);
            }
            Status::Unhandled => {
                self.entries[index].record.status = Status::HandledLater;
            }
            Status::HandledLater => return false,
        }
        true
    }

    /// A drained queue qualifies pending records. Already-emitted unhandled
    /// records produce one HandledLater event at the next checkpoint regardless
    /// of `drained`. Reserve the whole event batch before changing any state.
    pub fn checkpoint(&mut self, drained: bool) -> Result<Vec<(u64, Status)>, DebugError> {
        let count = self
            .entries
            .iter()
            .filter(|entry| {
                (drained && entry.record.status == Status::Pending)
                    || (entry.record.status == Status::HandledLater && !entry.handled_reported)
            })
            .count();
        let mut events = Vec::new();
        events
            .try_reserve_exact(count)
            .map_err(|_| DebugError::Capacity)?;
        for entry in &mut self.entries {
            if drained && entry.record.status == Status::Pending {
                entry.record.status = Status::Unhandled;
                events.push((entry.record.serial, Status::Unhandled));
            } else if entry.record.status == Status::HandledLater && !entry.handled_reported {
                entry.handled_reported = true;
                events.push((entry.record.serial, Status::HandledLater));
            }
        }
        Ok(events)
    }

    pub fn by_serial(&self, serial: u64) -> Option<&Record> {
        self.records().find(|record| record.serial == serial)
    }

    pub fn by_key(&self, key: usize) -> Option<&Record> {
        self.records().find(|record| record.key == key)
    }

    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.entries.iter().map(|entry| &entry.record)
    }

    pub fn mark_pause_reported(&mut self, serial: u64) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.record.serial == serial)
        else {
            return false;
        };
        if entry.record.pause_reported {
            return false;
        }
        entry.record.pause_reported = true;
        true
    }

    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Actual table allocation plus a conservative charge for two Rc allocations
    /// per record (NativeValue and reference counters), even if callers share Rc
    /// allocations. Allocator overhead and opaque native graph sizes are excluded;
    /// native graph memory remains governed by the context's native heap limits.
    pub fn retained_bytes(&self) -> usize {
        self.entries.capacity() * size_of::<Entry>() + self.entries.len() * ROOT_BYTES
    }

    /// Cumulative FIFO losses, saturating rather than wrapping.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Release every root without resetting serials or the cumulative loss count.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engine::{ContextLimits, JsEngine},
        engines::quickjs_ng::{QuickJsNgEngine, ffi},
    };

    fn fixture() -> (QuickJsNgEngine, Rc<NativeValue>) {
        let engine = QuickJsNgEngine::new().unwrap();
        let context = engine.create_context(ContextLimits::default()).unwrap();
        let raw = engine.context(context).unwrap().raw.clone();
        // SAFETY: undefined is an immediate owned value; its NativeValue wrapper
        // uses the live context's normal owner-thread destruction path.
        let value = Rc::new(NativeValue::new(unsafe { ffi::jbq_undefined() }, raw));
        (engine, value)
    }

    fn observe(store: &mut Store, key: usize, value: &Rc<NativeValue>) -> u64 {
        store
            .observe(key, value.clone(), value.clone(), None, None)
            .unwrap()
    }

    #[test]
    fn early_handling_and_duplicate_observation_do_not_report() {
        let (_engine, value) = fixture();
        let mut store = Store::new(3).unwrap();
        let serial = observe(&mut store, 1, &value);
        assert_eq!(observe(&mut store, 1, &value), serial);
        assert_eq!(store.count(), 1);
        assert_eq!(Rc::strong_count(&value), 3);
        assert!(store.checkpoint(false).unwrap().is_empty());
        assert!(store.handle(1));
        assert!(!store.handle(1));
        assert!(store.checkpoint(true).unwrap().is_empty());
        assert_eq!(Rc::strong_count(&value), 1);
        assert!(store.by_serial(serial).is_none());
    }

    #[test]
    fn qualification_and_late_handling_emit_once_at_checkpoints() {
        let (_engine, value) = fixture();
        let mut store = Store::new(2).unwrap();
        let serial = observe(&mut store, 1, &value);
        assert!(store.checkpoint(false).unwrap().is_empty());
        assert_eq!(store.by_key(1).unwrap().status, Status::Pending);
        assert_eq!(
            store.checkpoint(true).unwrap(),
            vec![(serial, Status::Unhandled)]
        );
        assert!(store.checkpoint(true).unwrap().is_empty());
        assert!(store.mark_pause_reported(serial));
        assert!(!store.mark_pause_reported(serial));
        assert!(store.by_serial(serial).unwrap().pause_reported);
        assert!(store.handle(1));
        assert!(!store.handle(1));
        assert_eq!(
            store.checkpoint(false).unwrap(),
            vec![(serial, Status::HandledLater)]
        );
        assert!(store.checkpoint(false).unwrap().is_empty());
        assert!(store.checkpoint(true).unwrap().is_empty());
        assert_eq!(store.by_key(1).unwrap().status, Status::HandledLater);
    }

    #[test]
    fn fifo_eviction_and_clear_release_roots_without_reusing_serials() {
        let (_engine, value) = fixture();
        let mut store = Store::new(1).unwrap();
        let bytes = store.retained_bytes();
        let first = observe(&mut store, 1, &value);
        let second = observe(&mut store, 2, &value);
        assert!(second > first);
        assert_eq!(store.count(), 1);
        assert_eq!(store.dropped(), 1);
        assert!(store.by_serial(first).is_none());
        assert_eq!(Rc::strong_count(&value), 3);
        assert_eq!(store.retained_bytes(), bytes + ROOT_BYTES);
        store.clear();
        assert_eq!(Rc::strong_count(&value), 1);
        assert_eq!(store.count(), 0);
        assert_eq!(store.dropped(), 1);
        assert_eq!(store.retained_bytes(), bytes);
        assert!(!store.mark_pause_reported(second));
        assert!(observe(&mut store, 2, &value) > second);
        drop(store);
        assert_eq!(Rc::strong_count(&value), 1);
        assert!(matches!(Store::new(0), Err(DebugError::Capacity)));
        assert!(matches!(Store::new(usize::MAX), Err(DebugError::Capacity)));
    }

    #[test]
    fn duplicate_keys_do_not_replace_roots_and_exhaustion_preserves_table() {
        let (_engine, value) = fixture();
        let mut store = Store::new(1).unwrap();
        let serial = observe(&mut store, 1, &value);
        // SAFETY: undefined is an immediate owned value in the fixture's live context.
        let replacement = Rc::new(NativeValue::new(
            unsafe { ffi::jbq_undefined() },
            value.raw.clone(),
        ));
        assert_eq!(observe(&mut store, 1, &replacement), serial);
        assert_eq!(Rc::strong_count(&replacement), 1);
        assert!(Rc::ptr_eq(&store.by_key(1).unwrap().promise, &value));
        assert!(Rc::ptr_eq(&store.by_key(1).unwrap().reason, &value));
        store.next_serial = u64::MAX;
        assert!(matches!(
            store.observe(2, value.clone(), value.clone(), None, None),
            Err(DebugError::Capacity)
        ));
        assert_eq!(store.by_key(1).unwrap().serial, serial);
        assert_eq!(store.dropped(), 0);
    }
}
