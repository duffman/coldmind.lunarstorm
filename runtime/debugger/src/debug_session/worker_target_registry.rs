//! Bounded cross-thread registry for independently owned worker debugger sessions.
use super::{DebugError, VmDebugHandle};
use crate::{engine::JsString, vm::VmHandle};
use std::sync::{
    Arc, Mutex, MutexGuard, Weak,
    atomic::{AtomicU64, Ordering},
};

static NEXT_BROKER: AtomicU64 = AtomicU64::new(1);

/// Opaque transferable identity issued by exactly one broker; never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DebugTargetId {
    broker: u64,
    serial: u64,
}
impl DebugTargetId {
    /// Opaque textual identity for protocol transport; never a JavaScript number.
    pub fn wire_id(self) -> String {
        format!("target:{}:{}", self.broker, self.serial)
    }
}

/// Detached target metadata. Exited targets may remain until capacity is needed.
#[derive(Clone, Debug)]
pub struct DebugTargetInfo {
    /// Exact broker-issued identity.
    pub id: DebugTargetId,
    /// Original ownership parent, independent of debugger attachment.
    pub parent: Option<DebugTargetId>,
    /// Lossless worker display name.
    pub name: JsString,
    /// The owner registration has finished or been dropped.
    pub exited: bool,
}

struct Entry {
    info: DebugTargetInfo,
    runtime: Option<u64>,
    debugger: Option<VmDebugHandle>,
    stop: Option<VmHandle>,
}
struct State {
    entries: Vec<Entry>,
    next_serial: u64,
    name_bytes: usize,
    closed: bool,
    debuggers_detached: bool,
}

/// Thread-safe broker, containing control leases and copied metadata only.
///
/// The byte budget charges actual table allocation and retained name capacities,
/// excluding allocator overhead and caller-owned list snapshots. Exited leaves
/// are evicted first; ancestry needed by retained children stays available so
/// subtree stop cannot lose ownership relationships. No live target is evicted.
pub struct DebugTargetBroker {
    identity: u64,
    limit: usize,
    max_bytes: usize,
    backing_bytes: usize,
    state: Mutex<State>,
}

/// Worker-owned lifetime token. Finishing revokes the broker's queued debugger
/// and stop lease, without stopping the application. Drop performs the same exit.
pub struct TargetRegistration {
    broker: Weak<DebugTargetBroker>,
    id: DebugTargetId,
    finished: bool,
}
impl TargetRegistration {
    /// Exact registered target identity.
    pub fn id(&self) -> DebugTargetId {
        self.id
    }

