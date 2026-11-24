//! Explicit debugger evaluator policy and detached terminal outcomes.
use super::{DebugError, DebugValue};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use std::time::{Duration, Instant};

/// Execution policy for a debugger expression, without awaiting returned Promises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvaluationOptions {
    /// Maximum evaluator runtime in milliseconds; zero is invalid.
    pub deadline_ms: u64,
    /// Whether the caller permits application side effects.
    pub allow_side_effects: bool,
}
impl Default for EvaluationOptions {
    fn default() -> Self {
        Self {
            deadline_ms: 2_000,
            allow_side_effects: true,
        }
    }
}

/// Thread-safe metadata for exactly one evaluator operation.
///
/// Clones address the same operation; create a fresh control for every later
/// evaluation. This token owns no native state and neither interrupts execution
/// nor rolls back effects. The native owner must act on the status it observes.
#[derive(Clone, Debug)]
pub struct EvaluationControl {
    inner: Arc<EvaluationControlInner>,
}

#[derive(Debug)]
struct EvaluationControlInner {
    deadline: Instant,
    // Immutable metadata is shared through Arc; this atomic publishes no other data.
    state: AtomicU8,
}

impl EvaluationControl {
    const ACTIVE: u8 = 0;
    const CANCEL_REQUESTED: u8 = 1;
    const REVOKED: u8 = 2;
    const RETURNED: u8 = 3;
    const THREW: u8 = 4;
    const CANCELLED: u8 = 5;
    const DEADLINE_EXCEEDED: u8 = 6;

    /// Start an operation now, rejecting zero or unrepresentable budgets.
    pub fn new(options: EvaluationOptions) -> Result<Self, DebugError> {
        Self::new_at(options, Instant::now())
    }

    /// Start at an explicit monotonic instant, also useful for deterministic polls.
    /// Budgets must fit in a nonzero `u64` number of nanoseconds and the platform's
    /// `Instant` range; invalid arithmetic returns `InvalidRequest`.
    pub fn new_at(options: EvaluationOptions, start: Instant) -> Result<Self, DebugError> {
        let nanos = options
            .deadline_ms
            .checked_mul(1_000_000)
            .filter(|nanos| *nanos != 0)
            .ok_or(DebugError::InvalidRequest)?;
        let deadline = start
            .checked_add(Duration::from_nanos(nanos))
            .ok_or(DebugError::InvalidRequest)?;
        Ok(Self {
            inner: Arc::new(EvaluationControlInner {
                deadline,
                state: AtomicU8::new(Self::ACTIVE),
            }),
        })
    }

    /// Request evaluator-only cancellation. Returns true only for the first
    /// accepted request; finished or revoked operations reject late requests.
    pub fn cancel(&self) -> bool {
        self.inner
            .state
            .compare_exchange(
                Self::ACTIVE,
                Self::CANCEL_REQUESTED,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    /// Decide cancellation/deadline status without touching native execution.
    /// The deadline is inclusive. A pending cancellation wins if both causes
    /// are present at the first poll; concurrent decisions use the first atomic
    /// transition. Every later poll returns that same terminal result, including
    /// polls with an earlier timestamp. Revoked operations return `None`.
    pub fn poll_at(&self, now: Instant) -> Option<EvaluationStatus> {
        loop {
            let state = self.inner.state.load(Ordering::Relaxed);
            let next = match state {
                Self::ACTIVE if now >= self.inner.deadline => Self::DEADLINE_EXCEEDED,
                Self::CANCEL_REQUESTED => Self::CANCELLED,
                _ => return Self::status(state),
            };
            if self
                .inner
                .state
                .compare_exchange(state, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return Self::status(next);
            }
        }
    }

    /// Seal the owner's terminal result, honoring pending cancellation/deadline
    /// first. Returns the winning status, or `None` if revoked. Late cloned
    /// tokens cannot change the result or affect another operation.
    pub fn finish(&self, status: EvaluationStatus) -> Option<EvaluationStatus> {
        let terminal = match status {
            EvaluationStatus::Returned => Self::RETURNED,
            EvaluationStatus::Threw => Self::THREW,
            EvaluationStatus::Cancelled => Self::CANCELLED,
            EvaluationStatus::DeadlineExceeded => Self::DEADLINE_EXCEEDED,
        };
        loop {
            if let Some(status) = self.poll_at(Instant::now()) {
                return Some(status);
            }
            match self.inner.state.compare_exchange(
                Self::ACTIVE,
                terminal,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Some(status),
                Err(Self::REVOKED) => return None,
                Err(_) => continue,
            }
        }
    }

    /// Disarm an undecided operation and all its clones, including an unobserved
    /// cancellation request. Already terminal outcomes remain stable. Revocation
    /// is metadata cleanup; it does not unwind an evaluator or undo side effects.
    pub fn revoke(&self) {
        let _ = self
            .inner
            .state
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |state| {
                matches!(state, Self::ACTIVE | Self::CANCEL_REQUESTED).then_some(Self::REVOKED)
            });
    }

    fn status(state: u8) -> Option<EvaluationStatus> {
        match state {
            Self::RETURNED => Some(EvaluationStatus::Returned),
            Self::THREW => Some(EvaluationStatus::Threw),
            Self::CANCELLED => Some(EvaluationStatus::Cancelled),
            Self::DEADLINE_EXCEEDED => Some(EvaluationStatus::DeadlineExceeded),
            _ => None,
        }
    }
}
/// Terminal evaluator state, separate from application stop or exception handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvaluationStatus {
    /// The expression returned a value.
    Returned,
    /// The expression threw normally.
    Threw,
    /// An evaluator-only cancellation unwound the expression.
    Cancelled,
    /// An evaluator-only deadline unwound the expression.
    DeadlineExceeded,
}
/// Detached evaluator result; cancellation never promises rollback.
#[derive(Clone, Debug)]
pub struct EvaluationOutcome {
    /// Exactly one terminal evaluator status.
    pub status: EvaluationStatus,
    /// Returned or thrown value when available.
    pub value: Option<DebugValue>,
    /// Earlier writes or queued host work may remain after this outcome.
    pub effects_may_remain: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn options(deadline_ms: u64) -> EvaluationOptions {
        EvaluationOptions {
            deadline_ms,
            ..EvaluationOptions::default()
        }
    }

