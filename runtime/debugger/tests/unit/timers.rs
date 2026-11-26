//! Unit tests of the timer heap: sequence exhaustion, index repair after removal,
//! duplicate and stale keys, and interval re-arming at the sequence limit.

use super::*;
impl TimerHeap {
    /// Simulates the final checked sequence without performing 2^64 admissions.
    pub(crate) fn exhaust_sequence_for_test(&mut self) {
        self.sequence = u64::MAX;
    }
}
#[test]
fn indexes_are_repaired_after_every_swap_and_removal() {
    let mut heap = TimerHeap::new(128).unwrap();
    let now = Instant::now();
    for i in 0..128 {
        heap.insert(
            Key {
                slot: i,
                generation: 1,
            },
            now + Duration::from_nanos(u64::from((i * 73) % 128)),
        )
        .unwrap();
    }
    for i in (0..128).step_by(3) {
        assert!(heap.remove(Key {
            slot: i,
            generation: 1
        }));
        for (position, entry) in heap.heap.iter().enumerate() {
            assert_eq!(heap.positions[entry.key.slot as usize], Some(position));
            if position > 0 {
                assert!(!heap.before(position, (position - 1) / 2));
            }
        }
    }
    let mut last = now;
    while let Some(entry) = heap.heap.first().copied() {
        assert!(entry.deadline >= last);
        last = entry.deadline;
        assert!(heap.pop_due(now + Duration::from_secs(1)).is_some());
    }
    assert!(heap.positions.iter().all(Option::is_none));
}
#[test]
fn insertion_sequence_never_wraps() {
    let mut heap = TimerHeap::new(1).unwrap();
    heap.sequence = u64::MAX;
    assert!(
        heap.insert(
            Key {
                slot: 0,
                generation: 1
            },
            Instant::now()
        )
        .is_err()
    );
    assert!(heap.heap.is_empty());
}
#[test]
fn duplicate_and_stale_keys_do_not_mutate_an_existing_timer() {
    assert!(TimerHeap::new(usize::MAX).is_err());
    let mut heap = TimerHeap::new(1).unwrap();
    let now = Instant::now();
    let key = Key {
        slot: 0,
        generation: 3,
    };
    heap.insert(key, now).unwrap();
    assert!(heap.insert(key, now + Duration::from_secs(1)).is_err());
    assert!(!heap.remove(Key {
        generation: 2,
        ..key
    }));
    assert_eq!(heap.next(), Some(now));
    assert_eq!(heap.pop_due(now), Some(key));
    assert_eq!(heap.next(), None);
}
#[test]
fn replacement_sifts_up_when_it_precedes_its_new_parent() {
    let mut heap = TimerHeap::new(6).unwrap();
    let now = Instant::now();
    for (slot, seconds) in [0, 10, 1, 11, 12, 2].into_iter().enumerate() {
        heap.insert(
            Key {
                slot: slot as u32,
                generation: 1,
            },
            now + Duration::from_secs(seconds),
        )
        .unwrap();
    }
    assert!(heap.remove(Key {
        slot: 3,
        generation: 1
    }));
    let mut order = Vec::new();
    while let Some(key) = heap.pop_due(now + Duration::from_secs(20)) {
        order.push(key.slot);
    }
    assert_eq!(order, [0, 2, 5, 1, 4]);
    assert!(heap.positions.iter().all(Option::is_none));
}
#[test]
fn invalid_and_unrepresentable_periods_never_wrap_a_deadline() {
    let now = Instant::now();
    assert_eq!(
        next_period(
            Schedule {
                deadline: now,
                period: None
            },
            now
        ),
        None
    );
    assert_eq!(
        next_period(
            Schedule {
                deadline: now,
                period: Some(Duration::ZERO)
            },
            now
        ),
        None
    );
    assert_eq!(
        next_period(
            Schedule {
                deadline: now,
                period: Some(Duration::MAX)
            },
            now
        ),
        None
    );
    // Real representable Instants can span more than a signed-second range.
    // The next interval step must still fit Duration's u64-second payload.
    let span = Duration::from_secs(i64::MAX as u64 / 2 + 1_000_000);
    if let (Some(start), Some(end)) = (now.checked_sub(span), now.checked_add(span)) {
        assert_eq!(
            next_period(
                Schedule {
                    deadline: start,
                    period: Some(Duration::from_secs(u64::MAX / 2 + 1))
                },
                end
            ),
            None
        );
    }
}
#[test]
fn either_timer_heap_allocation_can_fail_and_leave_the_next_construction_usable() {
    for allocation in 0..2 {
        let (result, fired) = crate::vm::test_allocator::fail_nth(allocation, || TimerHeap::new(2));
        assert!(fired && result.is_err());
        let mut healthy = TimerHeap::new(2).unwrap();
        let now = Instant::now();
        let key = Key {
            slot: 0,
            generation: 1,
        };
        healthy.insert(key, now).unwrap();
        assert_eq!(healthy.pop_due(now), Some(key));
    }
}

