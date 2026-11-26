//! Identities the debugger hands to a client: issuer epochs, `DebugReference`s for
//! pauses, inspections and writes, and `DebugIdentities`, which checks them on return.

use super::*;
use crate::engine::{ContextId, FailureCode, FailureReport};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::{
    cell::RefCell,
    num::NonZeroU64,
    thread::{self, ThreadId},
};

static NEXT_ISSUER: AtomicU64 = AtomicU64::new(1);

fn allocate_issuer_epoch(counter: &AtomicU64) -> Result<u64, DebugError> {
    counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| DebugError::Capacity)
}

/// Native inspection table category used to validate pause-local references.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceKind {
    /// A native paused stack frame.
    Frame,
    /// A native lexical scope belonging to a paused frame.
    Scope,
    /// A retained native inspection object.
    Object,
    /// A verified lexical binding write target.
    Binding,
    /// A verified own-property write target.
    Property,
    /// A resumable native inspection scan.
    Cursor,
}
impl ReferenceKind {
    fn inspection_local(self) -> bool {
        matches!(self, Self::Binding | Self::Property | Self::Cursor)
    }
}
/// Typed opaque inspection reference issued for exactly one current pause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugReference {
    /// A stack-frame reference; cannot resolve through a scope/object table entry.
    Frame(FrameRef),
    /// A lexical-scope reference; valid only until its pause ends.
    Scope(ScopeRef),
    /// A retained object reference bounded by the issuer’s per-pause capacity.
    Object(ObjectRef),
    /// An inspection-generation-bound lexical write target.
    Binding(BindingRef),
    /// An inspection-generation-bound own-property write target.
    Property(PropertyRef),
    /// An inspection-generation-bound scan continuation.
    Cursor(CursorRef),
}
impl DebugReference {
    /// Returns the issuing pause identity without resolving or retaining native state.
    pub fn pause(self) -> PauseId {
        match self {
            Self::Frame(v) => v.pause,
            Self::Scope(v) => v.pause,
            Self::Object(v) => v.pause,
            Self::Binding(v) => v.pause,
            Self::Property(v) => v.pause,
            Self::Cursor(v) => v.pause,
        }
    }
    fn parts(self) -> (ReferenceKind, u64, u64) {
        match self {
            Self::Frame(v) => (ReferenceKind::Frame, v.slot, v.generation),
            Self::Scope(v) => (ReferenceKind::Scope, v.slot, v.generation),
            Self::Object(v) => (ReferenceKind::Object, v.slot, v.generation),
            Self::Binding(v) => (ReferenceKind::Binding, v.slot, v.generation),
            Self::Property(v) => (ReferenceKind::Property, v.slot, v.generation),
            Self::Cursor(v) => (ReferenceKind::Cursor, v.slot, v.generation),
        }
    }
}
pub(super) struct RequestIssuer {
    session: DebugSessionId,
    counter: Arc<AtomicU64>,
    open: Arc<AtomicBool>,
}
impl RequestIssuer {
    fn check_open(&self) -> Result<(), DebugError> {
        if !self.open.load(Ordering::Acquire) {
            return Err(DebugError::Closed);
        }
        Ok(())
    }
    pub fn issue(&self) -> Result<DebugRequestId, DebugError> {
        self.check_open()?;
        let previous = self
            .counter
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| DebugError::Capacity)?;
        self.check_open()?;
        Ok(DebugRequestId {
            session: self.session,
            sequence: previous + 1,
        })
    }
}
struct State {
    closed: bool,
    session_counter: u64,
    pause_counter: u64,
    script_counter: u64,
    request_counter: Arc<AtomicU64>,
    request_open: Option<Arc<AtomicBool>>,
    update_counter: u64,
    session: Option<DebugSessionId>,
    pause: Option<PauseId>,
    inspection_generation: u64,
    reference_generation: u64,
    references: Vec<(ReferenceKind, NonZeroU64, u64, u64)>,
}
/// Owner-local metadata issuer shared by a coordinator and its native backend.
/// Native keys are adapter table keys, never encoded pointers. This owns no
/// engine, native value, callback, or cross-thread signal.
pub struct DebugIdentities {
    issuer: u64,
    vm: NonZeroU64,
    context: ContextId,
    owner: ThreadId,
    capacity: usize,
    target_capacity: usize,
    state: RefCell<State>,
}
impl DebugIdentities {
    /// Return the immutable context identity, including after metadata closure.
    /// This does not access native engine state or issue a new identity.
    pub fn context(&self) -> ContextId {
        self.context
    }
    /// Return this issuer's unique process epoch for registry ownership checks.
    pub fn issuer_epoch(&self) -> u64 {
        self.issuer
    }
    /// Creates an owner-local issuer for a VM/context pair and reserves reference capacity.
    ///
    /// The caller supplies the VM epoch and actual context identity; no native
    /// engine/session is created or validated. Returns `Capacity` for zero capacity,
    /// allocation failure, or global issuer exhaustion.
    pub fn new(vm: NonZeroU64, context: ContextId, capacity: usize) -> Result<Self, DebugError> {
        Self::new_with_targets(vm, context, capacity, capacity)
    }
    pub(crate) fn new_with_targets(
        vm: NonZeroU64,
        context: ContextId,
        capacity: usize,
        target_capacity: usize,
    ) -> Result<Self, DebugError> {
        if capacity == 0 || target_capacity == 0 {
            return Err(DebugError::Capacity);
        }
        let issuer = allocate_issuer_epoch(&NEXT_ISSUER)?;
        let mut references = Vec::new();
        references
            .try_reserve_exact(
                capacity
                    .checked_add(target_capacity)
                    .ok_or(DebugError::Capacity)?,
            )
            .map_err(|_| DebugError::Capacity)?;
        Ok(Self {
            issuer,
            vm,
            context,
            owner: thread::current().id(),
            capacity,
            target_capacity,
            state: RefCell::new(State {
                closed: false,
                session_counter: 0,
                pause_counter: 0,
                script_counter: 0,
                request_counter: Arc::new(AtomicU64::new(0)),
                request_open: None,
                update_counter: 0,
                session: None,
                pause: None,
                inspection_generation: 1,
                reference_generation: 1,
                references,
            }),
        })
    }
    fn check(&self) -> Result<(), DebugError> {
        if self.owner != thread::current().id() {
            return Err(DebugError::EngineFailure(FailureReport::fault(
                FailureCode::WrongOwner,
                "wrong debugger metadata owner",
            )));
        }
        if self.state.borrow().closed {
            return Err(DebugError::Closed);
        }
        Ok(())
    }
    fn session(state: &State, session: DebugSessionId) -> Result<(), DebugError> {
        if state.session != Some(session) {
            return Err(DebugError::StaleSession);
        }
        Ok(())
    }
    fn pause(state: &State, pause: PauseId) -> Result<(), DebugError> {
        Self::session(state, pause.session)?;
        if state.pause != Some(pause) {
            return Err(DebugError::StalePause);
        }
        Ok(())
    }
    /// Issues a fresh session generation when none is active.
    ///
    /// Returns `InFlight` for an existing session, `Capacity` on counter exhaustion,
    /// or owner/closed errors before mutation. This is metadata, not native attach.
    pub fn open_session(&self) -> Result<DebugSessionId, DebugError> {
        self.check()?;
        let mut s = self.state.borrow_mut();
        if s.session.is_some() {
            return Err(DebugError::InFlight);
        }
        let next = s
            .session_counter
            .checked_add(1)
            .ok_or(DebugError::Capacity)?;
        let session = DebugSessionId {
            issuer_epoch: self.issuer,
            vm_epoch: self.vm.get(),
            engine_epoch: self.context.engine_epoch(),
            context_generation: self.context.generation(),
            generation: next,
        };
        s.session_counter = next;
        s.session = Some(session);
        s.request_open = Some(Arc::new(AtomicBool::new(true)));
        Ok(session)
    }
    /// Issues a fresh pause for the current session and starts an empty reference table.
    ///
    /// Rejects stale session, existing pause (`InFlight`), exhausted counters, or
    /// owner/closed state. Native suspension/publication is the caller’s responsibility.
    pub fn begin_pause(&self, session: DebugSessionId) -> Result<PauseId, DebugError> {
        self.check()?;
        let mut s = self.state.borrow_mut();
        Self::session(&s, session)?;
        if s.pause.is_some() {
            return Err(DebugError::InFlight);
        }
        let next = s.pause_counter.checked_add(1).ok_or(DebugError::Capacity)?;
        let pause = PauseId {
            session,
            generation: next,
        };
        s.pause_counter = next;
        s.pause = Some(pause);
        s.inspection_generation = 1;
        s.references.clear();
        Ok(pause)
    }
    /// Invalidates the current pause and all its inspection references.
    ///
    /// Rejects stale session/pause or owner/closed state before mutation. This
    /// metadata operation does not itself resume native execution.
    pub fn end_pause(&self, pause: PauseId) -> Result<(), DebugError> {
        self.check()?;
        let mut s = self.state.borrow_mut();
        Self::pause(&s, pause)?;
        s.pause = None;
        s.references.clear();
        Ok(())
    }
    /// Issues/deduplicates a typed reference to an adapter-owned nonzero table key.
    ///
    /// Validates current session/pause and owner/liveness, returning `Capacity` when
    /// the reference table is full. Keys are table metadata, never encoded pointers;
    /// no native wrapper is retained by this issuer.
    pub fn register(
        &self,
        pause: PauseId,
        kind: ReferenceKind,
        key: NonZeroU64,
    ) -> Result<DebugReference, DebugError> {
        self.check()?;
        let mut s = self.state.borrow_mut();
        Self::pause(&s, pause)?;
        let epoch = if kind.inspection_local() {
            s.inspection_generation
        } else {
            0
        };
        let existing = s
            .references
            .iter()
            .position(|(stored, native, _, issued_epoch)| {
                *stored == kind && *native == key && *issued_epoch == epoch
            });
        let (slot, generation) = if let Some(slot) = existing {
            (slot, s.references[slot].2)
        } else {
            let generation = if kind.inspection_local() {
                s.reference_generation = s
                    .reference_generation
                    .checked_add(1)
                    .ok_or(DebugError::Capacity)?;
                s.reference_generation
            } else {
                1
            };
            let entry = (kind, key, generation, epoch);
            let reusable = s.references.iter().position(|(kind, _, _, issued_epoch)| {
                kind.inspection_local() && *issued_epoch != s.inspection_generation
            });
            let target = matches!(kind, ReferenceKind::Binding | ReferenceKind::Property);
            let used = s
                .references
                .iter()
                .filter(|(k, _, _, epoch)| {
                    matches!(k, ReferenceKind::Binding | ReferenceKind::Property) == target
                        && (!k.inspection_local() || *epoch == s.inspection_generation)
                })
                .count();
            if used
                >= if target {
                    self.target_capacity
                } else {
                    self.capacity
                }
            {
                return Err(DebugError::Capacity);
            }
            let slot = if let Some(slot) = reusable {
                s.references[slot] = entry;
                slot
            } else {
                let slot = s.references.len();
                s.references.push(entry);
                slot
            };
            (slot, generation)
        };
        let slot = u64::try_from(slot).map_err(|_| DebugError::Capacity)?;
        Ok(match kind {
            ReferenceKind::Frame => DebugReference::Frame(FrameRef {
                pause,
                slot,
                generation: 1,
            }),
            ReferenceKind::Scope => DebugReference::Scope(ScopeRef {
                pause,
                slot,
                generation: 1,
            }),
            ReferenceKind::Object => DebugReference::Object(ObjectRef {
                pause,
                slot,
                generation: 1,
            }),
            ReferenceKind::Binding => DebugReference::Binding(BindingRef {
                pause,
                slot,
                generation,
            }),
            ReferenceKind::Property => DebugReference::Property(PropertyRef {
                pause,
                slot,
                generation,
            }),
            ReferenceKind::Cursor => DebugReference::Cursor(CursorRef {
                pause,
                slot,
                generation,
            }),
        })
    }
    /// Validates current issuer/session/pause, reference kind, slot, and generation.
    ///
    /// Returns the adapter table key only after validation; stale/invalid references
    /// never gain native access. The caller still validates the native table entry.
    pub fn resolve(&self, reference: DebugReference) -> Result<NonZeroU64, DebugError> {
        self.check()?;
        let s = self.state.borrow();
        Self::pause(&s, reference.pause())?;
        let (kind, slot, generation) = reference.parts();
        if !kind.inspection_local() && generation != 1 {
            return Err(DebugError::InvalidReference);
        }
        let slot = usize::try_from(slot).map_err(|_| DebugError::InvalidReference)?;
        let (stored, key, issued, epoch) =
            s.references.get(slot).ok_or(DebugError::InvalidReference)?;
        if kind.inspection_local() && *issued != generation {
            return Err(DebugError::StaleInspection);
        }
        if *stored != kind {
            return Err(DebugError::InvalidReference);
        }
        if kind.inspection_local() && *epoch != s.inspection_generation {
            return Err(DebugError::StaleInspection);
        }
        if *issued != generation {
            return Err(DebugError::InvalidReference);
        }
        Ok(*key)
    }
    /// Releases a completed cursor's metadata slot without reviving prior tokens
    /// when that slot is reused. A stale release cannot revoke its replacement.
    pub fn release_cursor(&self, cursor: CursorRef) -> Result<(), DebugError> {
        self.resolve(DebugReference::Cursor(cursor))?;
        let mut state = self.state.borrow_mut();
        state.references[cursor.slot as usize].3 = 0;
        Ok(())
    }

