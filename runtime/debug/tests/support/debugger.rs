//! Real-VM debugger controller helpers shared by native acceptance suites.
//! The watchdog remains armed until the owner joins, even after controller return.
#![cfg(feature = "quickjs-ng")]
use coldmind_runtime::{debug::*, engines::quickjs_ng::QuickJsNgEngine, vm::*};
use std::{
    thread,
    time::{Duration, Instant},
};
type Vm = VirtualMachine<QuickJsNgEngine>;

/// The owner joins after its native operation finishes, cancelling the safety
/// deadline. The deadline remains active even if the controller has returned.
pub struct Controller<T> {
    worker: Option<thread::JoinHandle<T>>,
    watchdog: Option<thread::JoinHandle<()>>,
    cancel: std::sync::mpsc::Sender<()>,
    stop: VmHandle,
}
impl<T> Controller<T> {
    pub fn join(mut self) -> thread::Result<T> {
        let _ = self.cancel.send(());
        self.watchdog.take().unwrap().join().unwrap();
        self.worker.take().unwrap().join()
    }
}
impl<T> Drop for Controller<T> {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.stop.request_stop();
        }
        let _ = self.cancel.send(());
        if let Some(watchdog) = self.watchdog.take() {
            let _ = watchdog.join();
        }
    }
}
pub fn controller<T: Send + 'static>(
    vm: &Vm,
    body: impl FnOnce() -> T + Send + 'static,
) -> Controller<T> {
    let stop = vm.handle();
    let deadline_stop = vm.handle();
    let (cancel, canceled) = std::sync::mpsc::channel();
    let watchdog = thread::spawn(move || {
        if canceled.recv_timeout(Duration::from_secs(20)).is_err() {
            let _ = deadline_stop.request_stop();
        }
    });
    let worker =
        thread::spawn(
            move || match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
                Ok(value) => value,
                Err(panic) => {
                    let _ = stop.request_stop();
                    std::panic::resume_unwind(panic)
                }
            },
        );
    Controller {
        worker: Some(worker),
        watchdog: Some(watchdog),
        cancel,
        stop: vm.handle(),
    }
}
pub fn update(debug: &VmDebugHandle) -> DebugUpdate {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(update) = debug.try_recv() {
            return update;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for debugger update; state={:?}",
            debug.state()
        );
        if debug.state() == DebugExecutionState::Closed {
            // Closure and its terminal record may arrive after the first poll.
            // Re-read under the transport lock before declaring the stream drained.
            return debug
                .try_recv()
                .expect("debugger closed before expected update");
        }
        thread::yield_now();
    }
}
pub fn send(debug: &VmDebugHandle, request: DebugRequest) -> DebugRequestId {
    match debug.try_request(request).unwrap() {
        DebugRequestReceipt::Accepted { id }
        | DebugRequestReceipt::AcceptedWakeFailed { id, .. } => id,
    }
}
/// Wait for the terminal reply of `id`, skipping asynchronous notifications.
pub fn reply(debug: &VmDebugHandle, id: DebugRequestId) -> Result<DebugReply, DebugError> {
    loop {
        match update(debug) {
            DebugUpdate::Reply { id: actual, result } if actual == id => return result,
            DebugUpdate::SessionClosed { .. } => panic!("session closed before reply"),
            _ => {}
        }
    }
}
pub fn ask(debug: &VmDebugHandle, request: DebugRequest) -> DebugReply {
    let id = send(debug, request);
    reply(debug, id).unwrap()
}
pub fn paused(debug: &VmDebugHandle) -> (PauseId, PauseReason, Option<DebugLocation>) {
    loop {
        match update(debug) {
            DebugUpdate::Paused {
                pause,
                reason,
                location,
                ..
            } => return (pause, reason, location),
            DebugUpdate::SessionClosed { .. } => panic!("session closed before pause"),
            _ => {}
        }
    }
}
