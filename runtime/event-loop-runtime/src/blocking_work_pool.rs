//! Bounded worker pool for work that would block the reactor (filesystem, DNS).
//!
//! Workers never touch sockets or the JS engine. They receive [`Work`] over a bounded
//! channel, run it, send a [`ResultMsg`] back and wake the reactor.
//!
//! Design decisions:
//! - **Lazy threads.** No thread exists until the first job is submitted, so a script
//!   that never blocks pays nothing. Threads are added only while jobs outnumber idle
//!   workers, up to the core count.
//! - **Nothing is dropped.** When the bounded queue is full the job is parked in a
//!   reactor-side overflow queue and re-submitted as results come back. The reactor
//!   never blocks on the pool and never loses a job.
//! - **Coalesced wakes.** A worker wakes the reactor only when no wake is already
//!   pending, so a burst of completions costs one wake syscall.

use crate::blocking_filesystem_operations::{self, FsValue};
use crate::reactor_io_state::Fault;
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use mio::Waker;
use std::collections::VecDeque;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

/// Capacity of the work queue and of the result queue.
const QUEUE_DEPTH: usize = 256;

/// Work item sent from the reactor to a worker thread.
pub enum Work {
    /// One filesystem operation.
    Fs {
        /// Request id chosen by `node:fs`'s natives; echoed back in the result.
        req: i32,
        /// Operation code, one of [`fs::op`].
        op: i32,
        /// Target path.
        path: String,
        /// Payload for writes; the destination path for renames; empty otherwise.
        data: Vec<u8>,
    },
    /// Resolve a host name for an outbound connection.
    Resolve {
        /// Slab key of the pending connection.
        conn: usize,
        /// Host name to resolve.
        host: String,
        /// Port to attach to the resolved address.
        port: u16,
    },
}

/// Result sent back from a worker to the reactor.
pub enum ResultMsg {
    /// Outcome of a [`Work::Fs`] job.
    Fs {
        /// Request id from the job.
        req: i32,
        /// Value on success, fault on failure.
        result: Result<FsValue, Fault>,
    },
    /// Outcome of a [`Work::Resolve`] job.
    Resolved {
        /// Slab key of the pending connection.
        conn: usize,
        /// First resolved address, or the failure.
        addr: Result<SocketAddr, Fault>,
    },
}

/// State a worker thread shares with the reactor.
#[derive(Clone)]
struct Link {
    /// Jobs to run.
    work: Receiver<Work>,
    /// Completed jobs.
    results: Sender<ResultMsg>,
    /// Wakes the reactor out of `poll`.
    waker: Arc<Waker>,
    /// True while a wake is pending or being handled by the reactor.
    notified: Arc<AtomicBool>,
    /// Number of workers currently blocked waiting for a job.
    idle: Arc<AtomicUsize>,
}

/// Reactor-side handle of the worker pool.
pub struct Pool {
    /// Sending half of the bounded work queue.
    work_tx: Sender<Work>,
    /// Receiving half of the bounded result queue.
    result_rx: Receiver<ResultMsg>,
    /// Template cloned into each spawned worker.
    link: Link,
    /// Jobs that did not fit in the bounded queue, oldest first.
    overflow: VecDeque<Work>,
    /// Threads spawned so far.
    spawned: usize,
    /// Upper bound on threads: the number of cores.
    max: usize,
    /// Jobs submitted and not yet received back; keeps the event loop alive.
    pub pending: usize,
}

impl Pool {
    /// Creates an empty pool. No thread is spawned here.
    ///
    /// # Arguments
    /// * `waker` - wakes the reactor when a result is ready.
    ///
    /// # Returns
    /// The pool handle owned by the reactor.
    pub fn new(waker: Arc<Waker>) -> Self {
        let (work_tx, work_rx) = bounded(QUEUE_DEPTH);
        let (result_tx, result_rx) = bounded(QUEUE_DEPTH);
        Self {
            work_tx,
            result_rx,
            link: Link {
                work: work_rx,
                results: result_tx,
                waker,
                notified: Arc::new(AtomicBool::new(false)),
                idle: Arc::new(AtomicUsize::new(0)),
            },
            overflow: VecDeque::new(),
            spawned: 0,
            max: thread::available_parallelism().map_or(4, |n| n.get()),
            pending: 0,
        }
    }

    /// Queues a job. Never blocks and never drops.
    ///
    /// # Arguments
    /// * `work` - the job to run on a worker.
    pub fn submit(&mut self, work: Work) {
        self.pending += 1;
        if self.overflow.is_empty() {
            if let Err(TrySendError::Full(work) | TrySendError::Disconnected(work)) =
                self.work_tx.try_send(work)
            {
                self.overflow.push_back(work);
            }
        } else {
            // Keep submission order: older overflowed jobs go first.
            self.overflow.push_back(work);
        }
        if self.spawned < self.max && self.work_tx.len() > self.link.idle.load(Ordering::Relaxed) {
            self.spawn();
        }
    }

    /// Spawns one more worker thread.
    fn spawn(&mut self) {
        let link = self.link.clone();
        let named = thread::Builder::new().name(format!("lunarstorm-worker-{}", self.spawned));
        if named.spawn(move || worker(link)).is_ok() {
            self.spawned += 1;
        }
    }

    /// Marks the pending wake as handled. Call before draining results so a result
    /// that arrives during the drain triggers a fresh wake.
    pub fn begin_drain(&self) {
        self.link.notified.store(false, Ordering::Release);
    }

    /// Takes one completed job, refilling the work queue from the overflow.
    ///
    /// # Returns
    /// The next result, or `None` when the result queue is empty.
    pub fn try_recv(&mut self) -> Option<ResultMsg> {
        let msg = self.result_rx.try_recv().ok()?;
        self.pending -= 1;
        while let Some(work) = self.overflow.pop_front() {
            if let Err(TrySendError::Full(work) | TrySendError::Disconnected(work)) =
                self.work_tx.try_send(work)
            {
                self.overflow.push_front(work);
                break;
            }
        }
        Some(msg)
    }
}

/// Worker thread entry point: run jobs until the pool is dropped.
///
/// # Arguments
/// * `link` - channels and wake state shared with the reactor.
fn worker(link: Link) {
    loop {
        link.idle.fetch_add(1, Ordering::Relaxed);
        let work = link.work.recv();
        link.idle.fetch_sub(1, Ordering::Relaxed);
        let msg = match work {
            Ok(Work::Fs { req, op, path, data }) => ResultMsg::Fs {
                req,
                result: blocking_filesystem_operations::exec(op, &path, &data),
            },
            Ok(Work::Resolve { conn, host, port }) => ResultMsg::Resolved {
                conn,
                addr: resolve(&host, port),
            },
            Err(_) => return,
        };
        if link.results.send(msg).is_err() {
            return;
        }
        if !link.notified.swap(true, Ordering::AcqRel) {
            let _ = link.waker.wake();
        }
    }
}

/// Resolves a host name with the system resolver (blocking).
///
/// # Arguments
/// * `host` - host name or address literal.
/// * `port` - port for the resulting socket address.
///
/// # Returns
/// The first address the resolver reports.
///
/// # Errors
/// A fault with code `ENOTFOUND` when the name does not resolve.
fn resolve(host: &str, port: u16) -> Result<SocketAddr, Fault> {
    let not_found = || Fault {
        code: "ENOTFOUND",
        message: format!("getaddrinfo ENOTFOUND {host}"),
    };
    (host, port)
        .to_socket_addrs()
        .map_err(|_| not_found())?
        .next()
        .ok_or_else(not_found)
}
