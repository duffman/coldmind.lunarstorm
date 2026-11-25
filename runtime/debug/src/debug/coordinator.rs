use super::transport::{ContextControl, Session};
use super::*;
use crate::engine::{ContextId, FailureClass, FailureCode, FailureReport};
use std::{
    cell::{Cell, RefCell},
    num::NonZeroU64,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::{Arc, Weak, atomic::Ordering},
};

fn cleanup(step: &str, body: impl FnOnce() -> Result<(), DebugError>) -> Result<(), DebugError> {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or_else(|_| {
        Err(DebugError::EngineFailure(FailureReport::new(
            FailureClass::Host,
            FailureCode::HostPanic,
            step,
        )))
    })
}

struct Port {
    limits: DebugLimits,
    identities: DebugIdentities,
    sources: RefCell<SourceRegistry>,
    active: RefCell<Option<Arc<Session>>>,
    control: Arc<ContextControl>,
    closed: Cell<bool>,
}
impl Port {
    fn session(&self) -> Result<Arc<Session>, DebugError> {
        if self.closed.get() {
            return Err(DebugError::Closed);
        }
        self.active.borrow().clone().ok_or(DebugError::NotAttached)
    }
    fn project_async_trace(&self, trace: &mut AsyncStackTrace) -> Result<(), DebugError> {
        for entry in &mut trace.entries {
            entry.originals.clear();
        }
        let mut bytes = trace
            .heap_bytes()?
            .checked_add(std::mem::size_of::<DebugReply>())
            .ok_or(DebugError::Capacity)?;
        for entry in &mut trace.entries {
            let before = entry.originals.capacity();
            entry
                .originals
                .try_reserve_exact(entry.record.locations.len())
                .map_err(|_| DebugError::Capacity)?;
            bytes = bytes
                .checked_add(
                    (entry.originals.capacity() - before)
                        .checked_mul(std::mem::size_of::<Option<OriginalLocation>>())
                        .ok_or(DebugError::Capacity)?,
                )
                .ok_or(DebugError::Capacity)?;
            if bytes > self.limits.reply_bytes {
                return Err(DebugError::Capacity);
            }
            for location in &entry.record.locations {
                let original = self.sources.borrow().map_location(location)?;
                bytes = bytes
                    .checked_add(original.as_ref().map_or(0, |o| o.origin.capacity()))
                    .ok_or(DebugError::Capacity)?;
                if bytes > self.limits.reply_bytes {
                    return Err(DebugError::Capacity);
                }
                entry.originals.push(original);
            }
        }
        Ok(())
    }
    fn validate_references(&self, request: &DebugRequest) -> Result<(), DebugError> {
        match request {
            DebugRequest::ReleaseCursor(cursor) => {
                self.identities.resolve(DebugReference::Cursor(*cursor))?;
            }
            DebugRequest::Stack { pause, .. }
            | DebugRequest::ContinueTo { pause, .. }
            | DebugRequest::ExceptionInfo { pause }
            | DebugRequest::AsyncStack { pause } => {
                if self.identities.state() != DebugExecutionState::Paused(*pause) {
                    return Err(DebugError::StalePause);
                }
            }
            DebugRequest::Scopes { frame }
            | DebugRequest::Evaluate { frame, .. }
            | DebugRequest::EvaluateWithOptions { frame, .. }
            | DebugRequest::LookupPath { frame, .. }
            | DebugRequest::Write { frame, .. } => {
                self.identities.resolve(DebugReference::Frame(*frame))?;
            }
            DebugRequest::Variables { parent, .. }
            | DebugRequest::Inspect(InspectionRequest { parent, .. }) => {
                self.identities.resolve(match parent {
                    VariableParent::Scope(scope) => DebugReference::Scope(*scope),
                    VariableParent::Object(object) => DebugReference::Object(*object),
                })?;
            }
            _ => {}
        }
        if let DebugRequest::Inspect(InspectionRequest {
            cursor: Some(cursor),
            ..
        }) = request
        {
            self.identities.resolve(DebugReference::Cursor(*cursor))?;
        }
        if let DebugRequest::Write { target, .. } = request {
            self.identities.resolve(match target {
                WriteTarget::Binding(binding) => DebugReference::Binding(*binding),
                WriteTarget::Property(property) => DebugReference::Property(*property),
            })?;
        }
        Ok(())
    }
    fn finish_detach(
        &self,
        session: &Arc<Session>,
        reason: Option<FailureReport>,
        permanent: bool,
    ) -> Result<(), DebugError> {
        let identity_result = (|| {
            if let DebugExecutionState::Paused(pause) = self.identities.state() {
                self.identities.end_pause(pause)?;
            }
            if matches!(self.identities.state(), DebugExecutionState::Running) {
                self.identities.close_session(session.id)?;
            }
            Ok(())
        })();
        session.close(reason, permanent);
        identity_result
    }
}
impl DebugPort for Port {
    fn evaluation_control(&self, id: DebugRequestId) -> Option<EvaluationControl> {
        self.session().ok()?.evaluation_control(id)
    }
    fn limits(&self) -> &DebugLimits {
        &self.limits
    }
    fn identities(&self) -> &DebugIdentities {
        &self.identities
    }
    fn register_script(
        &self,
        script: NativeScriptMetadata,
    ) -> Result<ScriptRegistration, DebugError> {
        if self.closed.get() || self.control.stopping.load(Ordering::Acquire) {
            return Err(DebugError::Closed);
        }
        let origin = script.source.origin.clone();
        let length_units = script.source.text.as_units().len();
        let registration = self
            .sources
            .borrow_mut()
            .register(&self.identities, script)?;
        // Registration is authoritative and never depends on diagnostic/update
        // queue capacity or on there being an attached protocol client.
        if let Some(session) = self.active.borrow().clone() {
            let _ = session.publish(DebugUpdate::ScriptRegistered(ScriptInfo {
                registration,
                origin,
                length_units,
            }));
        }
        Ok(registration)
    }
    fn resolve_breakpoint(
        &self,
        selector: &ScriptSelector,
        line: u32,
        column: Option<u32>,
    ) -> Result<Vec<DebugLocation>, DebugError> {
        if self.closed.get() {
            return Err(DebugError::Closed);
        }
        self.sources
            .borrow()
            .resolve_breakpoint(selector, line, column)
    }
    fn select_script(
        &self,
        selector: &ScriptSelector,
    ) -> Result<Option<ScriptRegistration>, DebugError> {
        if self.closed.get() {
            return Err(DebugError::Closed);
        }
        self.sources.borrow().select(selector)
    }
    fn is_current_script(&self, location: &DebugLocation) -> Result<bool, DebugError> {
        if self.closed.get() {
            return Err(DebugError::Closed);
        }
        self.sources.borrow().is_current_script(location)
    }
    fn original_locations(
        &self,
        selector: &ScriptSelector,
        location: &DebugLocation,
    ) -> Result<Vec<OriginalLocation>, DebugError> {
        if self.closed.get() {
            return Err(DebugError::Closed);
        }
        self.sources.borrow().original_locations(selector, location)
    }
    fn skip_source(
        &self,
        location: &DebugLocation,
        policy: &StepPolicy,
    ) -> Result<bool, DebugError> {
        if self.closed.get() {
            return Err(DebugError::Closed);
        }
        self.sources.borrow().skip_source(location, policy)
    }
    fn next_directive(&self, wait: bool) -> Option<DebugDirective> {
        loop {
            let session = self.session().ok()?;
            let directive = session.next(wait)?;
            let DebugDirective::Request(command) = directive else {
                return Some(directive);
            };
            if let Err(error) = self.validate_references(&command.request) {
                let _ = session.publish(DebugUpdate::Reply {
                    id: command.id,
                    result: Err(error),
                });
                continue;
            }
            let mutation = match &command.request {
                DebugRequest::Evaluate {
                    frame,
                    allow_side_effects: true,
                    ..
                }
                | DebugRequest::EvaluateWithOptions {
                    frame,
                    options:
                        EvaluationOptions {
                            allow_side_effects: true,
                            ..
                        },
                    ..
                } => Some(frame.pause),
                _ => None,
            };
            if let Some(pause) = mutation
                && let Err(error) = self.identities.invalidate_inspection(pause)
            {
                let _ = session.publish(DebugUpdate::Reply {
                    id: command.id,
                    result: Err(error),
                });
                continue;
            }
            let result = match &command.request {
                DebugRequest::Sources { page } => Some(self.sources.borrow().sources(*page)),
                DebugRequest::OriginalSource { selector, page } => Some(
                    self.sources
                        .borrow()
                        .original_source(selector, *page)
                        .and_then(|reply| reply.ok_or(DebugError::InvalidSource)),
                ),
                DebugRequest::Source {
                    script,
                    revision,
                    page,
                } => Some(self.sources.borrow().source(*script, *revision, *page)),
                _ => None,
            };
            if let Some(result) = result {
                let _ = session.publish(DebugUpdate::Reply {
                    id: command.id,
                    result,
                });
                continue;
            }
            return Some(DebugDirective::Request(command));
        }
    }
    fn publish(&self, mut update: DebugUpdate) -> Result<(), DebugError> {
        let session = self.session()?;
        match &mut update {
            DebugUpdate::Paused {
                location, original, ..
            } => {
                *original = location
                    .as_ref()
                    .map(|location| self.sources.borrow().map_location(location))
                    .transpose()?
                    .flatten();
            }
            DebugUpdate::Reply {
                result: Ok(DebugReply::Stack { frames, .. }),
                ..
            } => {
                for frame in frames {
                    frame.original = frame
                        .generated
                        .as_ref()
                        .map(|location| self.sources.borrow().map_location(location))
                        .transpose()?
                        .flatten();
                }
            }
            DebugUpdate::PromiseRejection(info) => {
                info.original = info
                    .location
                    .as_ref()
                    .map(|location| self.sources.borrow().map_location(location))
                    .transpose()?
                    .flatten();
            }
            DebugUpdate::Reply {
                id,
                result: Ok(DebugReply::PromiseRejections { records, .. }),
            } => {
                let mut bytes = std::mem::size_of::<DebugReply>()
                    + records.capacity() * std::mem::size_of::<PromiseRejectionInfo>();
                for record in records {
                    record.original = record
                        .location
                        .as_ref()
                        .map(|location| self.sources.borrow().map_location(location))
                        .transpose()?
                        .flatten();
                    bytes = bytes
                        .checked_add(record.description.capacity_bytes())
                        .and_then(|n| {
                            n.checked_add(
                                record.original.as_ref().map_or(0, |o| o.origin.capacity()),
                            )
                        })
                        .ok_or(DebugError::Capacity)?;
                    if bytes > self.limits.reply_bytes {
                        return session.publish(DebugUpdate::Reply {
                            id: *id,
                            result: Err(DebugError::Capacity),
                        });
                    }
                }
            }
            DebugUpdate::Reply {
                id,
                result: Ok(DebugReply::AsyncStack(trace)),
            } => {
                if let Err(error) = self.project_async_trace(trace) {
                    // Projection is part of this request's reserved terminal reply;
                    // a tight reply budget must not unwind the paused application.
                    return session.publish(DebugUpdate::Reply {
                        id: *id,
                        result: Err(error),
                    });
                }
            }
            DebugUpdate::Reply {
                result: Ok(DebugReply::ExceptionInfo(info)),
                ..
            } => {
                for frame in &mut info.stack {
                    frame.original = frame
                        .generated
                        .as_ref()
                        .map(|location| self.sources.borrow().map_location(location))
                        .transpose()?
                        .flatten();
                }
            }
            _ => {}
        }
        match &update {
            DebugUpdate::Paused {
                pause, location, ..
            } => {
                if self.identities.state() != DebugExecutionState::Paused(*pause) {
                    return Err(DebugError::StalePause);
                }
                if let Some(location) = location {
                    self.sources.borrow().map_locations(location)?;
                }
            }
            DebugUpdate::Resumed { previous, .. } => {
                if previous.session != session.id {
                    return Err(DebugError::StaleSession);
                }
                if matches!(self.identities.state(), DebugExecutionState::Paused(_)) {
                    return Err(DebugError::InFlight);
                }
            }
            DebugUpdate::SessionClosed {
                session: closed,
                reason,
            } => {
                if *closed != session.id {
                    return Err(DebugError::StaleSession);
                }
                // The backend has resumed/unwound its native paused state before
                // reporting closure. Signal revocation happens without locks.
                self.finish_detach(&session, reason.clone(), false)?;
                return Ok(());
            }
            _ => {}
        }
        session.publish(update)
    }
    fn is_stopping(&self) -> bool {
        self.closed.get() || self.control.stopping.load(Ordering::Acquire)
    }
}
/// Owner-thread coordinator binding one truthful native backend to bounded control.
///
/// It initializes permanent source hooks before application entry and owns no
/// application scheduler. Native callbacks use the same permanent port and its
/// restricted Condvar control pump. The port retains no backend/coordinator,
/// preventing native ownership cycles. Rust fixtures do not qualify an engine.
pub struct DebugCoordinator {
    backend: Rc<dyn DebugBackend>,
    port: Rc<Port>,
    limits: DebugLimits,
    closed: Cell<bool>,
}
impl DebugCoordinator {
    /// Validates development capabilities/limits and initializes permanent hooks.
    ///
    /// Native initialization runs once, outside registry/transport borrows. Failure
    /// exposes no controller. `wake_owner` must wake the host's existing scheduler;
    /// a separate revocable native signal handles executing/paused JavaScript.
    pub fn new(
        backend: Rc<dyn DebugBackend>,
        context: ContextId,
        limits: DebugLimits,
        wake_owner: Arc<dyn Fn() -> Result<(), FailureReport> + Send + Sync>,
        vm_epoch: NonZeroU64,
    ) -> Result<Rc<Self>, DebugError> {
        if !backend
            .descriptor()
            .capabilities
            .contains_all(DebugCapabilities::baseline())
        {
            return Err(DebugError::Unsupported);
        }
        if limits.requests == 0
            || limits.request_bytes == 0
            || limits.reply_bytes == 0
            || limits.inspection_objects == 0
            || limits.write_targets == 0
            || limits.inspection_page_items == 0
            || limits.inspection_scan_slots == 0
            || limits.expression_units == 0
        {
            return Err(DebugError::Capacity);
        }
        // Worst-case JSON escaping uses six ASCII bytes per retained UTF-16 unit.
        // Source bytes already count two bytes/unit; protocol envelope headroom is
        // checked separately by the actual DAP encoder before output allocation.
        if limits
            .source_bytes
            .checked_mul(3)
            .is_none_or(|minimum| limits.dap_output_bytes < minimum)
        {
            return Err(DebugError::Capacity);
        }
        for deadline_ms in [limits.evaluation_deadline_ms, limits.condition_deadline_ms] {
            EvaluationControl::new(EvaluationOptions {
                deadline_ms,
                ..EvaluationOptions::default()
            })?;
        }
        let identities = DebugIdentities::new_with_targets(
            vm_epoch,
            context,
            limits.inspection_objects,
            limits.write_targets,
        )?;
        let sources = SourceRegistry::new(context, limits.clone())?;
        let control = ContextControl::new(wake_owner);
        let port = Rc::new(Port {
            limits: limits.clone(),
            identities,
            sources: RefCell::new(sources),
            active: RefCell::new(None),
            control,
            closed: Cell::new(false),
        });
        backend.initialize(port.clone())?;
        Ok(Rc::new(Self {
            backend,
            port,
            limits,
            closed: Cell::new(false),
        }))
    }
    /// Attaches one issued session before application entry and returns its controller.
    ///
    /// Native attachment must not wait for application startup before returning.
    /// Failure closes the issued metadata/session without retaining native signals.
    pub fn attach(&self, options: DebugSessionOptions) -> Result<VmDebugHandle, DebugError> {
        if self.closed.get() || self.is_stopping() {
            return Err(DebugError::Closed);
        }
        let id = self.port.identities.open_session()?;
        let issuer = self.port.identities.request_issuer(id)?;
        let session = match Session::new(
            id,
            options.break_on_start,
            issuer,
            self.backend.descriptor().clone(),
            self.limits.clone(),
            self.port.control.clone(),
        ) {
            Ok(session) => session,
            Err(error) => {
                self.port.identities.close_session(id)?;
                return Err(error);
            }
        };
        *self.port.active.borrow_mut() = Some(session.clone());
        *self
            .port
            .control
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&session);
        match self.backend.attach(DebugAttachOptions {
            session: id,
            break_on_start: options.break_on_start,
        }) {
            Ok(signal) => session.signal.install(signal),
            Err(error) => {
                let _ = self.port.finish_detach(&session, None, false);
                self.port.active.borrow_mut().take();
                *self
                    .port
                    .control
                    .active
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Weak::new();
                return Err(error);
            }
        }
        Ok(VmDebugHandle { session })
    }
    /// Services native controls at an owner-safe boundary, never ordinary loop work.
    ///
    /// Native callbacks invoke their dispatcher directly through the permanent port.
    /// The backend must not independently reacquire an already active mutable runtime.
    pub fn service(&self) -> Result<(), DebugError> {
        if self.closed.get() {
            return Ok(());
        }
        let active = self.port.active.borrow().clone();
        if let Some(session) = active {
            if !session.pending() {
                return Ok(());
            }
            if !matches!(
                session.lock().execution,
                DebugExecutionState::Detached | DebugExecutionState::Closed
            ) {
                let report = self.backend.service()?;
                if report.state != self.port.identities.state() {
                    return Err(DebugError::EngineFailure(FailureReport::fault(
                        FailureCode::EngineFailure,
                        "native debug state does not match issued metadata",
                    )));
                }
            }
            let fault = session.lock().fault.clone();
            if let Some(error) = fault {
                return Err(error);
            }
        }
        Ok(())
    }
    /// Holds application entry behind an active DAP configuration barrier.
    ///
    /// Call immediately before opening an application engine/root frame, without
    /// retaining bridge, registry, or transport borrows. Only native debug service
    /// runs while waiting; the debug Condvar blocks idle waits. Detach releases the
    /// gate by default; stop/closed context rejects entry. This host guard does not
    /// manufacture a native pause or replace the required first-instruction hook.
    pub fn before_entry(&self) -> Result<(), DebugError> {
        if self.closed.get() {
            return Err(DebugError::Closed);
        }
        loop {
            // Source/configuration controls admitted before this boundary run
            // before application entry; idle sessions take only atomic reads.
            self.service()?;
            let Some(session) = self.port.active.borrow().clone() else {
                return if self.is_stopping() {
                    Err(DebugError::Closed)
                } else {
                    Ok(())
                };
            };
            if !session.entry_held() {
                return if self.is_stopping() {
                    Err(DebugError::Closed)
                } else {
                    Ok(())
                };
            }
            self.service()?;
            if self.is_stopping() {
                return Err(DebugError::Closed);
            }
            if !session.entry_held() {
                return Ok(());
            }
            session.wait_entry_work();
        }
    }
    /// Returns authoritative issued session/pause metadata on the owner thread.
    pub fn state(&self) -> DebugExecutionState {
        if self.closed.get() {
            DebugExecutionState::Closed
        } else {
            self.port.identities.state()
        }
    }
    /// Reports owner-runnable debug commands/controls before an ordinary poll sleep.
    ///
    /// An empty paused mailbox is not runnable; its native restricted pump waits.
    pub fn pending(&self) -> bool {
        self.port
            .active
            .borrow()
            .as_ref()
            .is_some_and(|session| session.pending())
    }
    /// Alias used by VM owner-safe polling integration.
    pub fn has_control_work(&self) -> bool {
        self.pending()
    }
    /// Returns a transferable metadata-only urgent VM-stop publisher.
    ///
    /// Route ordinary VM stop here as well as to the scheduler: a poller wake alone
    /// cannot release a suspended or executing native debug control path.
    pub fn stop_signal(
        &self,
    ) -> Arc<dyn Fn() -> Result<DebugControlReceipt, DebugError> + Send + Sync> {
        let control = self.port.control.clone();
        Arc::new(move || control.stop())
    }
    /// Reports authoritative context stop/close state without native execution.
    pub fn is_stopping(&self) -> bool {
        self.port.is_stopping()
    }
    /// Closes admission, native observers/signals, source storage, and checked IDs.
    ///
    /// Invoke after application native calls have unwound. Signal revocation waits
    /// for entered short signal calls without holding any transport/registry lock.
    /// Cleanup proceeds even if native detach reports an error.
    pub fn shutdown(&self) -> Result<(), DebugError> {
        if self.closed.replace(true) {
            return Ok(());
        }
        self.port.closed.set(true);
        self.port.control.stopping.store(true, Ordering::Release);
        let session = self.port.active.borrow().clone();
        let mut failure = None;
        if let Some(session) = &session
            && let Err(error) = cleanup("debug stop signal panicked", || match session.stop()? {
                DebugControlReceipt::AcceptedWakeFailed { error } => Err(error),
                _ => Ok(()),
            })
        {
            failure.get_or_insert(error);
        }
        // Detach may use the port while tearing down; keep it callable until the
        // native observer cleanup has run, while is_stopping already blocks entry.
        self.port.closed.set(false);
        if matches!(
            self.port.identities.state(),
            DebugExecutionState::Running | DebugExecutionState::Paused(_)
        ) && let Err(error) =
            cleanup("native debugger detach panicked", || self.backend.detach())
        {
            failure.get_or_insert(error);
        }
        self.port.closed.set(true);
        if let Some(session) = session
            && let Err(error) = cleanup("debug session disposal panicked", || {
                self.port.finish_detach(&session, None, true)
            })
        {
            failure.get_or_insert(error);
        }
        self.port.identities.close();
        self.port.sources.borrow_mut().close();
        *self
            .port
            .control
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Weak::new();
        failure.map_or(Ok(()), Err)
    }
}
impl Drop for DebugCoordinator {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
#[path = "../../tests/unit/debug_coordinator.rs"]
mod tests;