    #[test]
    fn deadline_is_inclusive_and_terminal_polls_are_stable() {
        let start = Instant::now();
        let control = EvaluationControl::new_at(options(10), start).unwrap();
        assert_eq!(control.poll_at(start + Duration::from_millis(9)), None);
        assert_eq!(
            control.poll_at(start + Duration::from_millis(10)),
            Some(EvaluationStatus::DeadlineExceeded)
        );
        assert!(!control.cancel());
        assert_eq!(
            control.poll_at(start),
            Some(EvaluationStatus::DeadlineExceeded)
        );
        assert_eq!(
            control.finish(EvaluationStatus::Returned),
            Some(EvaluationStatus::DeadlineExceeded)
        );
    }

    #[test]
    fn zero_and_overflowing_budgets_are_invalid() {
        for budget in [0, u64::MAX / 1_000_000 + 1, u64::MAX] {
            assert!(matches!(
                EvaluationControl::new(options(budget)),
                Err(super::super::DebugError::InvalidRequest)
            ));
        }
        assert!(EvaluationControl::new(options(1)).is_ok());
    }

    #[test]
    fn cross_thread_cancel_is_latched_before_pending_deadline() {
        let start = Instant::now();
        let control = EvaluationControl::new_at(options(10), start).unwrap();
        let token = control.clone();
        assert!(std::thread::spawn(move || token.cancel()).join().unwrap());
        assert!(!control.cancel());
        assert_eq!(
            control.poll_at(start + Duration::from_millis(10)),
            Some(EvaluationStatus::Cancelled)
        );
        assert_eq!(control.poll_at(start), Some(EvaluationStatus::Cancelled));
    }

    #[test]
    fn finish_disarms_late_tokens_and_preserves_first_result() {
        for status in [EvaluationStatus::Returned, EvaluationStatus::Threw] {
            let control = EvaluationControl::new(options(2_000)).unwrap();
            let late_token = control.clone();
            assert_eq!(control.finish(status), Some(status));
            assert!(!late_token.cancel());
            assert_eq!(control.finish(EvaluationStatus::Cancelled), Some(status));
            control.revoke();
            assert_eq!(late_token.poll_at(Instant::now()), Some(status));
        }
    }

    #[test]
    fn finish_honors_a_pending_cancel() {
        let control = EvaluationControl::new(options(2_000)).unwrap();
        assert!(control.cancel());
        assert_eq!(
            control.finish(EvaluationStatus::Returned),
            Some(EvaluationStatus::Cancelled)
        );
    }

    #[test]
    fn finish_honors_an_elapsed_deadline() {
        let start = Instant::now() - Duration::from_secs(1);
        let control = EvaluationControl::new_at(options(1), start).unwrap();
        assert_eq!(
            control.finish(EvaluationStatus::Threw),
            Some(EvaluationStatus::DeadlineExceeded)
        );
        control.revoke();
        assert_eq!(
            control.poll_at(start),
            Some(EvaluationStatus::DeadlineExceeded)
        );
    }

    #[test]
    fn revocation_disarms_pending_and_late_cancel_tokens() {
        for pending_cancel in [false, true] {
            let start = Instant::now();
            let control = EvaluationControl::new_at(options(10), start).unwrap();
            let late_token = control.clone();
            if pending_cancel {
                assert!(late_token.cancel());
            }
            control.revoke();
            control.revoke();
            assert!(!late_token.cancel());
            assert_eq!(late_token.poll_at(start + Duration::from_secs(1)), None);
            assert_eq!(control.finish(EvaluationStatus::Returned), None);
        }
    }

    #[test]
    fn separate_evaluations_have_independent_tokens() {
        let start = Instant::now();
        let first = EvaluationControl::new_at(options(10), start).unwrap();
        let second = EvaluationControl::new_at(options(60_000), start).unwrap();
        assert!(first.cancel());
        assert_eq!(first.poll_at(start), Some(EvaluationStatus::Cancelled));
        assert_eq!(second.poll_at(start + Duration::from_millis(10)), None);
        assert_eq!(
            second.finish(EvaluationStatus::Threw),
            Some(EvaluationStatus::Threw)
        );
        assert!(!first.cancel());
        assert_eq!(second.poll_at(start), Some(EvaluationStatus::Threw));
    }
}
