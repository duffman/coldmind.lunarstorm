//! Owner-thread callbacks supplied by an application or a native-engine bridge.

use crate::engine::{CheckpointReport, CompletionPayload, FailureReport, OperationId};
use crate::host::LocalHost;
use std::num::NonZeroUsize;
/// Executes ordinary turns and checkpoints on the event loop's owner thread.
///
/// This trait creates no JavaScript VM, native I/O source, or Promise binding.
/// A bridge keeps native values and operation bindings in its own owner-local
/// tables; producers transfer only owned messages or completion payloads.
///
/// Returning an error, reporting a checkpoint failure, or panicking in a driver
/// callback stops the loop. The owner pump bounds the diagnostic and disposes
/// queued work. Native adapters must also contain panics before crossing an FFI ABI.
pub trait LoopDriver {
    /// Receives the owner-local handle of the loop's reactor before `initialize`.
    ///
    /// Every loop has exactly one reactor; drivers without sockets ignore it.
    ///
    /// # Arguments
    ///
    /// * `_` - handle that spawns socket tasks and publishes their events to this loop
    #[doc(hidden)]
    fn attach_owner_io(&mut self, _: crate::owner_io::OwnerIoHandle<Self::Message>) {}

    /// Connect owner-control metadata after the loop/waker exists and before entry.
    /// Resource-free drivers require no setup. No application work is dispatched.
    fn initialize(
        &mut self,
        _handle: crate::transport::LoopHandle<Self::Message>,
    ) -> Result<(), FailureReport> {
        Ok(())
    }
    /// Service an independent control backend at an owner-safe boundary.
    /// Returns whether control work progressed; ordinary turns are never pumped.
    fn service_control(&mut self) -> Result<bool, FailureReport> {
        Ok(false)
    }
    /// Report owner-runnable control backlog so polling cannot hide admitted work.
    fn has_control_work(&self) -> bool {
        false
    }
    /// Close driver-owned bindings and native context after transport, sources and
    /// queued jobs were disposed. Resource-free drivers need no explicit teardown.
    fn shutdown(&mut self) -> Result<(), FailureReport> {
        Ok(())
    }
    /// Return immutable producer-side message checks. The closure must contain
    /// only transferable policy metadata and must never enter an engine.
    fn message_validator(&self) -> Option<crate::transport::MessageValidator<Self::Message>> {
        None
    }
    /// Owner-local callback state retained through its turn's final checkpoint.
    ///
    /// It need not be `Send`; repeating timers reuse the same job between firings.
    type Job;
    /// Owned application message accepted by the loop's transferable handle.
    ///
    /// Messages must not contain owner-local native value handles.
    type Message: Send + 'static;
    /// Executes one admitted local job without consuming its retained job record.
    ///
    /// The host supports posting, cancellation, and cooperative stop during this
    /// callback. After success, the loop completes a checkpoint before another
    /// ordinary turn; a running timer's job remains alive until that checkpoint ends.
    fn execute_job(
        &mut self,
        job: &mut Self::Job,
        host: &dyn LocalHost<Self::Job>,
    ) -> Result<(), FailureReport>;
    /// Consumes one admitted application message as an ordinary owner-thread turn.
    ///
    /// The message's admission permit remains occupied through the following
    /// checkpoint. Any engine import or message routing belongs to the driver.
    fn deliver_message(
        &mut self,
        message: Self::Message,
        host: &dyn LocalHost<Self::Job>,
    ) -> Result<(), FailureReport>;
    /// Consumes the winning terminal payload for a bound asynchronous operation.
    ///
    /// Resolve `operation` through the driver's owner-local binding table before
    /// touching engine state. This can represent success, failure, cancellation,
    /// abandonment, or oversize rejection. The loop holds reservation capacity
    /// through the following checkpoint; this method does not itself imply native
    /// Promise settlement or implement a native cancellation hook.
    fn deliver_completion(
        &mut self,
        operation: OperationId,
        payload: CompletionPayload,
        host: &dyn LocalHost<Self::Job>,
    ) -> Result<(), FailureReport>;
    /// Advances engine microtasks or other driver-owned checkpoint work.
    ///
    /// Called at startup and after every ordinary turn. `Empty` ends the checkpoint;
    /// `Pending` keeps it active and blocks the next ordinary turn while allowing
    /// later owner slices to observe controls, service bounded native I/O without
    /// dispatching its application messages, and resume checkpoint work. Treat
    /// `hint` as a cooperative budget, not an interruption guarantee. The first
    /// reported script failure stops the loop under the current error policy.
    fn checkpoint(
        &mut self,
        hint: NonZeroUsize,
        host: &dyn LocalHost<Self::Job>,
    ) -> Result<CheckpointReport, FailureReport>;
}
