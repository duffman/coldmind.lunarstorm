//! Callbacks the loop schedules itself: expired timers, `setImmediate`, tasks
//! posted from other threads, worker and IPC messages, and finished pool jobs.

use crate::runtime_public_types::ev;
use crate::worker_threads_module::WorkerEvent;
use crate::host_services_trait::Host;
use crate::internal_natives;
use crate::blocking_work_pool::ResultMsg;
use lunarstorm_engine_contract::{
    Scope, Thrown, Vm,
};
use std::sync::atomic::Ordering;
use super::*;

impl<'s, V: Vm, H: Host> Turn<'_, 's, V, H> {
    /// Fires every timer that is due. Each callback is its own JS entry, so promise
    /// jobs run between timers.
    pub(crate) fn run_timers(&mut self) {
        let mut now = None;
        loop {
            let due = {
                let mut io = self.shared.io.borrow_mut();
                // Read the clock only when a timer exists, and once per phase.
                io.next_deadline()
                    .and_then(|_| io.timer_pop(*now.get_or_insert_with(|| self.shared.host.monotonic())))
            };
            let Some((key, one_shot)) = due else { return };
            self.fire(&[self.int(ev::TIMER), self.int(key as i32), self.scope.boolean(one_shot)]);
            self.drain_deferred();
        }
    }

    /// Runs the `setImmediate` callbacks queued before this phase, one JS entry each.
    pub(crate) fn run_immediates(&mut self) {
        loop {
            self.entry();
            self.beat();
            let more = match self.scope.call(&self.dispatch, &[self.int(ev::IMMEDIATE)]) {
                Ok(more) => self.scope.as_bool(&more).unwrap_or(false),
                // The callback that threw was already taken off the queue; the
                // next phase runs whatever is left.
                Err(Thrown) => {
                    self.raise();
                    false
                }
            };
            self.settle();
            self.beat();
            self.drain_deferred();
            if !more {
                return;
            }
        }
    }

    /// Runs the tasks other threads posted, one JS entry each.
    pub(crate) fn drain_tasks(&mut self) {
        // Cleared before draining, so a task posted during the drain wakes the loop
        // again instead of being missed.
        self.pending.store(false, Ordering::Release);
        while let Ok(task) = self.tasks.try_recv() {
            self.entry();
            self.beat();
            task(&self.scope);
            self.settle();
            self.beat();
            self.drain_deferred();
        }
        self.drain_messages();
        self.drain_workers();
    }

    /// Hands the messages and exits of the workers this Runtime started to the
    /// listeners and `exited` callbacks that wait for them, one JS entry each.
    pub(crate) fn drain_workers(&mut self) {
        while let Some(event) = self.workers.next_event(&self.scope, &self.shared.refs) {
            let (function, args) = match event {
                WorkerEvent::Message { listener, bytes } => match self.scope.bytes(bytes) {
                    Ok(message) => (listener, vec![message]),
                    Err(Thrown) => self.fatal(),
                },
                WorkerEvent::Exit { callback, state, message } => {
                    let mut args = vec![self.text(state)];
                    if let Some(message) = message {
                        args.push(self.text(&message));
                    }
                    (callback, args)
                }
            };
            self.beat();
            if self.scope.call(&function, &args).is_err() {
                self.raise();
            }
            self.settle();
            self.beat();
            self.drain_deferred();
        }
    }

    /// Hands the messages the host posted to the script's receiver, one JS entry
    /// each, in arrival order. With no receiver set they stay queued. Once the host
    /// has closed the channel and the queue is empty, the receiver is released so it
    /// no longer keeps the loop alive.
    pub(crate) fn drain_messages(&mut self) {
        while let Some(receiver) = self.ipc.receiver(&self.scope) {
            let Some(bytes) = self.ipc.channel.take() else { break };
            let message = match self.scope.bytes(bytes) {
                Ok(message) => message,
                Err(Thrown) => self.fatal(),
            };
            self.beat();
            if self.scope.call(&receiver, &[message]).is_err() {
                self.raise();
            }
            self.settle();
            self.beat();
            self.drain_deferred();
        }
        if self.ipc.channel.is_closed() && !self.ipc.channel.has_messages() {
            self.ipc.clear(&self.shared.refs);
        }
    }

    /// Delivers worker-pool results to JS.
    pub(crate) fn drain_pool(&mut self) {
        self.shared.io.borrow().pool.begin_drain();
        loop {
            let message = self.shared.io.borrow_mut().pool.try_recv();
            match message {
                None => return,
                Some(ResultMsg::Fs { req, result: Ok(value) }) => match internal_natives::fs_value::<V>(&self.scope, value) {
                    Ok(value) => self.fire(&[self.int(ev::FS), self.int(req), value]),
                    Err(Thrown) => self.fatal(),
                },
                Some(ResultMsg::Fs { req, result: Err(fault) }) => self.fire(&[
                    self.int(ev::FS),
                    self.int(req),
                    self.scope.undefined(),
                    self.text(&fault.message),
                    self.text(fault.code),
                ]),
                Some(ResultMsg::Resolved { conn, addr }) => {
                    self.shared.io.borrow_mut().connect(conn, addr);
                }
            }
            self.drain_deferred();
        }
    }

    // -------------------------------------------------------------- HTTP pump
}
