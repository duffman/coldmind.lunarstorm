//! One step of the event loop, split by what it handles: [`loop_turn`] owns the
//! step itself, settling and fatal errors; [`scheduled_callbacks`] runs timers,
//! immediates, posted tasks, worker and IPC messages and pool results;
//! [`tcp_readiness_events`] accepts, reads and flushes sockets;
//! [`http_connection_pump`] parses requests and hands them to JS.

pub(crate) mod http_connection_pump;
pub(crate) mod loop_turn;
pub(crate) mod scheduled_callbacks;
pub(crate) mod tcp_readiness_events;

pub(crate) use loop_turn::Turn;

use std::time::Duration;

/// Size of the loop's read buffer.
pub(crate) const SCRATCH: usize = 64 * 1024;
/// Readiness events taken from the kernel per `poll` call.
pub(crate) const EVENT_BATCH: usize = 1024;
/// Longest the loop goes without asking the kernel for readiness while immediates
/// or posted tasks keep it busy.
///
/// Decision: a zero-timeout kernel poll costs about 13 microseconds on macOS, far
/// more than running an immediate callback. At 250 microseconds between polls the
/// syscall is about 5% of a busy loop's time, and I/O that becomes ready during a
/// storm of immediates waits at most this long plus one callback.
pub(crate) const POLL_GAP: Duration = Duration::from_micros(250);
