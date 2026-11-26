//! The handle the target registry ([`crate::debug::DebugTargets`]) keeps for each
//! debugged VM.
//!
//! jetbunny's `VmHandle` wraps its loop handle; here it wraps whatever the host
//! uses to stop a Runtime (its interrupt), with the same two operations the
//! registry calls.

use crate::engine::FailureReport;
use std::sync::Arc;

/// Stops one VM from any thread.
#[derive(Clone)]
pub struct VmHandle {
    /// Identity of the VM's run, distinct for every Runtime the process starts.
    epoch: u64,
    /// Asks the VM to stop cooperatively.
    stop: Arc<dyn Fn() -> Result<(), FailureReport> + Send + Sync>,
}

impl VmHandle {
    /// # Arguments
    /// * `epoch` - identity of the VM's run; two live VMs never share one.
    /// * `stop` - requests a cooperative stop; must not block.
    ///
    /// # Returns
    /// The handle.
    pub fn new(epoch: u64, stop: impl Fn() -> Result<(), FailureReport> + Send + Sync + 'static) -> Self {
        Self { epoch, stop: Arc::new(stop) }
    }

    /// # Returns
    /// The VM's run identity.
    pub(crate) fn runtime_epoch(&self) -> u64 {
        self.epoch
    }

    /// Publishes a cooperative stop.
    ///
    /// # Errors
    /// The host could not signal the VM.
    pub fn request_stop(&self) -> Result<(), FailureReport> {
        (self.stop)()
    }
}

impl std::fmt::Debug for VmHandle {
    /// # Arguments
    /// * `f` - the formatter.
    ///
    /// # Errors
    /// The formatter's.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VmHandle").field("epoch", &self.epoch).finish_non_exhaustive()
    }
}

// jetbunny's test fixtures, from `src/vm/mod.rs` there: the contract adapter of its
// integration tests and the counting allocator, which the copied unit tests use.
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
pub(crate) mod contract_fixture;
#[cfg(test)]
#[path = "../tests/support/allocator.rs"]
pub(crate) mod test_allocator;
