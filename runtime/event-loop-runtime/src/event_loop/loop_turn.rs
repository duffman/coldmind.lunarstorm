//! [`Turn`]: the loop's state for one scope entry, and one step of the loop:
//! readiness polling, settling ticks and promise jobs, memory checks and the
//! fatal and uncaught error paths.

use crate::runtime_public_types::{ev, State, Task, Wait};
use crate::ipc_module::Ipc;
use crate::worker_threads_module::Workers;
use crate::host_services_trait::{Host, Log};
use crate::reactor_io_state::{Deferred, Fault, Shared};
use crate::module_loader_callbacks::Rejections;
use crossbeam_channel::Receiver;
use lunarstorm_engine_contract::{
    Instance, Scope, Thrown, Vm,
};
use mio::{Events, Poll};
use std::io::{self};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use super::*;

/// The loop's state for one scope entry: the context handle, the hooks as scoped
/// values, and borrows of what the Runtime owns.
pub(crate) struct Turn<'a, 's, V: Vm, H: Host> {
    /// The context handle.
    pub(crate) scope: V::Scope<'s>,
    /// The engine instance, for the debugger hooks.
    pub(crate) instance: &'a V::Instance,
    /// JS entry point for every event.
    pub(crate) dispatch: V::Value<'s>,
    /// Runs the `process.nextTick` queue.
    pub(crate) ticks: V::Value<'s>,
    /// Formats a thrown value for the uncaught-error record.
    pub(crate) describe: V::Value<'s>,
    /// State shared with the native functions.
    pub(crate) shared: &'a Shared<H>,
    /// Unhandled promise rejections, filled by the VM callbacks.
    pub(crate) rejections: &'a Rejections<V>,
    /// Read buffer shared by every socket.
    pub(crate) scratch: &'a mut [u8],
    /// Tasks posted from other threads.
    pub(crate) tasks: &'a Receiver<Task<V>>,
    /// The `lunarstorm:ipc` channel and receiver.
    pub(crate) ipc: &'a Ipc<V>,
    /// The `lunarstorm:worker` children.
    pub(crate) workers: &'a Workers<V>,
    /// Set by a poster when the task queue became non-empty.
    pub(crate) pending: &'a AtomicBool,
    /// Kernel readiness polls made so far.
    pub(crate) polls: &'a mut u64,
    /// When readiness was last polled while work was queued; see [`Runtime`].
    pub(crate) polled: &'a mut Option<Instant>,
}

