use super::{FailureCode, FailureReport};
use std::{
    cell::Cell,
    marker::PhantomData,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, ThreadId},
};

static NEXT_ENGINE: AtomicU64 = AtomicU64::new(1);

fn allocate_engine_epoch(counter: &AtomicU64) -> Result<u64, FailureReport> {
    counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| FailureReport::fault(FailureCode::Capacity, "engine identities exhausted"))
}

/// Engine instance identity; local counters never cross threads.
#[derive(Clone, Debug)]
pub struct EngineIdentity {
    epoch: u64,
    owner: ThreadId,
    generation: Rc<Cell<u64>>,
}

impl EngineIdentity {
    /// Issues a fresh engine epoch owned by the current thread.
    ///
    /// Returns [`FailureCode::Capacity`] if the global identity space is exhausted.
    /// Clones share the context-generation counter rather than issuing another engine.
    pub fn new() -> Result<Self, FailureReport> {
        let epoch = allocate_engine_epoch(&NEXT_ENGINE)?;
        Ok(Self {
            epoch,
            owner: thread::current().id(),
            generation: Rc::new(Cell::new(0)),
        })
    }
    /// Returns the thread permitted to access this engine identity.
    pub fn owner(&self) -> ThreadId {
        self.owner
    }
    /// Returns the globally issued engine-instance epoch for diagnostics/table keys.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub(crate) fn context(&self) -> Result<ContextId, FailureReport> {
        if thread::current().id() != self.owner {
            return Err(FailureReport::fault(
                FailureCode::WrongOwner,
                "wrong engine owner",
            ));
        }
        let generation = self.generation.get().checked_add(1).ok_or_else(|| {
            FailureReport::fault(FailureCode::Capacity, "context identities exhausted")
        })?;
        self.generation.set(generation);
        Ok(ContextId {
            engine: self.epoch,
            generation,
            owner: PhantomData,
        })
    }
}

/// Opaque engine/context generation identity that is neither `Send` nor `Sync`.
///
/// Copying the identity does not extend a native context lifetime or make a closed
/// context valid. Adapters validate it before native access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ContextId {
    pub(crate) engine: u64,
    pub(crate) generation: u64,
    owner: PhantomData<Rc<()>>,
}
impl ContextId {
    /// Returns the issuing engine-instance epoch.
    pub fn engine_epoch(self) -> u64 {
        self.engine
    }
    /// Returns the never-reused context generation within its issuing engine.
    pub fn generation(self) -> u64 {
        self.generation
    }
}

/// Transferable scheduler identity; its constructors remain crate-private.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TaskId {
    pub(crate) epoch: u64,
    pub(crate) slot: u32,
    pub(crate) generation: u64,
}
/// Transferable identity of a registered native I/O source.
///
/// Its loop epoch, arena slot, and generation distinguish stale registrations;
/// this is separate from the debugger’s script identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceId {
    pub(crate) epoch: u64,
    pub(crate) slot: u32,
    pub(crate) generation: u64,
}
/// Transferable identity of a reserved asynchronous operation.
///
/// The loop/context epochs, slot, and generation prevent stale completion or
/// cancellation from reaching a replacement operation. It contains no engine value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OperationId {
    pub(crate) epoch: u64,
    pub(crate) context: u64,
    pub(crate) slot: u32,
    pub(crate) generation: u64,
}

#[cfg(test)]
#[path = "../../tests/unit/identity.rs"]
mod tests;
