use super::{ContextId, ContextLimits, EngineIdentity, FailureCode, FailureReport};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    fmt,
    marker::PhantomData,
    rc::{Rc, Weak},
    thread,
};

/// Copyable, owner-local token tied to one active value scope.
///
/// The lifetime prevents escape; context/scope/slot checks reject foreign tokens.
/// It contains no exposed native pointer and is neither `Send` nor `Sync`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopedValue<'s> {
    context: ContextId,
    scope: u64,
    slot: usize,
    lifetime: PhantomData<&'s Rc<()>>,
}
#[derive(Clone, Copy, Debug)]
struct RootKey {
    slot: usize,
    generation: u64,
}
trait ReleaseQueue {
    fn release(&self, key: RootKey);
}
struct RootLease {
    context: ContextId,
    key: RootKey,
    release: Weak<dyn ReleaseQueue>,
}
impl Drop for RootLease {
    fn drop(&mut self) {
        if let Some(queue) = self.release.upgrade() {
            queue.release(self.key);
        }
    }
}

/// A root is owner-local and cannot be duplicated implicitly.
///
/// ```compile_fail
/// use coldmind_runtime::engine::RootedValue;
/// fn transfer(root: RootedValue) { std::thread::spawn(move || drop(root)); }
/// ```
///
/// Explicit retention preserves native value identity after a scope ends.
/// Final drop queues release; it does not execute JavaScript or keep a closed
/// context alive. Use [`super::JsScope::retain`] for another ownership token.
pub struct RootedValue {
    lease: Rc<RootLease>,
}
impl fmt::Debug for RootedValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RootedValue")
            .field("context", &self.lease.context)
            .field("key", &self.lease.key)
            .finish()
    }
}
#[allow(dead_code, reason = "lunarstorm: called by jetbunny's VM call path (src/vm), which is not taken")]
impl RootedValue {
    /// Private VM lease on the same ownership record; it creates no new native
    /// root and never exposes implicit public Clone semantics.
    pub(crate) fn local_lease(&self) -> Self {
        Self {
            lease: self.lease.clone(),
        }
    }
    pub(crate) fn context_id(&self) -> ContextId {
        self.lease.context
    }
}
/// Owned, non-cloneable Promise resolver capability bound to its context.
///
/// Dropping queues root release without entering the engine. This capability
/// remains owner-local; workers receive a [`super::CompletionTicket`] instead.
pub struct RootedDeferred {
    root: RootedValue,
}
impl fmt::Debug for RootedDeferred {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RootedDeferred").field(&self.root).finish()
    }
}
impl RootedDeferred {
    /// Returns the resolver’s context identity without extending its native lifetime.
    pub fn context(&self) -> ContextId {
        self.root.lease.context
    }
}

struct RootSlot<V> {
    generation: u64,
    value: Option<Rc<V>>,
}
struct RootTable<V> {
    slots: Vec<RootSlot<V>>,
    free: Vec<usize>,
    live: usize,
}
struct StoreState<V> {
    id: ContextId,
    owner: thread::ThreadId,
    limits: ContextLimits,
    closed: Cell<bool>,
    active: Cell<usize>,
    sequence: Cell<u64>,
    module_sequence: Cell<u64>,
    roots: RefCell<RootTable<V>>,
    release: RefCell<VecDeque<RootKey>>,
    frames: RefCell<Vec<Vec<Rc<V>>>>,
}
impl<V> ReleaseQueue for StoreState<V> {
    fn release(&self, key: RootKey) {
        if !self.closed.get() {
            self.release.borrow_mut().push_back(key);
        }
    }
}
thread_local! {
    /// This thread's id, read once per thread.
    static CURRENT_THREAD: thread::ThreadId = thread::current().id();
}

/// The calling thread's id.
///
/// Fix: every value insert, read and scope entry compared the owner with
/// `thread::current().id()`, which clones and drops the thread's handle (two atomic
/// operations) each time. Decision: the id is cached in a thread-local, so the owner check
/// is one thread-local read and a compare. The check itself stays on every operation.
///
/// # Returns
/// The id `thread::current().id()` returns on this thread.
pub(crate) fn current_thread() -> thread::ThreadId {
    CURRENT_THREAD.with(|id| *id)
}