impl<'s, V: Vm, H: Host> Turn<'_, 's, V, H> {
    /// Runs one step of the loop.
    ///
    /// # Arguments
    /// * `reactor` - the readiness source.
    /// * `events` - buffer for this step's readiness events.
    /// * `wait` - whether the step may sleep.
    ///
    /// # Returns
    /// [`State::Idle`] when there was nothing to wait for; otherwise
    /// [`State::Running`] after the step.
    pub(crate) fn step(&mut self, reactor: &mut Poll, events: &mut Events, wait: Wait) -> State {
        // A worker that called `close()` or was terminated ends at its next step,
        // whatever is still armed or open.
        if self.shared.stop.get() || self.shared.terminated.load(Ordering::Relaxed) {
            return State::Idle;
        }

        // Phase 0: debugger requests (pause, resume, breakpoints, inspection). A
        // client that asks the program to stop ends the loop.
        match self.instance.debug_service() {
            Ok(false) => {}
            Ok(true) => return State::Idle,
            Err(e) => self.die(&format!("debugger: {e}")),
        }

        // Phase 1: timers.
        self.run_timers();

        // Phase 2: poll. Sleep exactly as long as the next timer allows.
        let host = &self.shared.host;
        let (timeout, queued) = {
            let mut io = self.shared.io.borrow_mut();
            io.flush_stdout(host);
            io.date_fresh = false;
            if self.shared.immediate.get() || self.pending.load(Ordering::Acquire) {
                (Some(Duration::ZERO), true)
            } else if let Some(deadline) = io.next_deadline() {
                (Some(deadline.saturating_duration_since(host.monotonic())), false)
            } else if io.alive() || self.shared.held.get() || self.shared.refs.get() > 0 {
                (None, false)
            } else {
                return State::Idle;
            }
        };
        let timeout = if wait == Wait::NoBlock { Some(Duration::ZERO) } else { timeout };
        // Fix: with an immediate or a posted task queued, every step used to make a
        // zero-timeout kernel poll, so a chain of `setImmediate` callbacks paid one
        // syscall each. Such a step now polls only when `POLL_GAP` has passed.
        // Dropped alternative: polling every Nth step, which bounds the number of
        // callbacks I/O waits behind but not the time.
        if self.poll_due(queued) {
            *self.polls += 1;
            match reactor.poll(events, timeout) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => self.die(&format!("poll failed: {e}")),
            }

            // Phase 3: I/O. Only after a poll: the buffer otherwise still holds the
            // events of an earlier step.
            for event in events.iter() {
                self.on_event(event);
            }
        }

        // Phase 4: tasks posted from other threads.
        if self.pending.load(Ordering::Acquire) {
            self.drain_tasks();
        }

        // Phase 5: immediates.
        if self.shared.immediate.replace(false) {
            self.run_immediates();
        }
        State::Running
    }

    /// Decides whether this step asks the kernel for readiness.
    ///
    /// Decision: the step that has nothing queued always polls and reads no clock,
    /// so the I/O path costs what it did before. Only a step with queued work reads
    /// the clock, and the gap is measured from the first such step after a poll.
    ///
    /// # Arguments
    /// * `queued` - whether an immediate or a posted task is waiting, which makes
    ///   the poll a readiness check rather than a wait.
    ///
    /// # Returns
    /// `true` to poll now.
    #[inline]
    pub(crate) fn poll_due(&mut self, queued: bool) -> bool {
        if !queued {
            *self.polled = None;
            return true;
        }
        let now = self.shared.host.monotonic();
        match *self.polled {
            Some(last) if now.saturating_duration_since(last) < POLL_GAP => false,
            Some(_) => {
                *self.polled = Some(now);
                true
            }
            // The previous step polled (or none has run yet): readiness is fresh.
            None => {
                *self.polled = Some(now);
                false
            }
        }
    }

    // ------------------------------------------------------------ JS entries

    /// # Arguments
    /// * `value` - an event code or slab key.
    ///
    /// # Returns
    /// It as a JS integer.
    #[inline]
    pub(crate) fn int(&self, value: i32) -> V::Value<'s> {
        self.scope.int(value)
    }

    /// # Arguments
    /// * `text` - text for an event argument.
    ///
    /// # Returns
    /// It as a JS string. Allocation failure is fatal.
    pub(crate) fn text(&self, text: &str) -> V::Value<'s> {
        match self.scope.string(text) {
            Ok(value) => value,
            Err(Thrown) => self.fatal(),
        }
    }

    /// Calls the JS `dispatch` function with one event and settles the engine.
    ///
    /// # Arguments
    /// * `args` - event code followed by the event's arguments.
    pub(crate) fn fire(&mut self, args: &[V::Value<'s>]) {
        self.entry();
        self.beat();
        if self.scope.call(&self.dispatch, args).is_err() {
            self.raise();
        }
        self.settle();
        self.beat();
    }

    /// Moves the pulse: called once before and once after each JS entry, so the
    /// counter is odd exactly while a script runs. Compiled out unless the host
    /// type asks for the pulse.
    #[inline(always)]
    pub(crate) fn beat(&self) {
        if H::PULSE {
            let pulse = &self.shared.pulse;
            // One writer (this thread), so a plain load and store is enough.
            pulse.store(pulse.load(Ordering::Relaxed).wrapping_add(1), Ordering::Relaxed);
        }
    }

    /// Brings the engine to rest after a JS call, with Node's ordering: all
    /// `nextTick` callbacks, then all promise jobs, repeated until both are empty.
    /// Then reports any promise rejection that is still unhandled.
    pub(crate) fn settle(&mut self) {
        // Fix: a script that caught an out-of-memory error kept running on an
        // exhausted heap. The engine's sticky mark is checked before and after the
        // queued jobs run, so a caught failure ends the program like an uncaught one.
        self.check_memory();
        loop {
            if self.shared.tick.replace(false) && self.scope.call(&self.ticks, &[]).is_err() {
                self.raise();
            }
            while self.scope.run_job() {}
            if !self.shared.tick.get() {
                break;
            }
        }
        self.check_memory();
        // The job queue is empty: the debugger qualifies unhandled rejections now,
        // before they are reported below.
        if let Err(e) = self.instance.debug_checkpoint() {
            self.die(&format!("debugger: {e}"));
        }
        // Each unhandled rejection is reported: under the default host the first
        // one exits, and a host that survives them sees every one.
        while !self.rejections.borrow().is_empty() {
            let (_, reason) = self.rejections.borrow_mut().remove(0);
            let text = self.describe_value(&self.scope.local(&reason));
            self.uncaught(true, &text);
        }
    }

    /// Before a JS entry: lets an attached debugger hold it (break on start).
    pub(crate) fn entry(&self) {
        if let Err(e) = self.instance.debug_entry() {
            self.die(&format!("debugger: {e}"));
        }
    }

    /// Exits with status 1 when an allocation failed since the last check, whether
    /// or not the script caught the error ([`Scope::take_out_of_memory`]).
    pub(crate) fn check_memory(&self) {
        if self.scope.take_out_of_memory() {
            self.die("the engine ran out of memory");
        }
    }

    /// Formats a JS value with the glue's inspector.
    ///
    /// # Arguments
    /// * `value` - a thrown value or rejection reason.
    ///
    /// # Returns
    /// Its text: name, message and stack for errors.
    pub(crate) fn describe_value(&self, value: &V::Value<'s>) -> String {
        let described = self.scope.call(&self.describe, std::slice::from_ref(value));
        match described.and_then(|text| self.scope.with_str(&text, str::to_owned)) {
            Ok(text) => text,
            Err(Thrown) => {
                let _ = self.scope.exception();
                String::from("<unprintable>")
            }
        }
    }

    /// Reports the pending exception as an uncaught script error. Whether the
    /// program goes on is the host's choice ([`Host::uncaught`]).
    pub(crate) fn raise(&self) {
        let thrown = self.scope.exception();
        let text = self.describe_value(&thrown);
        self.uncaught(false, &text);
    }

    /// Reports the pending exception and exits with status 1. For failures the
    /// loop itself cannot continue from (the engine could not allocate an event's
    /// argument), whatever the host's policy for script errors is.
    pub(crate) fn fatal(&self) -> ! {
        let thrown = self.scope.exception();
        let text = self.describe_value(&thrown);
        self.die(&text)
    }

    /// Flushes stdout and hands an uncaught script error to the host.
    ///
    /// # Arguments
    /// * `in_promise` - whether it is a rejected promise rather than a thrown
    ///   exception.
    /// * `description` - the error as `console` would show it.
    pub(crate) fn uncaught(&self, in_promise: bool, description: &str) {
        // The interrupt that ends a terminated worker surfaces as an uncaught error;
        // that is the requested outcome, not a script error to report.
        if self.shared.terminated.load(Ordering::Relaxed) {
            return;
        }
        let host = &self.shared.host;
        self.shared.io.borrow_mut().flush_stdout(host);
        host.uncaught(in_promise, description);
    }

    /// Flushes stdout, logs an error the loop cannot recover from and exits with
    /// status 1.
    ///
    /// # Arguments
    /// * `message` - what failed.
    pub(crate) fn die(&self, message: &str) -> ! {
        let host = &self.shared.host;
        self.shared.io.borrow_mut().flush_stdout(host);
        host.log(Log::Error(message));
        host.exit(1)
    }

    /// Delivers the events that ops queued while JS was running.
    pub(crate) fn drain_deferred(&mut self) {
        loop {
            let next = self.shared.io.borrow_mut().take_deferred();
            match next {
                None => return,
                Some(Deferred::Close(key, fault)) => self.fire_close(key, fault),
                Some(Deferred::Drain(key)) => self.fire(&[self.int(ev::DRAIN), self.int(key as i32)]),
                Some(Deferred::Resume(key)) => self.http_pump(key),
            }
        }
    }

    /// Tells JS a TCP socket closed.
    ///
    /// # Arguments
    /// * `key` - slab key the socket had.
    /// * `fault` - the error that closed it, if any.
    pub(crate) fn fire_close(&mut self, key: usize, fault: Option<Fault>) {
        match fault {
            Some(fault) => self.fire(&[
                self.int(ev::CLOSE),
                self.int(key as i32),
                self.text(&fault.message),
                self.text(fault.code),
            ]),
            None => self.fire(&[self.int(ev::CLOSE), self.int(key as i32)]),
        }
    }

    // ------------------------------------------------------------ loop phases
}
