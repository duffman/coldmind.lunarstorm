//! Detached Promise rejection identities and checkpoint-qualified state.
// `DebugError` and `ContextId` serve the constructor only an engine adapter calls.
#[cfg(feature = "quickjs-ng")]
use super::DebugError;
use super::{AsyncStackId, DebugLocation, OriginalLocation};
#[cfg(feature = "quickjs-ng")]
use crate::engine::ContextId;
use crate::engine::JsString;
use std::num::NonZeroU64;

/// Opaque context-bound Promise identity, separate from live frame/object handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PromiseRejectionId {
    engine_epoch: u64,
    context_generation: u64,
    serial: NonZeroU64,
}
impl PromiseRejectionId {
    #[cfg(feature = "quickjs-ng")]
    pub(crate) fn new(context: ContextId, serial: u64) -> Result<Self, DebugError> {
        Ok(Self {
            engine_epoch: context.engine_epoch(),
            context_generation: context.generation(),
            serial: NonZeroU64::new(serial).ok_or(DebugError::InvalidReference)?,
        })
    }
    pub(crate) fn wire_id(self) -> String {
        format!(
            "rejection:{}:{}:{}",
            self.engine_epoch, self.context_generation, self.serial
        )
    }
}
/// Rejection state; pending is never qualified as unhandled by a partial checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromiseRejectionState {
    /// Awaiting completion of the whole nextTick/native-job checkpoint.
    Pending,
    /// Unhandled at a fully drained checkpoint.
    Unhandled,
    /// A handler was attached after unhandled reporting; emitted once.
    HandledLater,
}
/// Detached safe metadata; original native values remain confined to the owner.
#[derive(Clone, Debug)]
pub struct PromiseRejectionInfo {
    /// Stable identity within this target/context, never an evaluation capability.
    pub id: PromiseRejectionId,
    /// Checkpoint-qualified state.
    pub state: PromiseRejectionState,
    /// Bounded getter-free description of the original rejection reason.
    pub description: JsString,
    /// Copied original rejection site, when available.
    pub location: Option<DebugLocation>,
    /// Host-projected source position, absent when unavailable.
    pub original: Option<OriginalLocation>,
    /// Scheduling ancestry; eviction remains explicit when reading history.
    pub history: Option<AsyncStackId>,
}
