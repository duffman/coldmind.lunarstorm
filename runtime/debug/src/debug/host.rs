//! Dedicated-process stdio DAP transport. The owner constructs all engines/VMs locally;
//! only debugger/control metadata crosses threads. Protocol stdout is exclusively framed.
use super::{DapAdapter, DebugTargetBroker, DebugTargetId, VmDebugHandle};
use crate::vm::VmHandle;
use std::{
    io::{Read, Write},
    sync::{
        Arc,
        mpsc::{self, RecvTimeoutError, TryRecvError},
    },
    time::Duration,
};

/// Owned metadata handed off before application entry, never native engine values.
pub struct OwnerSession {
    /// Exclusive frontend observer and protocol controller.
    pub debugger: VmDebugHandle,
    /// Urgent cooperative stop control for this owned runtime only.
    pub stop: VmHandle,
    /// Optional bounded registry and root identity for independent child sessions.
    pub targets: Option<(Arc<DebugTargetBroker>, DebugTargetId)>,
}

/// Runs a dedicated stdio process with a cooperatively stoppable VM owner.
///
/// EOF or broken transport requests owner stop, then releases the debugger entry gate.
/// DAP disconnect only detaches/resumes; it does not stop the owned application.
/// Native callbacks must return before cooperative stop can complete. Blocking OS I/O
/// threads belong to this dedicated process and end when its streams/process close.
pub fn run<F>(owner: F) -> Result<(), String>
where
    F: FnOnce(mpsc::SyncSender<OwnerSession>) -> Result<(), String> + Send + 'static,
{
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let owner = std::thread::spawn(move || {
        let result = owner(ready_tx);
        let _ = done_tx.send(result);
    });
    let session = loop {
        match ready_rx.recv_timeout(Duration::from_millis(5)) {
            Ok(session) => break session,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                let result = done_rx
                    .recv()
                    .unwrap_or_else(|_| Err("debugger owner panicked".into()));
                let _ = owner.join();
                return result;
            }
        }
    };
    let stop = session.stop;
    let targets = session.targets.as_ref().map(|(broker, _)| broker.clone());
    let adapter = match session.targets {
        Some((broker, root)) => DapAdapter::with_targets(session.debugger, broker, root),
        None => DapAdapter::new(session.debugger),
    };
    let mut adapter = match adapter {
        Ok(adapter) => adapter,
        Err(error) => {
            let _ = stop.request_stop();
            if let Some(broker) = &targets {
                let _ = broker.close();
            }
            let _ = owner.join();
            return Err(error.to_string());
        }
    };
    // Fixed read size and queue capacities bound pending OS input/output independently
    // of the adapter's validated framing and native request limits.
    let (input_tx, input_rx) = mpsc::sync_channel(8);
    std::thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        loop {
            let mut bytes = vec![0; 4096];
            match input.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    bytes.truncate(n);
                    if input_tx.send(Ok(bytes)).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    let _ = input_tx.send(Err(e.to_string()));
                    break;
                }
            }
        }
    });
    let (output_tx, output_rx) = mpsc::sync_channel::<Vec<u8>>(32);
    let (writer_error_tx, writer_error_rx) = mpsc::sync_channel(1);
    let writer = std::thread::spawn(move || {
        let mut output = std::io::stdout().lock();
        for frame in output_rx {
            if let Err(e) = output.write_all(&frame).and_then(|_| output.flush()) {
                let _ = writer_error_tx.send(e.to_string());
                return;
            }
        }
    });
    let mut transport_closed = false;
    let result = loop {
        let step = (|| -> Result<(), String> {
            if let Ok(error) = writer_error_rx.try_recv() {
                return Err(error);
            }
            let mut frames = adapter.poll().map_err(|e| e.to_string())?;
            if !transport_closed && !adapter.is_disconnected() {
                match input_rx.recv_timeout(Duration::from_millis(2)) {
                    Ok(Ok(bytes)) => {
                        frames.extend(adapter.receive(&bytes).map_err(|e| e.to_string())?)
                    }
                    Ok(Err(e)) => return Err(e),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        transport_closed = true;
                        stop.request_stop().map_err(|e| e.to_string())?;
                        if let Some(broker) = &targets {
                            broker.close().map_err(|e| e.to_string())?;
                        }
                        adapter.end_input().map_err(|e| e.to_string())?;
                    }
                }
            } else {
                std::thread::sleep(Duration::from_millis(2));
            }
            for frame in frames {
                output_tx
                    .try_send(frame)
                    .map_err(|_| "DAP output queue is full or closed".to_owned())?;
            }
            Ok(())
        })();
        if let Err(error) = step {
            let _ = stop.request_stop();
            if let Some(broker) = &targets {
                let _ = broker.close();
            }
            let _ = adapter.end_input();
            break Err(error);
        }
        match done_rx.try_recv() {
            Ok(result) => {
                // Owner completion may have published its terminal update after this poll.
                match adapter.poll() {
                    Ok(frames) => {
                        let mut failed = false;
                        for frame in frames {
                            if output_tx.try_send(frame).is_err() {
                                failed = true;
                                break;
                            }
                        }
                        if failed {
                            break Err("DAP output queue is full or closed".into());
                        }
                    }
                    Err(e) if !transport_closed => break Err(e.to_string()),
                    Err(_) => {}
                }
                break if transport_closed { Ok(()) } else { result };
            }
            Err(TryRecvError::Disconnected) => break Err("debugger owner panicked".into()),
            Err(TryRecvError::Empty) => {}
        }
    };
    if let Some(broker) = &targets {
        let _ = broker.close();
    }
    drop(adapter);
    // No native-callback preemption: joining waits for the owner to service stop.
    let owner_result = owner
        .join()
        .map_err(|_| "debugger owner panicked".to_owned());
    drop(output_tx);
    let writer_result = if result.is_ok() {
        writer
            .join()
            .map_err(|_| "DAP writer panicked".to_owned())
            .and_then(|_| writer_error_rx.try_recv().map_or(Ok(()), Err))
    } else {
        Ok(())
    };
    result.and(owner_result).and(writer_result)
}