impl<V> StoreState<V> {
    fn check(&self) -> Result<(), FailureReport> {
        if self.owner != current_thread() {
            return Err(FailureReport::fault(
                FailureCode::WrongOwner,
                "wrong value owner",
            ));
        }
        if self.closed.get() {
            return Err(FailureReport::fault(
                FailureCode::ClosedContext,
                "context is closed",
            ));
        }
        Ok(())
    }
    fn close(&self) -> Result<(), FailureReport> {
        if self.owner != current_thread() {
            return Err(FailureReport::fault(
                FailureCode::WrongOwner,
                "wrong value owner",
            ));
        }
        if self.closed.get() {
            return Ok(());
        }
        if self.active.get() != 0 {
            return Err(FailureReport::fault(
                FailureCode::ActiveContext,
                "context has active scopes",
            ));
        }
        self.closed.set(true);
        let values: Vec<_> = {
            let mut roots = self.roots.borrow_mut();
            roots.live = 0;
            roots
                .slots
                .iter_mut()
                .filter_map(|slot| slot.value.take())
                .collect()
        };
        self.release.borrow_mut().clear();
        drop(values);
        Ok(())
    }
}

/// Optional adapter-side ownership primitive. This stores Rust/native wrappers,
/// never interprets JavaScript, and does not replace an engine's GC.
pub struct ValueStore<V: 'static> {
    state: Rc<StoreState<V>>,
}
impl<V: 'static> ValueStore<V> {
    /// Creates a checked wrapper store and fresh context identity on the engine owner.
    ///
    /// Validates positive limits and reserves root/release capacity up front. Returns
    /// `WrongOwner` for a foreign owner and `Capacity` for invalid limits, exhausted
    /// identity space, or unavailable checked allocation. `V` remains adapter-owned
    /// wrapper data; creating this store does not create a native JavaScript context.
    pub fn new(identity: &EngineIdentity, limits: ContextLimits) -> Result<Self, FailureReport> {
        limits.validate()?;
        let id = identity.context()?;
        let capacity = limits.max_persistent_roots;
        let allocation_failure =
            |_| FailureReport::fault(FailureCode::Capacity, "root storage capacity unavailable");
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(capacity)
            .map_err(allocation_failure)?;
        slots.extend((0..capacity).map(|_| RootSlot {
            generation: 1,
            value: None,
        }));
        let mut free = Vec::new();
        free.try_reserve_exact(capacity)
            .map_err(allocation_failure)?;
        free.extend((0..capacity).rev());
        let mut release = VecDeque::new();
        release
            .try_reserve_exact(capacity)
            .map_err(allocation_failure)?;
        let roots = RootTable {
            slots,
            free,
            live: 0,
        };
        Ok(Self {
            state: Rc::new(StoreState {
                id,
                owner: identity.owner(),
                limits,
                closed: Cell::new(false),
                active: Cell::new(0),
                sequence: Cell::new(0),
                module_sequence: Cell::new(0),
                roots: RefCell::new(roots),
                release: RefCell::new(release),
                frames: RefCell::new(Vec::new()),
            }),
        })
    }
    /// Returns the context identity assigned to this wrapper store.
    pub fn context(&self) -> ContextId {
        self.state.id
    }
    /// Issue opaque module metadata for an adapter's own native module table.
    /// The adapter still owns, validates, and releases the corresponding module.
    /// Returns owner/closed-context errors or `Capacity` on identity exhaustion;
    /// issuing metadata does not create, link, or retain a native module.
    pub fn issue_module_id(&self) -> Result<super::ModuleId, FailureReport> {
        self.state.check()?;
        let slot = self
            .state
            .module_sequence
            .get()
            .checked_add(1)
            .ok_or_else(|| {
                FailureReport::fault(FailureCode::Capacity, "module identities exhausted")
            })?;
        self.state.module_sequence.set(slot);
        Ok(super::ModuleId {
            context: self.state.id,
            slot,
        })
    }
    /// Reports whether this store has been invalidated.
    pub fn is_closed(&self) -> bool {
        self.state.closed.get()
    }
    /// Returns allocated persistent root slots, including drops awaiting a flush.
    pub fn live_roots(&self) -> usize {
        self.state.roots.borrow().live
    }
    /// Invalidates the inactive store and releases its native wrapper roots.
    ///
    /// Repeated close is harmless. Returns `WrongOwner` on a foreign thread or
    /// `ActiveContext` while a value scope is active, leaving the store unchanged.
    /// Outstanding tokens then fail access and may be dropped safely.
    pub fn close(&self) -> Result<(), FailureReport> {
        self.state.close()
    }

    /// Scoped tokens cannot escape the callback.
    ///
    /// ```compile_fail
    /// use coldmind_runtime::engine::{ContextLimits, EngineIdentity, ValueStore};
    /// let owner = EngineIdentity::new().unwrap();
    /// let store = ValueStore::new(&owner, ContextLimits::default()).unwrap();
    /// let escaped = store.with_scope(|scope| scope.insert(42)).unwrap();
    /// println!("{:?}", escaped);
    /// ```
    /// Runs `body` once with temporary roots protected until scope exit.
    ///
    /// Returns owner/closed-context errors, `CallbackDepth` at the nesting limit,
    /// or `Capacity` if scope identities are exhausted. Callback errors propagate.
    /// Local values are cleared on normal return or unwind; no microtasks or
    /// persistent-release flush occurs automatically.
    pub fn with_scope<R>(
        &self,
        body: impl FnOnce(&ValueScope<'_, V>) -> Result<R, FailureReport>,
    ) -> Result<R, FailureReport> {
        self.state.check()?;
        if self.state.active.get() >= self.state.limits.max_host_depth {
            return Err(FailureReport::fault(
                FailureCode::CallbackDepth,
                "scope depth exceeded",
            ));
        }
        let sequence = self.state.sequence.get().checked_add(1).ok_or_else(|| {
            FailureReport::fault(FailureCode::Capacity, "scope identities exhausted")
        })?;
        self.state.sequence.set(sequence);
        let values = self.state.frames.borrow_mut().pop().unwrap_or_default();
        self.state.active.set(self.state.active.get() + 1);
        let scope = ValueScope {
            state: &self.state,
            id: sequence,
            values: RefCell::new(values),
        };
        body(&scope)
    }
    /// Releases queued roots after all value scopes have unwound.
    ///
    /// Returns `WrongOwner`, `ClosedContext`, or `ActiveContext` before release when
    /// its preconditions fail. Native wrapper destructors run outside table borrows;
    /// reusable slots advance generation, and exhausted generations retire.
    pub fn flush_releases(&self) -> Result<(), FailureReport> {
        self.state.check()?;
        if self.state.active.get() != 0 {
            return Err(FailureReport::fault(
                FailureCode::ActiveContext,
                "native frames must unwind before release",
            ));
        }
        loop {
            let key = self.state.release.borrow_mut().pop_front();
            let Some(key) = key else {
                break;
            };
            let value = {
                let mut roots = self.state.roots.borrow_mut();
                let slot = &mut roots.slots[key.slot];
                if slot.generation != key.generation || slot.value.is_none() {
                    continue;
                }
                let value = slot.value.take();
                if let Some(next) = slot.generation.checked_add(1) {
                    slot.generation = next;
                    roots.free.push(key.slot);
                }
                roots.live -= 1;
                value
            };
            drop(value);
        }
        Ok(())
    }
}
impl<V: 'static> Drop for ValueStore<V> {
    fn drop(&mut self) {
        let _ = self.state.close();
    }
}

