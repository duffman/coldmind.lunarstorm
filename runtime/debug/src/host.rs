//! Owner-local admission and controls sharing the event loop's scheduling state.

use crate::{
    arena::{Arena, Key},
    engine::*,
    ready::ReadyQueue,
    timers::{Clock, Schedule, TimerHeap},
    transport::{Delivery, HostControl},
};
use std::{cell::RefCell, rc::Rc, time::Duration};
/// Services available synchronously inside an owner-thread driver callback.
///
/// Implemented by [`Host`], which shares the existing loop rather than creating
/// a nested scheduler. A rejected local job remains owned by the caller. For
/// cross-thread work, use the loop's [`crate::transport::LoopHandle`] or a bound
/// [`CompletionTicket`]; local jobs and native values stay on the owner thread.
pub trait LocalHost<J> {
    /// Transfers `job` into the local FIFO queue and returns its checked identity.
    ///
    /// `Full` or `Closed` rejection returns the same unexecuted job in [`Rejected`].
    /// Posting during a callback never synchronously dispatches the new job.
    fn post(&self, job: J) -> Result<TaskId, Rejected<J>>;
    /// Transfers `job` into a monotonic one-shot or repeating timer.
    ///
    /// `None` fires once; a repeating `period` must be nonzero. Equal deadlines
    /// retain insertion order. Repeating timers skip missed periods and rearm
    /// only after their callback's checkpoint finishes. Closed/full admission,
    /// invalid periods, deadline overflow, or exhausted timer identities return
    /// the original job without executing it.
    fn timer(
        &self,
        delay: Duration,
        period: Option<Duration>,
        job: J,
    ) -> Result<TaskId, Rejected<J>>;
    /// Cancels a queued task without entering its callback.
    ///
    /// A foreign loop identity returns `WrongOwner`; a stale/missing task returns
    /// `NotFound`. A running one-shot is `TooLate`. A running repeating timer returns
    /// `Running` and will finish its current checkpoint without rearming. Removed
    /// job destructors run outside scheduling-table borrows.
    fn cancel(&self, task: TaskId) -> CancelOutcome;
    /// Sets whether a queued job or timer keeps the loop alive (`true` by default). An unrefed
    /// job still runs when due, but tracked-work checks ignore it. `false` for a foreign,
    /// stale or finished task.
    fn set_ref(&self, task: TaskId, keep_alive: bool) -> bool;
    /// Closes admission and requests cooperative shutdown at the next owner check.
    ///
    /// Does not preempt the active callback or synchronously dispose queued jobs.
    /// Stop has a dedicated transport latch and is not blocked by a full control
    /// queue. Any wake failure is retained for the owner pump to report.
    fn request_stop(&self);
    /// Reserves one dedicated terminal-delivery slot and a positive byte allowance.
    ///
    /// Reserve before starting producer/native work. `Capacity` means operation
    /// slots or aggregate payload allowance are unavailable; `ClosedContext` means
    /// transport admission is closed. Dropping the owner-local reservation rolls
    /// admission back without publishing a completion.
    fn reserve_operation(&self, bytes: usize) -> Result<OperationReservation, FailureReport>;
    /// Commits a reservation after the driver installs its owner-local binding.
    ///
    /// Returns a single-use `Send` ticket that a worker may own. Start producer
    /// work only after this succeeds; workers publish copied payloads, never access
    /// native values. Ticket abandonment and cancellation compete for the same
    /// reserved terminal delivery. Foreign, stale, or closed reservations fail;
    /// the consumed reservation rolls back any still-uncommitted admission.
    fn bind_operation(
        &self,
        reservation: OperationReservation,
    ) -> Result<CompletionTicket, FailureReport>;
    /// Races cancellation against publication through the operation's reserved slot.
    ///
    /// A winning cancellation schedules one terminal payload and returns `Canceled`.
    /// Already-ready publication is `TooLate`, active delivery is `Running`, and
    /// stale/unbound/closed operations are `NotFound`; foreign IDs are `WrongOwner`.
    /// The driver remains responsible for any native cancellation policy.
    fn cancel_operation(&self, id: OperationId) -> CancelOutcome;
}
pub(crate) struct JobRecord<J> {
    pub job: Option<J>,
    pub running: bool,
    pub schedule: Option<Schedule>,
    pub canceled: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Terminal result of waiting for a particular local task turn.
pub enum TurnOutcome {
    /// The selected callback firing and its following checkpoint finished.
    Completed,
    /// The task was removed or its identity was already absent before capture.
    Canceled,
    /// Cooperative shutdown ended the wait before normal completion.
    Stopped,
}
pub(crate) struct LocalState<J> {
    pub epoch: u64,
    pub jobs: Arena<JobRecord<J>>,
    pub ready: ReadyQueue<ReadyItem>,
    pub timers: TimerHeap,
    pub stopping: bool,
    pub capture: Option<(Key, Option<TurnOutcome>)>,
    /// Jobs that do not keep the loop alive. Holds live keys only: every site that removes
    /// a job from `jobs` also calls [`LocalState::remove_job`], so the set never needs a scan.
    pub unrefed: std::collections::HashSet<Key>,
}
impl<J> LocalState<J> {
    /// Live jobs that keep the loop alive.
    ///
    /// Fix: every tracked-work query ran `unrefed.retain` over the whole set to drop keys of
    /// removed jobs. Decision: `unrefed` is kept exact at removal time by
    /// [`Self::remove_job`], so this is two length reads. Tried and dropped: a dirty flag that
    /// pruned lazily; it still needs a scan after every removal of an unref'd job.
    ///
    /// # Returns
    ///
    /// The number of live jobs not marked unref'd.
    pub(crate) fn keepalive_jobs(&self) -> usize {
        self.jobs.len() - self.unrefed.len()
    }
    /// Removes a job record and forgets its unref mark.
    ///
    /// # Arguments
    ///
    /// * `key` - the job to remove
    ///
    /// # Returns
    ///
    /// The removed record, or `None` when the key is stale. Callers drop it outside the
    /// scheduling borrow.
    pub(crate) fn remove_job(&mut self, key: Key) -> Option<JobRecord<J>> {
        let record = self.jobs.remove(key)?;
        if !self.unrefed.is_empty() {
            self.unrefed.remove(&key);
        }
        Some(record)
    }
}
#[derive(Clone, Copy)]
pub(crate) enum ReadyItem {
    Job(Key),
    Delivery(Delivery),
    /// A socket event waiting in the owner reactor's inbox.
    OwnerDelivery(Key),
}
/// Cloneable owner-local view of one existing event loop's admission and controls.
///
/// Clones share task/timer state and transport admission. They neither own a VM
/// nor keep an event loop running after its owner drops it. Jobs rejected after
/// shutdown are returned unchanged. `Rc`-backed state keeps this handle owner-local;
/// use [`crate::transport::LoopHandle`] for cross-thread publication.
pub struct Host<J> {
    pub(crate) state: Rc<RefCell<LocalState<J>>>,
    pub(crate) clock: Rc<dyn Clock>,
    pub(crate) control: Rc<dyn HostControl>,
}
impl<J> Clone for Host<J> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            clock: self.clock.clone(),
            control: self.control.clone(),
        }
    }
}
impl<J> LocalHost<J> for Host<J> {
    fn post(&self, job: J) -> Result<TaskId, Rejected<J>> {
        let stopping = self.control.stopping();
        let mut state = self.state.borrow_mut();
        if state.stopping || stopping {
            return Err(Rejected {
                value: job,
                reason: RejectionReason::Closed,
            });
        }
        let key = state
            .jobs
            .insert(JobRecord {
                job: Some(job),
                running: false,
                schedule: None,
                canceled: false,
            })
            .map_err(|record| Rejected {
                value: record.job.expect("admitted job"),
                reason: RejectionReason::Full,
            })?;
        state.ready.push(key.slot as usize, ReadyItem::Job(key));
        Ok(TaskId {
            epoch: state.epoch,
            slot: key.slot,
            generation: key.generation,
        })
    }
    fn timer(
        &self,
        delay: Duration,
        period: Option<Duration>,
        job: J,
    ) -> Result<TaskId, Rejected<J>> {
        let stopping = self.control.stopping();
        let now = self.clock.now();
        let mut state = self.state.borrow_mut();
        let rejected = if state.stopping || stopping {
            Some(RejectionReason::Closed)
        } else if period == Some(Duration::ZERO) {
            Some(RejectionReason::InvalidPeriod)
        } else {
            None
        };
        if let Some(reason) = rejected {
            return Err(Rejected { value: job, reason });
        }
        let Some(deadline) = now.checked_add(delay) else {
            return Err(Rejected {
                value: job,
                reason: RejectionReason::DeadlineOverflow,
            });
        };
        let key = state
            .jobs
            .insert(JobRecord {
                job: Some(job),
                running: false,
                schedule: Some(Schedule { deadline, period }),
                canceled: false,
            })
            .map_err(|record| Rejected {
                value: record.job.expect("new timer"),
                reason: RejectionReason::Full,
            })?;
        if state.timers.insert(key, deadline).is_err() {
            let record = state.jobs.remove(key).expect("new timer");
            return Err(Rejected {
                value: record.job.expect("new timer"),
                reason: RejectionReason::IdentityExhausted,
            });
        }
        Ok(TaskId {
            epoch: state.epoch,
            slot: key.slot,
            generation: key.generation,
        })
    }
    fn cancel(&self, task: TaskId) -> CancelOutcome {
        let removed = {
            let mut state = self.state.borrow_mut();
            if task.epoch != state.epoch {
                return CancelOutcome::WrongOwner;
            }
            let key = Key {
                slot: task.slot,
                generation: task.generation,
            };
            match state.jobs.get_mut(key) {
                None => return CancelOutcome::NotFound,
                Some(record) if record.running => {
                    return if record
                        .schedule
                        .is_some_and(|schedule| schedule.period.is_some())
                    {
                        record.canceled = true;
                        CancelOutcome::Running
                    } else {
                        CancelOutcome::TooLate
                    };
                }
                _ => {}
            }
            state.ready.remove(key.slot as usize);
            state.timers.remove(key);
            if let Some((target, outcome)) = &mut state.capture
                && *target == key
            {
                *outcome = Some(TurnOutcome::Canceled);
            }
            // Fix: unref marks were pruned by scanning. Decision: remove the mark here.
            state.remove_job(key)
        };
        // User-owned destructors never run under the scheduling borrow.
        drop(removed);
        CancelOutcome::Canceled
    }
    fn set_ref(&self, task: TaskId, keep_alive: bool) -> bool {
        let mut state = self.state.borrow_mut();
        let key = Key {
            slot: task.slot,
            generation: task.generation,
        };
        if task.epoch != state.epoch || state.jobs.get(key).is_none() {
            return false;
        }
        if keep_alive {
            state.unrefed.remove(&key);
        } else {
            state.unrefed.insert(key);
        }
        true
    }
    fn request_stop(&self) {
        self.state.borrow_mut().stopping = true;
        let _ = self.control.stop();
    }
    fn reserve_operation(&self, bytes: usize) -> Result<OperationReservation, FailureReport> {
        self.control.reserve(bytes)
    }
    fn bind_operation(
        &self,
        reservation: OperationReservation,
    ) -> Result<CompletionTicket, FailureReport> {
        self.control.bind(reservation)
    }
    fn cancel_operation(&self, id: OperationId) -> CancelOutcome {
        self.control.cancel(id)
    }
}
