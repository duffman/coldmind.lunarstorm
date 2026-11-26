//! Owner-local copied async ancestry; scheduling adapters supply all captures.
use super::{DebugError, DebugLocation};
use crate::engine::{ContextId, JsString};
use std::{
    mem::size_of,
    sync::atomic::{AtomicU64, Ordering},
};

/// Scheduling boundary recorded without application AsyncLocalStorage state.
#[derive(Clone, Copy, Debug)]
#[repr(i32)]
pub enum AsyncScheduleKind {
    /// Promise reaction registration.
    Promise = 0,
    /// Suspension at an actual await instruction.
    Await = 1,
    /// Explicit microtask admission.
    Microtask = 2,
    /// Owner-loop callback admission.
    Callback = 3,
    /// Timer admission.
    Timer = 4,
    /// Promise thenable job admission.
    Thenable = 5,
    /// Original Promise rejection site.
    Rejection = 6,
}
impl AsyncScheduleKind {
    #[cfg(feature = "quickjs-ng")]
    pub(crate) fn from_native(kind: i32) -> Option<Self> {
        Some(match kind {
            0 => Self::Promise,
            1 => Self::Await,
            2 => Self::Microtask,
            3 => Self::Callback,
            4 => Self::Timer,
            5 => Self::Thenable,
            6 => Self::Rejection,
            _ => return None,
        })
    }
    #[cfg(feature = "quickjs-ng")]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Promise => "Promise.then",
            Self::Await => "await",
            Self::Microtask => "microtask",
            Self::Callback => "callback",
            Self::Timer => "timer",
            Self::Thenable => "Promise.thenable",
            Self::Rejection => "Promise rejection",
        }
    }
}

static NEXT_STACK: AtomicU64 = AtomicU64::new(1);

/// Opaque context-bound identity. Serials never repeat, even across replacement stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AsyncStackId {
    engine_epoch: u64,
    context_generation: u64,
    serial: u64,
}

impl AsyncStackId {
    /// Native callback routing token; resolution still requires the owning store.
    pub(crate) fn serial(self) -> u64 {
        self.serial
    }
}

/// Detached copied ancestry metadata; contains no native roots or engine handles.
#[derive(Clone, Debug)]
pub struct AsyncStackRecord {
    /// Retained immediate ancestor, if available.
    pub parent: Option<AsyncStackId>,
    /// Lossless host-supplied scheduling label.
    pub label: JsString,
    /// Copied synchronous locations at the scheduling boundary.
    pub locations: Vec<DebugLocation>,
    /// Some ancestry was omitted by depth limits, release, or eviction.
    pub truncated: bool,
}

/// A detached scheduling boundary and host-projected original locations.
#[derive(Clone, Debug)]
pub struct AsyncStackTraceEntry {
    /// Context-bound immutable record identity, never a live frame reference.
    pub id: AsyncStackId,
    /// Copied scheduling metadata.
    pub record: AsyncStackRecord,
    /// Parallel mapped positions, absent when no reliable original is available.
    pub originals: Vec<Option<super::OriginalLocation>>,
}

/// Bounded historical ancestry for the currently executing job at a live pause.
#[derive(Clone, Debug, Default)]
pub struct AsyncStackTrace {
    /// Newest scheduling boundary first; no entry authorizes evaluation or editing.
    pub entries: Vec<AsyncStackTraceEntry>,
    /// Some history was omitted, evicted, or could not be captured.
    pub truncated: bool,
    /// Current owner-local record count, including other pending jobs' histories.
    pub retained_records: usize,
    /// Actual history-store backing allocation, excluding this detached reply.
    pub retained_bytes: usize,
}

impl AsyncStackTrace {
    pub(crate) fn heap_bytes(&self) -> Result<usize, DebugError> {
        let mut bytes = self
            .entries
            .capacity()
            .checked_mul(size_of::<AsyncStackTraceEntry>())
            .ok_or(DebugError::Capacity)?;
        for entry in &self.entries {
            bytes = bytes
                .checked_add(entry.record.label.capacity_bytes())
                .and_then(|n| {
                    n.checked_add(
                        entry
                            .record
                            .locations
                            .capacity()
                            .checked_mul(size_of::<DebugLocation>())?,
                    )
                })
                .and_then(|n| {
                    n.checked_add(
                        entry
                            .originals
                            .capacity()
                            .checked_mul(size_of::<Option<super::OriginalLocation>>())?,
                    )
                })
                .ok_or(DebugError::Capacity)?;
            for original in entry.originals.iter().flatten() {
                bytes = bytes
                    .checked_add(original.origin.capacity())
                    .ok_or(DebugError::Capacity)?;
            }
        }
        Ok(bytes)
    }
}