/// Adapter-local frame protecting native wrappers for the frame’s lifetime.
///
/// Created by [`ValueStore::with_scope`]; the frame and its tokens cannot escape
/// the callback. The store manages wrapper ownership, not native GC semantics.
pub struct ValueScope<'a, V: 'static> {
    state: &'a Rc<StoreState<V>>,
    id: u64,
    values: RefCell<Vec<Rc<V>>>,
}
impl<V: 'static> ValueScope<'_, V> {
    fn insert_rc(&self, value: Rc<V>) -> Result<ScopedValue<'_>, FailureReport> {
        self.state.check()?;
        let mut values = self.values.borrow_mut();
        let slot = values.len();
        values.push(value);
        Ok(ScopedValue {
            context: self.state.id,
            scope: self.id,
            slot,
            lifetime: PhantomData,
        })
    }
    /// Takes an adapter wrapper and protects it until this frame exits.
    ///
    /// Returns owner/closed-context failures before insertion. The resulting token
    /// is borrowed from this scope and consumes no persistent root slot.
    pub fn insert(&self, value: V) -> Result<ScopedValue<'_>, FailureReport> {
        self.insert_rc(Rc::new(value))
    }
    /// Localize an adapter-owned native wrapper without creating a persistent token.
    /// The supplied `Rc` is retained by this frame; the returned token borrows
    /// the frame. Returns owner/closed-context failures before insertion.
    pub fn insert_shared(&self, value: Rc<V>) -> Result<ScopedValue<'_>, FailureReport> {
        self.insert_rc(value)
    }
    /// Only adapters use this native-wrapper accessor; JsScope exposes no V.
    /// Release all registry borrows before invoking any user/native callback.
    /// Returns a cloned wrapper `Rc`, not a new persistent root. Checks owner,
    /// closed context, context identity, scope identity, and slot before access.
    /// The adapter must keep actual native GC/root lifetime guarantees valid.
    pub fn read(&self, value: ScopedValue<'_>) -> Result<Rc<V>, FailureReport> {
        self.state.check()?;
        if value.context != self.state.id {
            return Err(FailureReport::fault(
                FailureCode::WrongContext,
                "foreign context value",
            ));
        }
        if value.scope != self.id {
            return Err(FailureReport::fault(
                FailureCode::InvalidHandle,
                "foreign scope value",
            ));
        }
        self.values
            .borrow()
            .get(value.slot)
            .cloned()
            .ok_or_else(|| FailureReport::fault(FailureCode::InvalidHandle, "invalid scoped slot"))
    }
    /// Creates a distinct persistent ownership token for this scoped wrapper.
    ///
    /// Validates the value and returns `Capacity` when the root table is full.
    /// Dropping the token queues release until an inactive store flush.
    pub fn retain(&self, value: ScopedValue<'_>) -> Result<RootedValue, FailureReport> {
        let value = self.read(value)?;
        let key = {
            let mut roots = self.state.roots.borrow_mut();
            let slot = roots.free.pop().ok_or_else(|| {
                FailureReport::fault(FailureCode::Capacity, "persistent root limit reached")
            })?;
            let record = &mut roots.slots[slot];
            record.value = Some(value);
            let key = RootKey {
                slot,
                generation: record.generation,
            };
            roots.live += 1;
            key
        };
        let queue: Rc<dyn ReleaseQueue> = self.state.clone();
        Ok(RootedValue {
            lease: Rc::new(RootLease {
                context: self.state.id,
                key,
                release: Rc::downgrade(&queue),
            }),
        })
    }
    /// Borrows a persistent wrapper into this frame without another root slot.
    ///
    /// Checks owner/context liveness and root context, slot, and generation;
    /// foreign or stale roots fail before wrapper access.
    pub fn local(&self, root: &RootedValue) -> Result<ScopedValue<'_>, FailureReport> {
        self.state.check()?;
        if root.lease.context != self.state.id {
            return Err(FailureReport::fault(
                FailureCode::WrongContext,
                "foreign root",
            ));
        }
        let value = {
            let roots = self.state.roots.borrow();
            let slot = roots.slots.get(root.lease.key.slot).ok_or_else(|| {
                FailureReport::fault(FailureCode::InvalidHandle, "invalid root slot")
            })?;
            if slot.generation != root.lease.key.generation {
                return Err(FailureReport::fault(
                    FailureCode::InvalidHandle,
                    "stale root",
                ));
            }
            slot.value
                .clone()
                .ok_or_else(|| FailureReport::fault(FailureCode::InvalidHandle, "released root"))?
        };
        self.insert_rc(value)
    }
    /// Retains a scoped wrapper as an owner-local settlement capability.
    ///
    /// The adapter must supply an actual native resolver; this storage helper does
    /// not establish Promise semantics. Validation and capacity errors match `retain`.
    pub fn retain_deferred(&self, value: ScopedValue<'_>) -> Result<RootedDeferred, FailureReport> {
        Ok(RootedDeferred {
            root: self.retain(value)?,
        })
    }
    /// Borrows a retained resolver wrapper into this scope.
    ///
    /// Validation and failures match [`Self::local`]; the capability is not consumed.
    pub fn local_deferred(&self, value: &RootedDeferred) -> Result<ScopedValue<'_>, FailureReport> {
        self.local(&value.root)
    }
}
impl<V: 'static> Drop for ValueScope<'_, V> {
    fn drop(&mut self) {
        // Native wrapper destructors run while the frame is still active. This
        // guard releases activity after their normal or unwinding destruction.
        struct Activity<'a>(&'a Cell<usize>);
        impl Drop for Activity<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() - 1);
            }
        }
        let _activity = Activity(&self.state.active);
        let mut values = std::mem::take(self.values.get_mut());
        values.clear();
        self.state.frames.borrow_mut().push(values);
    }
}

#[cfg(test)]
#[path = "../../tests/unit/values.rs"]
mod tests;
