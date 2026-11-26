//! Capacity and cooperative scheduling budgets shared by a loop and its transport.

use crate::engine::{FailureCode, FailureReport};
use std::num::NonZeroUsize;
/// Admission limits and per-slice budgets for an owner-thread event loop.
///
/// Local jobs, ordinary cross-thread messages, and reserved operation completions
/// use separate bounded storage. Counts must be positive and fit in `u32`; byte
/// allowances must be positive. Construction reserves storage from these limits.
#[derive(Clone, Debug)]
pub struct SchedulerConfig {
    /// Maximum live local jobs, including timers and jobs awaiting their checkpoint.
    pub max_jobs: usize,
    /// Maximum admitted ordinary messages, including messages already imported as ready.
    pub max_messages: usize,
    /// Maximum operation reservations, including pending and terminal deliveries.
    pub max_operations: usize,
    /// Capacity of the cross-thread task-cancellation control queue.
    ///
    /// Stop uses a separate latch and does not require a free queue entry.
    pub max_controls: usize,
    /// Aggregate payload bytes that active operation reservations may commit.
    pub reserved_payload_bytes: usize,
    /// Per-operation payload policy available to a driver or VM bridge.
    ///
    /// The generic reservation API accounts for explicitly requested bytes against
    /// `reserved_payload_bytes`; it does not independently enforce this policy field.
    pub operation_payload_bytes: usize,
    /// Maximum ordinary turns, transport imports and owner-reactor socket events per slice.
    ///
    /// The owner reactor gets one nonblocking turn per slice, including while a JS
    /// checkpoint is pending, so sockets advance between batches of JS work.
    pub task_batch: usize,
    /// Cooperative microtask budget passed to the driver for each checkpoint.
    ///
    /// A driver may report `Pending`; this hint does not preempt running native code.
    pub microtask_hint: NonZeroUsize,
}
impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_jobs: 4096,
            max_messages: 1024,
            max_operations: 1024,
            max_controls: 256,
            reserved_payload_bytes: 64 * 1024 * 1024,
            operation_payload_bytes: 1024 * 1024,
            task_batch: 64,
            microtask_hint: NonZeroUsize::new(1024).unwrap(),
        }
    }
}
impl SchedulerConfig {
    /// Checks representable, positive capacities before loop allocation.
    ///
    /// Returns [`FailureCode::Capacity`] for invalid counts, zero byte allowances,
    /// or overflow when summing count budgets. Success does not guarantee that the
    /// requested storage or operating-system poll resources can be allocated.
    pub fn validate(&self) -> Result<(), FailureReport> {
        let counts = [
            self.max_jobs,
            self.max_messages,
            self.max_operations,
            self.max_controls,
            self.task_batch,
        ];
        if counts.iter().any(|&n| n == 0 || n > u32::MAX as usize)
            || self.operation_payload_bytes == 0
            || self.reserved_payload_bytes == 0
            || counts
                .iter()
                .try_fold(0usize, |a, &b| a.checked_add(b))
                .is_none()
        {
            return Err(FailureReport::fault(
                FailureCode::Capacity,
                "invalid scheduler capacities",
            ));
        }
        Ok(())
    }
}