struct Entry {
    id: AsyncStackId,
    record: AsyncStackRecord,
    bytes: usize,
}

/// Bounded owner-local metadata, independent of any native scheduler.
///
/// The oldest captures are evicted first. Stored records never change; reads
/// produce independent snapshots and mark missing ancestry explicitly. The byte
/// budget includes the entry table's actual allocation and all retained label
/// and location capacities, excluding allocator overhead and caller-owned read
/// snapshots. `ContextId` makes this store neither `Send` nor `Sync`.
pub struct AsyncStackStore {
    context: ContextId,
    max_depth: usize,
    max_records: usize,
    max_bytes: usize,
    backing_bytes: usize,
    payload_bytes: usize,
    entries: Vec<Entry>,
}

impl AsyncStackStore {
    /// Defaults to 32 ancestry records, 1,024 captures, and 8 MiB retained allocation.
    pub fn new(context: ContextId) -> Result<Self, DebugError> {
        Self::with_limits(context, 32, 1024, 8 * 1024 * 1024)
    }

    /// Reserves a bounded table; zero limits or insufficient bytes return `Capacity`.
    pub fn with_limits(
        context: ContextId,
        max_depth: usize,
        max_records: usize,
        max_bytes: usize,
    ) -> Result<Self, DebugError> {
        let minimum = max_records
            .checked_mul(size_of::<Entry>())
            .ok_or(DebugError::Capacity)?;
        if max_depth == 0 || max_records == 0 || minimum > max_bytes {
            return Err(DebugError::Capacity);
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(max_records)
            .map_err(|_| DebugError::Capacity)?;
        let backing_bytes = entries
            .capacity()
            .checked_mul(size_of::<Entry>())
            .ok_or(DebugError::Capacity)?;
        if backing_bytes > max_bytes {
            return Err(DebugError::Capacity);
        }
        Ok(Self {
            context,
            max_depth,
            max_records,
            max_bytes,
            backing_bytes,
            payload_bytes: 0,
            entries,
        })
    }

    fn index(&self, id: AsyncStackId) -> Result<usize, DebugError> {
        if id.engine_epoch != self.context.engine_epoch()
            || id.context_generation != self.context.generation()
        {
            return Err(DebugError::InvalidReference);
        }
        self.entries
            .iter()
            .position(|entry| entry.id == id)
            .ok_or(DebugError::InvalidReference)
    }

    // Walk at most the configured depth. IDs only point backwards, so no cycles
    // can be created through the public API.
    fn ancestry(&self, mut parent: Option<AsyncStackId>) -> (usize, bool) {
        let mut depth = 0;
        let mut truncated = false;
        while let Some(id) = parent {
            let Ok(index) = self.index(id) else {
                return (depth, true);
            };
            if depth == self.max_depth {
                return (depth, true);
            }
            let record = &self.entries[index].record;
            depth += 1;
            truncated |= record.truncated;
            parent = record.parent;
        }
        (depth, truncated)
    }

    /// Retains copied metadata, evicting oldest captures as necessary.
    ///
    /// Foreign/released parents fail `InvalidReference` before any eviction.
    /// Over-budget payloads fail `Capacity` without changing retained records.
    /// Depth overflow omits the parent and explicitly marks the new record.
    pub fn capture(
        &mut self,
        mut parent: Option<AsyncStackId>,
        label: JsString,
        locations: Vec<DebugLocation>,
    ) -> Result<AsyncStackId, DebugError> {
        if let Some(id) = parent {
            self.index(id)?;
        }
        let bytes = locations
            .capacity()
            .checked_mul(size_of::<DebugLocation>())
            .and_then(|bytes| bytes.checked_add(label.capacity_bytes()))
            .ok_or(DebugError::Capacity)?;
        if bytes > self.max_bytes - self.backing_bytes {
            return Err(DebugError::Capacity);
        }
        let (depth, mut truncated) = self.ancestry(parent);
        if depth >= self.max_depth {
            parent = None;
            truncated = true;
        }
        let serial = NEXT_STACK
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < (1u64 << 52)).then(|| n + 1)
            })
            .map_err(|_| DebugError::Capacity)?;
        while self.entries.len() >= self.max_records
            || bytes > self.max_bytes - self.retained_bytes()
        {
            let removed = self.entries.remove(0);
            self.payload_bytes -= removed.bytes;
        }
        if parent.is_some_and(|id| self.index(id).is_err()) {
            parent = None;
            truncated = true;
        }
        let id = AsyncStackId {
            engine_epoch: self.context.engine_epoch(),
            context_generation: self.context.generation(),
            serial,
        };
        self.entries.push(Entry {
            id,
            record: AsyncStackRecord {
                parent,
                label,
                locations,
                truncated,
            },
            bytes,
        });
        self.payload_bytes += bytes;
        Ok(id)
    }

    #[cfg(feature = "quickjs-ng")]
    pub(crate) fn capture_truncated(
        &mut self,
        parent: Option<AsyncStackId>,
        label: JsString,
        locations: Vec<DebugLocation>,
        truncated: bool,
    ) -> Result<AsyncStackId, DebugError> {
        let id = self.capture(parent, label, locations)?;
        // Complete this newly created snapshot before exposing its identity.
        if let Some(entry) = self.entries.last_mut() {
            entry.record.truncated |= truncated;
        }
        Ok(id)
    }

    /// Resolve an untrusted native routing token only against retained captures.
    #[cfg(any(feature = "quickjs-ng", test))]
    pub(crate) fn resolve_serial(&self, serial: u64) -> Result<AsyncStackId, DebugError> {
        self.entries
            .iter()
            .find(|entry| entry.id.serial == serial)
            .map(|entry| entry.id)
            .ok_or(DebugError::InvalidReference)
    }

    /// Copies a snapshot; released/evicted IDs never resolve to a replacement.
    /// Missing ancestry is flagged even when an intermediate parent still exists.
    pub fn read(&self, id: AsyncStackId) -> Result<AsyncStackRecord, DebugError> {
        let record = &self.entries[self.index(id)?].record;
        let mut units = Vec::new();
        units
            .try_reserve_exact(record.label.as_units().len())
            .map_err(|_| DebugError::Capacity)?;
        units.extend_from_slice(record.label.as_units());
        let mut locations = Vec::new();
        locations
            .try_reserve_exact(record.locations.len())
            .map_err(|_| DebugError::Capacity)?;
        locations.extend_from_slice(&record.locations);
        let (_, truncated) = self.ancestry(record.parent);
        let parent = record.parent.filter(|id| self.index(*id).is_ok());
        Ok(AsyncStackRecord {
            parent,
            label: JsString::from_units(units),
            locations,
            truncated: record.truncated || truncated,
        })
    }

    /// Releases exactly one record; dependent snapshots report omitted ancestry.
    pub fn release(&mut self, id: AsyncStackId) -> Result<(), DebugError> {
        let index = self.index(id)?;
        self.payload_bytes -= self.entries.remove(index).bytes;
        Ok(())
    }

    /// Releases all captures, retaining bounded table capacity and never reusing IDs.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.payload_bytes = 0;
    }

    /// Current retained capture count.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Actual retained table, label, and location backing allocation bytes.
    pub fn retained_bytes(&self) -> usize {
        self.backing_bytes + self.payload_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineIdentity;

    fn context() -> ContextId {
        EngineIdentity::new().unwrap().context().unwrap()
    }
    fn capture(store: &mut AsyncStackStore, parent: Option<AsyncStackId>) -> AsyncStackId {
        store
            .capture(parent, JsString::from_utf8("task"), Vec::new())
            .unwrap()
    }

    #[test]
    fn rejects_foreign_stale_and_replacement_store_ids() {
        let context = context();
        let mut store = AsyncStackStore::new(context).unwrap();
        let first = capture(&mut store, None);
        let mut other = AsyncStackStore::new(context).unwrap();
        let second = capture(&mut other, None);
        assert_ne!(first, second);
        fn transferable<T: Send + Sync>() {}
        transferable::<AsyncStackId>();
        assert_eq!(store.resolve_serial(first.serial()).unwrap(), first);
        assert!(matches!(
            other.resolve_serial(first.serial()),
            Err(DebugError::InvalidReference)
        ));
        assert!(matches!(
            other.read(first),
            Err(DebugError::InvalidReference)
        ));
        let mut foreign =
            AsyncStackStore::new(EngineIdentity::new().unwrap().context().unwrap()).unwrap();
        assert!(matches!(
            foreign.capture(Some(first), JsString::default(), Vec::new()),
            Err(DebugError::InvalidReference)
        ));
        store.release(first).unwrap();
        assert!(matches!(
            store.release(first),
            Err(DebugError::InvalidReference)
        ));
        assert!(matches!(
            store.capture(Some(first), JsString::default(), Vec::new()),
            Err(DebugError::InvalidReference)
        ));
        let next = capture(&mut store, None);
        store.clear();
        assert!(matches!(
            store.read(next),
            Err(DebugError::InvalidReference)
        ));
        assert_ne!(next, capture(&mut store, None));
    }

    #[test]
    fn snapshots_are_independent_and_depth_is_bounded() {
        let mut store = AsyncStackStore::with_limits(context(), 2, 8, 8192).unwrap();
        let root = capture(&mut store, None);
        let child = capture(&mut store, Some(root));
        let tip = capture(&mut store, Some(child));
        let tip = store.read(tip).unwrap();
        assert!(tip.truncated);
        assert_eq!(tip.parent, None);
        let mut copy = store.read(child).unwrap();
        copy.label = JsString::from_utf8("changed");
        copy.parent = None;
        assert_eq!(
            store.read(child).unwrap().label,
            JsString::from_utf8("task")
        );
        assert_eq!(store.read(child).unwrap().parent, Some(root));
    }

    #[test]
    fn count_eviction_and_release_mark_missing_ancestors() {
        let mut store = AsyncStackStore::with_limits(context(), 8, 3, 8192).unwrap();
        let root = capture(&mut store, None);
        let child = capture(&mut store, Some(root));
        let tip = capture(&mut store, Some(child));
        capture(&mut store, None);
        assert_eq!(store.count(), 3);
        assert!(matches!(
            store.read(root),
            Err(DebugError::InvalidReference)
        ));
        assert_eq!(store.read(child).unwrap().parent, None);
        assert!(store.read(child).unwrap().truncated);
        assert!(store.read(tip).unwrap().truncated);
        store.release(child).unwrap();
        assert_eq!(store.read(tip).unwrap().parent, None);
        assert!(store.read(tip).unwrap().truncated);
    }

    #[test]
    fn byte_budget_accounts_for_backing_capacities_and_failed_capture_is_atomic() {
        let table = 2 * size_of::<Entry>();
        let mut store = AsyncStackStore::with_limits(context(), 4, 2, table + 64).unwrap();
        let first = capture(&mut store, None);
        let mut units = Vec::with_capacity(32);
        units.push(65);
        let second = store
            .capture(Some(first), JsString::from_units(units), Vec::new())
            .unwrap();
        assert_eq!(store.count(), 1);
        assert_eq!(store.retained_bytes(), table + 64);
        assert!(store.read(second).unwrap().truncated);
        let units = Vec::with_capacity(33);
        assert!(matches!(
            store.capture(None, JsString::from_units(units), Vec::new()),
            Err(DebugError::Capacity)
        ));
        assert_eq!(store.count(), 1);
        assert!(store.read(second).is_ok());
        let locations = Vec::<DebugLocation>::with_capacity(65);
        assert!(matches!(
            store.capture(None, JsString::default(), locations),
            Err(DebugError::Capacity)
        ));
        assert!(store.read(second).is_ok());
        store.clear();
        assert_eq!(store.count(), 0);
        assert_eq!(store.retained_bytes(), table);
        assert!(matches!(
            AsyncStackStore::with_limits(context(), 0, 2, 8192),
            Err(DebugError::Capacity)
        ));
        assert!(matches!(
            AsyncStackStore::with_limits(context(), 4, 2, table - 1),
            Err(DebugError::Capacity)
        ));
    }
}
