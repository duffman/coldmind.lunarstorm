use coldmind_runtime::engine::*;
use std::{rc::Rc, time::Duration};
pub struct NoLoop;
impl HostServices for NoLoop {
    #[allow(
        clippy::result_large_err,
        reason = "The host contract preserves the rejected owned callback inline."
    )]
    fn post(&self, job: CallbackJob) -> Result<TaskId, Rejected<CallbackJob>> {
        Err(Rejected {
            value: job,
            reason: RejectionReason::Unsupported,
        })
    }
    #[allow(
        clippy::result_large_err,
        reason = "The host contract preserves the rejected owned callback inline."
    )]
    fn timer(
        &self,
        _: Duration,
        _: Option<Duration>,
        job: CallbackJob,
    ) -> Result<TaskId, Rejected<CallbackJob>> {
        self.post(job)
    }
    fn cancel(&self, _: TaskId) -> CancelOutcome {
        CancelOutcome::NotFound
    }
    fn request_stop(&self) -> Result<(), FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "no loop in ownership fixture",
        ))
    }
    fn reserve_operation(&self, _: usize) -> Result<OperationReservation, FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "no async bridge in ownership fixture",
        ))
    }
    fn bind_deferred(
        &self,
        _: OperationReservation,
        _: RootedDeferred,
        _: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "no async bridge in ownership fixture",
        ))
    }
    fn bind_callback(
        &self,
        _: OperationReservation,
        _: RootedValue,
        _: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "no async bridge in ownership fixture",
        ))
    }
    fn cancel_operation(&self, _: OperationId) -> CancelOutcome {
        CancelOutcome::NotFound
    }
}