// These two cases use an actual Mio-backed owner; heap-only tests above remain
// available to Miri without native poll resources.
#[cfg(not(miri))]
mod native_loop {
    use crate::{engine::*, scheduler::*};
    use std::{cell::Cell, num::NonZeroUsize, rc::Rc, time::Duration};
    struct Job {
        calls: Rc<Cell<usize>>,
        drops: Rc<Cell<usize>>,
    }
    impl Drop for Job {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }
    struct Driver;
    impl LoopDriver for Driver {
        type Job = Job;
        type Message = ();
        fn execute_job(
            &mut self,
            job: &mut Job,
            _: &dyn LocalHost<Job>,
        ) -> Result<(), FailureReport> {
            job.calls.set(job.calls.get() + 1);
            Ok(())
        }
        fn deliver_message(&mut self, _: (), _: &dyn LocalHost<Job>) -> Result<(), FailureReport> {
            Ok(())
        }
        fn deliver_completion(
            &mut self,
            _: OperationId,
            _: CompletionPayload,
            _: &dyn LocalHost<Job>,
        ) -> Result<(), FailureReport> {
            Ok(())
        }
        fn checkpoint(
            &mut self,
            _: NonZeroUsize,
            _: &dyn LocalHost<Job>,
        ) -> Result<CheckpointReport, FailureReport> {
            Ok(CheckpointReport::empty())
        }
    }
    fn runtime() -> EventLoop<Driver> {
        EventLoop::new(
            Driver,
            SchedulerConfig {
                max_jobs: 1,
                ..Default::default()
            },
            None,
        )
        .unwrap()
    }

    #[test]
    fn timer_sequence_exhaustion_returns_the_owned_job_and_restores_admission() {
        let mut runtime = runtime();
        let host = runtime.host();
        host.state.borrow_mut().timers.sequence = u64::MAX;
        let calls = Rc::new(Cell::new(0));
        let drops = Rc::new(Cell::new(0));
        let rejected = host
            .timer(
                Duration::ZERO,
                None,
                Job {
                    calls: calls.clone(),
                    drops: drops.clone(),
                },
            )
            .err()
            .unwrap();
        assert_eq!(rejected.reason, RejectionReason::IdentityExhausted);
        assert_eq!((calls.get(), drops.get()), (0, 0));
        assert_eq!(runtime.stats().live_jobs, 0);
        let id = host.post(rejected.value).ok().unwrap();
        assert_eq!(runtime.run_until(id).unwrap(), TurnOutcome::Completed);
        assert_eq!((calls.get(), drops.get()), (1, 1));
        assert!(runtime.last_failure().is_none());
    }

    #[test]
    fn interval_rearm_sequence_exhaustion_faults_after_one_call_and_disposes_once() {
        let mut runtime = runtime();
        let host = runtime.host();
        let calls = Rc::new(Cell::new(0));
        let drops = Rc::new(Cell::new(0));
        host.timer(
            Duration::ZERO,
            Some(Duration::from_secs(1)),
            Job {
                calls: calls.clone(),
                drops: drops.clone(),
            },
        )
        .ok()
        .unwrap();
        host.state.borrow_mut().timers.sequence = u64::MAX;
        let error = runtime.tick(false).unwrap_err();
        assert_eq!(error.code, FailureCode::Capacity);
        assert_eq!((calls.get(), drops.get()), (1, 1));
        assert_eq!(runtime.stats().live_jobs, 0);
        assert!(runtime.tick(false).unwrap().stopped);
        drop(runtime);
        assert_eq!((calls.get(), drops.get()), (1, 1));
    }
}