    /// Validate a release without confusing fabricated future/zero generations
    /// with genuine same-pause retired tokens. Called by engine adapters only.
    #[cfg_attr(not(feature = "quickjs-ng"), allow(dead_code))]
    pub(crate) fn target_for_release(
        &self,
        reference: DebugReference,
    ) -> Result<Option<NonZeroU64>, DebugError> {
        self.check()?;
        let (kind, _, generation) = reference.parts();
        {
            let state = self.state.borrow();
            Self::pause(&state, reference.pause())?;
            if !matches!(kind, ReferenceKind::Binding | ReferenceKind::Property)
                || generation < 2
                || generation > state.reference_generation
            {
                return Err(DebugError::InvalidReference);
            }
        }
        match self.resolve(reference) {
            Ok(key) => Ok(Some(key)),
            Err(DebugError::StaleInspection) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Release an unexposed or abandoned inspection-local target. Reused slots
    /// always receive a fresh generation; stale releases cannot affect successors.
    /// Called by engine adapters only.
    #[cfg_attr(not(feature = "quickjs-ng"), allow(dead_code))]
    pub(crate) fn release_target(&self, reference: DebugReference) -> Result<(), DebugError> {
        let (kind, slot, _) = reference.parts();
        if !matches!(kind, ReferenceKind::Binding | ReferenceKind::Property) {
            return Err(DebugError::InvalidReference);
        }
        self.resolve(reference)?;
        self.state.borrow_mut().references[slot as usize].3 = 0;
        Ok(())
    }

    /// Revokes cursors and write targets after a potentially mutating operation.
    /// Frame, scope and retained object references remain valid for the pause.
    /// Stale slots are reused on later issuance without growing the bounded table.
    pub fn invalidate_inspection(&self, pause: PauseId) -> Result<(), DebugError> {
        self.check()?;
        let mut state = self.state.borrow_mut();
        Self::pause(&state, pause)?;
        state.inspection_generation = state
            .inspection_generation
            .checked_add(1)
            .ok_or(DebugError::Capacity)?;
        Ok(())
    }
    /// Issues a fresh context/engine-bound script identity without retaining source.
    ///
    /// Returns owner/closed errors or `Capacity` on sequence exhaustion. Source
    /// deduplication, revisions, and capacity are coordinator responsibilities.
    pub fn next_script(&self) -> Result<ScriptId, DebugError> {
        self.check()?;
        let mut s = self.state.borrow_mut();
        s.script_counter = s
            .script_counter
            .checked_add(1)
            .ok_or(DebugError::Capacity)?;
        Ok(ScriptId {
            issuer_epoch: self.issuer,
            engine_epoch: self.context.engine_epoch(),
            context_generation: self.context.generation(),
            sequence: s.script_counter,
        })
    }
    /// Issues a fresh request ID for the current session.
    ///
    /// Returns stale-session, owner/closed, or counter-capacity errors. Issuing an ID
    /// does not reserve command/reply capacity or admit a command.
    pub fn next_request(&self, session: DebugSessionId) -> Result<DebugRequestId, DebugError> {
        self.request_issuer(session)?.issue()
    }
    pub(super) fn request_issuer(
        &self,
        session: DebugSessionId,
    ) -> Result<RequestIssuer, DebugError> {
        self.check()?;
        let state = self.state.borrow();
        Self::session(&state, session)?;
        Ok(RequestIssuer {
            session,
            counter: state.request_counter.clone(),
            open: state
                .request_open
                .as_ref()
                .ok_or(DebugError::Closed)?
                .clone(),
        })
    }
    /// Issues the next monotonic execution-state update sequence.
    ///
    /// Returns owner/closed errors or `Capacity` on sequence exhaustion. No update
    /// is published and no native execution state is changed by this metadata call.
    pub fn next_update_sequence(&self) -> Result<u64, DebugError> {
        self.check()?;
        let mut s = self.state.borrow_mut();
        s.update_counter = s
            .update_counter
            .checked_add(1)
            .ok_or(DebugError::Capacity)?;
        Ok(s.update_counter)
    }
    /// Invalidates the current session, pause, and all pause references.
    ///
    /// Rejects stale-session or owner/closed state. Native detach/resume, signal
    /// revocation, and terminal reply publication remain caller obligations.
    pub fn close_session(&self, session: DebugSessionId) -> Result<(), DebugError> {
        self.check()?;
        let mut s = self.state.borrow_mut();
        Self::session(&s, session)?;
        if let Some(open) = s.request_open.take() {
            open.store(false, Ordering::Release);
        }
        s.session = None;
        s.pause = None;
        s.references.clear();
        Ok(())
    }
    /// Permanently marks this owner-local issuer closed and clears active references.
    ///
    /// Idempotent metadata teardown; it does not call native code or publish closure.
    /// Future checked issuance/resolution returns `Closed`.
    pub fn close(&self) {
        let mut s = self.state.borrow_mut();
        s.closed = true;
        if let Some(open) = s.request_open.take() {
            open.store(false, Ordering::Release);
        }
        s.pause = None;
        s.session = None;
        s.references.clear();
    }
    /// Returns the issuer’s current detached/running/paused/closed metadata state.
    ///
    /// It reflects issued session/pause metadata, not an independent native-engine
    /// probe; the coordinator/backend must keep actual execution synchronized.
    pub fn state(&self) -> DebugExecutionState {
        let s = self.state.borrow();
        if s.closed {
            DebugExecutionState::Closed
        } else if let Some(p) = s.pause {
            DebugExecutionState::Paused(p)
        } else if s.session.is_some() {
            DebugExecutionState::Running
        } else {
            DebugExecutionState::Detached
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/debug_identities.rs"]
mod tests;