    /// Mark worker exit once. An already-closed or dropped broker is harmless.
    pub fn finish(&mut self) -> Result<(), DebugError> {
        if !self.finished {
            if let Some(broker) = self.broker.upgrade() {
                broker.finish(self.id);
            }
            self.finished = true;
        }
        Ok(())
    }
}
impl Drop for TargetRegistration {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

impl DebugTargetBroker {
    /// Allocate a fixed table. `max_bytes` includes table backing and all names.
    pub fn new(limit: usize, max_bytes: usize) -> Result<Arc<Self>, DebugError> {
        let minimum = limit
            .checked_mul(std::mem::size_of::<Entry>())
            .ok_or(DebugError::Capacity)?;
        if limit == 0 || minimum > max_bytes {
            return Err(DebugError::Capacity);
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(limit)
            .map_err(|_| DebugError::Capacity)?;
        let backing_bytes = entries
            .capacity()
            .checked_mul(std::mem::size_of::<Entry>())
            .ok_or(DebugError::Capacity)?;
        if backing_bytes > max_bytes {
            return Err(DebugError::Capacity);
        }
        let identity = NEXT_BROKER
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| DebugError::Capacity)?;
        Ok(Arc::new(Self {
            identity,
            limit,
            max_bytes,
            backing_bytes,
            state: Mutex::new(State {
                entries,
                next_serial: 0,
                name_bytes: 0,
                closed: false,
                debuggers_detached: false,
            }),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn index(&self, state: &State, id: DebugTargetId) -> Result<usize, DebugError> {
        if id.broker != self.identity {
            return Err(DebugError::InvalidReference);
        }
        state
            .entries
            .iter()
            .position(|entry| entry.info.id == id)
            .ok_or(DebugError::InvalidReference)
    }

    /// Transfer a worker's sole debugger consumer to the broker until taken.
    /// Failed admission drops that debugger outside the registry lock (detach).
    pub fn register(
        self: &Arc<Self>,
        parent: Option<DebugTargetId>,
        name: JsString,
        debugger: VmDebugHandle,
        stop: VmHandle,
    ) -> Result<TargetRegistration, DebugError> {
        // Capture the scheduler identity before entering the registry lock.
        let runtime = stop.runtime_epoch();
        let entry = Entry {
            info: DebugTargetInfo {
                id: DebugTargetId {
                    broker: self.identity,
                    serial: 0,
                },
                parent,
                name,
                exited: false,
            },
            runtime: Some(runtime),
            debugger: Some(debugger),
            stop: Some(stop),
        };
        let id = self.insert(entry)?;
        Ok(TargetRegistration {
            broker: Arc::downgrade(self),
            id,
            finished: false,
        })
    }

    // Also used by metadata-only unit tests; real registrations always carry both
    // leases. The owned candidate outlives the inner guard even on failed admission.
    fn insert(&self, entry: Entry) -> Result<DebugTargetId, DebugError> {
        let mut victims = Vec::new();
        victims
            .try_reserve_exact(self.limit)
            .map_err(|_| DebugError::Capacity)?;
        let mut removed = Vec::new();
        removed
            .try_reserve_exact(self.limit)
            .map_err(|_| DebugError::Capacity)?;
        let mut candidate = Some(entry);
        let mut detached = None;
        let result = {
            let mut state = self.lock();
            (|| {
                let entry = candidate
                    .as_ref()
                    .expect("candidate retained until admission");
                if state.closed {
                    return Err(DebugError::Closed);
                }
                if let Some(runtime) = entry.runtime
                    && state
                        .entries
                        .iter()
                        .any(|existing| !existing.info.exited && existing.runtime == Some(runtime))
                {
                    return Err(DebugError::InFlight);
                }
                if let Some(parent) = entry.info.parent {
                    let index = self.index(&state, parent)?;
                    if state.entries[index].info.exited {
                        return Err(DebugError::InvalidReference);
                    }
                }
                let bytes = entry.info.name.capacity_bytes();
                if bytes > self.max_bytes - self.backing_bytes {
                    return Err(DebugError::Capacity);
                }
                let serial = state
                    .next_serial
                    .checked_add(1)
                    .ok_or(DebugError::Capacity)?;
                let mut count = state.entries.len();
                let mut names = state.name_bytes;
                while count >= self.limit || bytes > self.max_bytes - self.backing_bytes - names {
                    let victim =
                        state
                            .entries
                            .iter()
                            .enumerate()
                            .position(|(index, candidate)| {
                                candidate.info.exited
                                    && !victims.contains(&index)
                                    && !state.entries.iter().enumerate().any(
                                        |(child_index, child)| {
                                            !victims.contains(&child_index)
                                                && child.info.parent == Some(candidate.info.id)
                                        },
                                    )
                            })
                            .ok_or(DebugError::Capacity)?;
                    victims.push(victim);
                    count -= 1;
                    names -= state.entries[victim].info.name.capacity_bytes();
                }
                victims.sort_unstable();
                for index in victims.iter().rev() {
                    removed.push(state.entries.remove(*index));
                }
                let id = DebugTargetId {
                    broker: self.identity,
                    serial,
                };
                let mut entry = candidate.take().expect("candidate admitted once");
                entry.info.id = id;
                if state.debuggers_detached {
                    detached = entry.debugger.take();
                }
                state.next_serial = serial;
                state.name_bytes = names + bytes;
                state.entries.push(entry);
                Ok(id)
            })()
        };
        drop(removed);
        drop(detached);
        result
    }

    fn finish(&self, id: DebugTargetId) {
        let leases = {
            let mut state = self.lock();
            let Ok(index) = self.index(&state, id) else {
                return;
            };
            let entry = &mut state.entries[index];
            entry.info.exited = true;
            (entry.debugger.take(), entry.stop.take())
        };
        drop(leases);
    }

    /// Copy at most the configured number of targets, in registration order.
    pub fn list(&self) -> Result<Vec<DebugTargetInfo>, DebugError> {
        let state = self.lock();
        let mut result = Vec::new();
        result
            .try_reserve_exact(state.entries.len())
            .map_err(|_| DebugError::Capacity)?;
        for entry in &state.entries {
            let mut units = Vec::new();
            units
                .try_reserve_exact(entry.info.name.as_units().len())
                .map_err(|_| DebugError::Capacity)?;
            units.extend_from_slice(entry.info.name.as_units());
            result.push(DebugTargetInfo {
                id: entry.info.id,
                parent: entry.info.parent,
                name: JsString::from_units(units),
                exited: entry.info.exited,
            });
        }
        Ok(result)
    }

    /// Find the live registration for this exact runtime. A detached controller
    /// does not change ownership; finished or foreign runtimes are refused.
    pub fn target_for_vm(&self, handle: &VmHandle) -> Result<DebugTargetId, DebugError> {
        let runtime = handle.runtime_epoch();
        let state = self.lock();
        if state.closed {
            return Err(DebugError::Closed);
        }
        state
            .entries
            .iter()
            .find(|entry| !entry.info.exited && entry.runtime == Some(runtime))
            .map(|entry| entry.info.id)
            .ok_or(DebugError::InvalidReference)
    }

    /// Resolve an exact canonical textual identity retained by this broker.
    /// Exited metadata can be resolved, but cannot yield a debugger or stop lease.
    pub fn resolve_wire_id(&self, wire: &str) -> Result<DebugTargetId, DebugError> {
        if wire.len() > 48 {
            return Err(DebugError::InvalidReference);
        }
        let mut parts = wire
            .strip_prefix("target:")
            .ok_or(DebugError::InvalidReference)?
            .split(':');
        let number = |part: Option<&str>| -> Result<u64, DebugError> {
            let part = part.ok_or(DebugError::InvalidReference)?;
            if part.is_empty() || part.starts_with('0') || !part.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(DebugError::InvalidReference);
            }
            part.parse().map_err(|_| DebugError::InvalidReference)
        };
        let id = DebugTargetId {
            broker: number(parts.next())?,
            serial: number(parts.next())?,
        };
        if parts.next().is_some() {
            return Err(DebugError::InvalidReference);
        }
        let state = self.lock();
        if state.closed {
            return Err(DebugError::Closed);
        }
        self.index(&state, id)?;
        Ok(id)
    }

    /// Whether newly started workers should attach a debugger. This closes on
    /// frontend disconnect independently of application ownership and stop leases.
    pub fn debugger_enabled(&self) -> bool {
        let state = self.lock();
        !state.closed && !state.debuggers_detached
    }

    /// Detach unclaimed controllers without stopping applications or discarding
    /// ownership metadata. Already-taken controllers remain consumer-owned.
    /// Future registrations retain metadata/stop leases but release controllers,
    /// including a worker racing disconnect after its initial enable check.
    pub fn detach_debuggers(&self) -> Result<(), DebugError> {
        {
            let mut state = self.lock();
            if state.closed {
                return Err(DebugError::Closed);
            }
            state.debuggers_detached = true;
        }
        // Teardown must work even when allocation is exhausted. Drop each
        // controller outside the registry lock because detach can wake an owner.
        loop {
            let detached = self
                .lock()
                .entries
                .iter_mut()
                .find_map(|entry| entry.debugger.take());
            let Some(detached) = detached else {
                break;
            };
            drop(detached);
        }
        Ok(())
    }

    /// Move the sole debugger controller once; repeated takes return `InFlight`.
    pub fn take_debugger(&self, id: DebugTargetId) -> Result<VmDebugHandle, DebugError> {
        let mut state = self.lock();
        if state.closed {
            return Err(DebugError::Closed);
        }
        if state.debuggers_detached {
            return Err(DebugError::NotAttached);
        }
        let index = self.index(&state, id)?;
        if state.entries[index].info.exited {
            return Err(DebugError::InvalidReference);
        }
        state.entries[index]
            .debugger
            .take()
            .ok_or(DebugError::InFlight)
    }

    /// Signal exactly one live target after releasing the registry lock.
    pub fn request_stop(&self, id: DebugTargetId) -> Result<(), DebugError> {
        let stop = {
            let state = self.lock();
            if state.closed {
                return Err(DebugError::Closed);
            }
            let index = self.index(&state, id)?;
            state.entries[index]
                .stop
                .clone()
                .ok_or(DebugError::InvalidReference)?
        };
        stop.request_stop()
            .map_err(|error| DebugError::WakeFailed(error.bounded()))
    }

    /// Stop a retained ownership subtree, including live descendants of an exited
    /// parent. Unrelated roots and targets in other brokers are never signaled.
    pub fn stop_subtree(&self, id: DebugTargetId) -> Result<(), DebugError> {
        let stops = {
            let state = self.lock();
            if state.closed {
                return Err(DebugError::Closed);
            }
            self.index(&state, id)?;
            let mut stops = Vec::new();
            stops
                .try_reserve_exact(state.entries.len())
                .map_err(|_| DebugError::Capacity)?;
            for entry in &state.entries {
                let mut ancestor = Some(entry.info.id);
                while let Some(current) = ancestor {
                    if current == id {
                        if let Some(stop) = &entry.stop {
                            stops.push(stop.clone());
                        }
                        break;
                    }
                    ancestor = state.entries[self.index(&state, current)?].info.parent;
                }
            }
            stops
        };
        Self::signal(stops)
    }

    fn signal(stops: Vec<VmHandle>) -> Result<(), DebugError> {
        let mut error = None;
        for stop in stops {
            if let Err(failure) = stop.request_stop()
                && error.is_none()
            {
                error = Some(DebugError::WakeFailed(failure.bounded()));
            }
        }
        error.map_or(Ok(()), Err)
    }

    /// Close admission, stop this broker's remaining live targets, and release its
    /// queued debugger controllers outside the lock. Already-taken controllers
    /// remain consumer-owned. Repeated close is harmless.
    pub fn close(&self) -> Result<(), DebugError> {
        let entries = {
            let mut state = self.lock();
            state.closed = true;
            state.name_bytes = 0;
            std::mem::take(&mut state.entries)
        };
        let mut error = None;
        for mut entry in entries {
            if let Some(stop) = entry.stop.take()
                && let Err(failure) = stop.request_stop()
                && error.is_none()
            {
                error = Some(DebugError::WakeFailed(failure.bounded()));
            }
            drop(entry);
        }
        error.map_or(Ok(()), Err)
    }

    /// Number of retained live and exited metadata records.
    pub fn count(&self) -> usize {
        self.lock().entries.len()
    }

    /// Maximum number of retained targets and independent frontend sessions.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Actual retained table backing and name capacities; snapshots are excluded.
    pub fn retained_bytes(&self) -> usize {
        let state = self.lock();
        state.entries.capacity() * std::mem::size_of::<Entry>() + state.name_bytes
    }
}
impl Drop for DebugTargetBroker {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(
        broker: &DebugTargetBroker,
        parent: Option<DebugTargetId>,
        name: JsString,
    ) -> Result<DebugTargetId, DebugError> {
        broker.insert(Entry {
            info: DebugTargetInfo {
                id: DebugTargetId {
                    broker: 0,
                    serial: 0,
                },
                parent,
                name,
                exited: false,
            },
            runtime: None,
            debugger: None,
            stop: None,
        })
    }
    fn add(broker: &DebugTargetBroker, parent: Option<DebugTargetId>) -> DebugTargetId {
        metadata(broker, parent, JsString::from_utf8("worker")).unwrap()
    }

    #[test]
    fn ids_are_transferable_and_foreign_or_exited_parents_fail() {
        fn transferable<T: Send + Sync>() {}
        transferable::<DebugTargetBroker>();
        transferable::<DebugTargetId>();
        transferable::<TargetRegistration>();
        let broker = DebugTargetBroker::new(4, 8192).unwrap();
        let other = DebugTargetBroker::new(4, 8192).unwrap();
        let parent = add(&broker, None);
        assert!(matches!(
            metadata(&other, Some(parent), JsString::default()),
            Err(DebugError::InvalidReference)
        ));
        assert!(matches!(
            other.take_debugger(parent),
            Err(DebugError::InvalidReference)
        ));
        broker.finish(parent);
        assert!(matches!(
            metadata(&broker, Some(parent), JsString::default()),
            Err(DebugError::InvalidReference)
        ));
        assert!(matches!(
            broker.take_debugger(parent),
            Err(DebugError::InvalidReference)
        ));
    }

    #[test]
    fn registration_exit_is_idempotent_and_does_not_revoke_children() {
        let broker = DebugTargetBroker::new(4, 8192).unwrap();
        let parent = add(&broker, None);
        let child = add(&broker, Some(parent));
        let mut registration = TargetRegistration {
            broker: Arc::downgrade(&broker),
            id: parent,
            finished: false,
        };
        registration.finish().unwrap();
        registration.finish().unwrap();
        drop(registration);
        let info = broker.list().unwrap();
        assert!(info[0].exited);
        assert!(!info[1].exited);
        assert_eq!(info[1].parent, Some(parent));
        let registration = TargetRegistration {
            broker: Arc::downgrade(&broker),
            id: child,
            finished: false,
        };
        drop(registration);
        assert!(broker.list().unwrap()[1].exited);
    }

    #[test]
    fn metadata_bounds_evict_exited_leaves_without_reusing_ids() {
        let broker = DebugTargetBroker::new(2, 8192).unwrap();
        let parent = add(&broker, None);
        let child = add(&broker, Some(parent));
        broker.finish(parent);
        assert!(matches!(
            metadata(&broker, None, JsString::default()),
            Err(DebugError::Capacity)
        ));
        broker.finish(child);
        let next = add(&broker, None);
        assert!(next.serial > child.serial);
        assert_eq!(broker.count(), 2);
        assert!(matches!(
            broker.take_debugger(child),
            Err(DebugError::InvalidReference)
        ));
        assert_eq!(broker.list().unwrap()[0].id, parent);
    }

    #[test]
    fn identity_exhaustion_preserves_existing_metadata() {
        let broker = DebugTargetBroker::new(1, 8192).unwrap();
        let first = add(&broker, None);
        broker.finish(first);
        broker.lock().next_serial = u64::MAX;
        assert!(matches!(
            metadata(&broker, None, JsString::default()),
            Err(DebugError::Capacity)
        ));
        assert_eq!(broker.list().unwrap()[0].id, first);
        assert_eq!(broker.count(), 1);
    }

    #[test]
    fn a_live_runtime_has_one_target_and_exit_allows_a_fresh_identity() {
        let broker = DebugTargetBroker::new(2, 8192).unwrap();
        let candidate = || Entry {
            info: DebugTargetInfo {
                id: DebugTargetId {
                    broker: 0,
                    serial: 0,
                },
                parent: None,
                name: JsString::from_utf8("same runtime"),
                exited: false,
            },
            runtime: Some(42),
            debugger: None,
            stop: None,
        };
        let first = broker.insert(candidate()).unwrap();
        assert!(matches!(
            broker.insert(candidate()),
            Err(DebugError::InFlight)
        ));
        assert_eq!(broker.count(), 1);
        broker.finish(first);
        let next = broker.insert(candidate()).unwrap();
        assert_ne!(first, next);
        assert!(next.serial > first.serial);
    }

    #[test]
    fn wire_identity_is_exact_bounded_and_resolves_retained_metadata_only() {
        let broker = DebugTargetBroker::new(1, 8192).unwrap();
        let other = DebugTargetBroker::new(1, 8192).unwrap();
        let first = add(&broker, None);
        assert_eq!(broker.resolve_wire_id(&first.wire_id()).unwrap(), first);
        assert!(matches!(
            other.resolve_wire_id(&first.wire_id()),
            Err(DebugError::InvalidReference)
        ));
        for invalid in [
            "",
            "target:0:1",
            "target:1:0",
            "target:01:1",
            "target:+1:1",
            "target:1:1:1",
            "target:18446744073709551616:1",
            "target:1: 1",
        ] {
            assert!(
                matches!(
                    broker.resolve_wire_id(invalid),
                    Err(DebugError::InvalidReference)
                ),
                "{invalid}"
            );
        }
        broker.finish(first);
        assert_eq!(broker.resolve_wire_id(&first.wire_id()).unwrap(), first);
        let next = add(&broker, None);
        assert_ne!(first, next);
        assert!(matches!(
            broker.resolve_wire_id(&first.wire_id()),
            Err(DebugError::InvalidReference)
        ));
        assert_eq!(broker.resolve_wire_id(&next.wire_id()).unwrap(), next);
    }

    #[test]
    fn detach_retains_ownership_and_disables_controllers_until_close() {
        let broker = DebugTargetBroker::new(2, 8192).unwrap();
        let parent = add(&broker, None);
        assert!(broker.debugger_enabled());
        broker.detach_debuggers().unwrap();
        broker.detach_debuggers().unwrap();
        assert!(!broker.debugger_enabled());
        assert!(matches!(
            broker.take_debugger(parent),
            Err(DebugError::NotAttached)
        ));
        let child = add(&broker, Some(parent));
        assert_eq!(broker.list().unwrap()[1].parent, Some(parent));
        assert_eq!(broker.resolve_wire_id(&child.wire_id()).unwrap(), child);
        assert_eq!(broker.count(), broker.limit());
        broker.close().unwrap();
        assert!(!broker.debugger_enabled());
        assert!(matches!(broker.detach_debuggers(), Err(DebugError::Closed)));
    }

    #[test]
    fn byte_budget_charges_name_capacity_and_failed_admission_is_atomic() {
        let table = 2 * std::mem::size_of::<Entry>();
        let broker = DebugTargetBroker::new(2, table + 32).unwrap();
        let name = JsString::from_units(Vec::with_capacity(16));
        let first = metadata(&broker, None, name).unwrap();
        assert_eq!(broker.retained_bytes(), table + 32);
        let oversized = JsString::from_units(Vec::with_capacity(17));
        assert!(matches!(
            metadata(&broker, None, oversized),
            Err(DebugError::Capacity)
        ));
        assert_eq!(broker.list().unwrap()[0].id, first);
        let mut snapshot = broker.list().unwrap();
        snapshot[0].name = JsString::from_utf8("changed");
        assert!(broker.list().unwrap()[0].name.as_units().is_empty());
        broker.close().unwrap();
        broker.close().unwrap();
        assert_eq!(broker.count(), 0);
        assert_eq!(broker.retained_bytes(), 0);
        assert!(matches!(
            metadata(&broker, None, JsString::default()),
            Err(DebugError::Closed)
        ));
        assert!(matches!(
            DebugTargetBroker::new(0, 8192),
            Err(DebugError::Capacity)
        ));
        assert!(matches!(
            DebugTargetBroker::new(2, table - 1),
            Err(DebugError::Capacity)
        ));
    }
}
