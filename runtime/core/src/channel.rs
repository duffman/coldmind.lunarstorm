//! A bounded byte-message queue between two threads: the transport of
//! `lunarstorm:ipc` (host to program) and, later, of workers.
//!
//! The producer side is `Send + Clone` ([`Inbox`]); the consumer side runs on the
//! Runtime's loop thread. Messages are opaque byte vectors.
//!
//! Decisions:
//! - Bounds are those of the jetbunny channel this was ported from: 4096 unread
//!   messages, 64 MiB of unread bytes, 8 MiB per message. A message past a bound is
//!   refused (`post` returns `false`) and nothing is queued, so a fast producer
//!   cannot grow memory without limit; the refusal is the back-pressure signal.
//! - `post` returning `true` is a promise that the consumer will get the message
//!   unless it ends first; a message is counted out only when it is taken.
//! - One mutex guards the queue and its byte count. jetbunny used two atomics so a
//!   sender never waits on the receiver; here a post and a take each hold the lock
//!   for a `VecDeque` push or pop, which is shorter than the wake syscall that
//!   follows a post, so the lock-free credit was not ported.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Most unread messages a channel holds.
pub const MAX_MESSAGES: usize = 4096;
/// Most unread bytes a channel holds.
pub const MAX_BYTES: usize = 64 << 20;
/// Longest single message.
pub const MAX_MESSAGE_BYTES: usize = 8 << 20;

/// The unread messages and their total size.
#[derive(Default)]
struct Queue {
    /// Messages in arrival order.
    messages: VecDeque<Vec<u8>>,
    /// Sum of the lengths in `messages`.
    bytes: usize,
}

/// The shared state of one channel.
#[derive(Default)]
pub struct Channel {
    /// Unread messages.
    queue: Mutex<Queue>,
    /// Set by [`Channel::close`]; nothing is accepted afterwards.
    closed: AtomicBool,
}

impl Channel {
    /// Queues a message.
    ///
    /// # Arguments
    /// * `bytes` - the message.
    ///
    /// # Returns
    /// `true` when it was queued. `false` when the channel is closed, the message is
    /// longer than [`MAX_MESSAGE_BYTES`], or it would take the queue past
    /// [`MAX_MESSAGES`] or [`MAX_BYTES`]; the bytes are dropped then.
    pub fn post(&self, bytes: Vec<u8>) -> bool {
        if self.is_closed() || bytes.len() > MAX_MESSAGE_BYTES {
            return false;
        }
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        // `closed` is read again under the lock: a close that lands between the check
        // above and the push must not let a message in behind it.
        if self.is_closed() || queue.messages.len() >= MAX_MESSAGES || queue.bytes + bytes.len() > MAX_BYTES {
            return false;
        }
        queue.bytes += bytes.len();
        queue.messages.push_back(bytes);
        true
    }

    /// Takes the oldest unread message.
    ///
    /// # Returns
    /// The message, or `None` when none is queued.
    pub fn take(&self) -> Option<Vec<u8>> {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        let message = queue.messages.pop_front()?;
        queue.bytes -= message.len();
        Some(message)
    }

    /// # Returns
    /// Whether an unread message is queued.
    pub fn has_messages(&self) -> bool {
        !self.queue.lock().unwrap_or_else(|e| e.into_inner()).messages.is_empty()
    }

    /// Refuses every later [`Channel::post`]. Messages already queued stay readable.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// # Returns
    /// Whether [`Channel::close`] was called.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

/// Wakes a Runtime's loop from another thread: sets the flag the loop reads at the
/// top of a step and kicks the poller, coalescing a burst into one kick.
#[derive(Clone)]
pub struct Nudge {
    /// Wakes the loop out of `poll`.
    waker: Arc<mio::Waker>,
    /// Whether work is queued for the loop; shared with the Runtime's `Poster`.
    pending: Arc<AtomicBool>,
}

impl Nudge {
    /// # Arguments
    /// * `waker` - the loop's waker.
    /// * `pending` - the loop's "work is queued" flag.
    ///
    /// # Returns
    /// The handle.
    pub(crate) fn new(waker: Arc<mio::Waker>, pending: Arc<AtomicBool>) -> Self {
        Self { waker, pending }
    }

    /// Tells the loop there is something to look at.
    pub fn nudge(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            let _ = self.waker.wake();
        }
    }
}

/// The producer side of a channel into a Runtime's program, usable from any thread.
#[derive(Clone)]
pub struct Inbox {
    /// The queue.
    channel: Arc<Channel>,
    /// Wakes the Runtime that reads it.
    nudge: Nudge,
}

impl Inbox {
    /// # Arguments
    /// * `channel` - the queue the program reads.
    /// * `nudge` - wakes the program's loop.
    ///
    /// # Returns
    /// The handle.
    pub(crate) fn new(channel: Arc<Channel>, nudge: Nudge) -> Self {
        Self { channel, nudge }
    }

    /// Queues a message for the program's receiver; a message posted before the
    /// program has set one waits for it.
    ///
    /// # Arguments
    /// * `bytes` - the message.
    ///
    /// # Returns
    /// `true` when the program will receive it unless it ends first; `false` when
    /// the channel is closed or a bound of [`Channel::post`] would be passed.
    pub fn post(&self, bytes: Vec<u8>) -> bool {
        let accepted = self.channel.post(bytes);
        if accepted {
            self.nudge.nudge();
        }
        accepted
    }

    /// Closes the channel from the host: later posts return `false`, messages already
    /// queued are still delivered, and the receiver then stops keeping the program
    /// alive, so it ends once nothing else is pending.
    pub fn close(&self) {
        self.channel.close();
        self.nudge.nudge();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message past the count bound is refused and nothing is queued for it.
    #[test]
    fn refuses_past_the_message_count() {
        let channel = Channel::default();
        for _ in 0..MAX_MESSAGES {
            assert!(channel.post(vec![1]));
        }
        assert!(!channel.post(vec![2]), "message {} must be refused", MAX_MESSAGES + 1);
        assert_eq!(channel.take(), Some(vec![1]));
        assert!(channel.post(vec![3]), "taking one makes room for one");
    }

    /// The byte bound counts unread bytes, and a taken message frees its bytes.
    #[test]
    fn refuses_past_the_byte_total() {
        let channel = Channel::default();
        for _ in 0..8 {
            assert!(channel.post(vec![0; MAX_MESSAGE_BYTES]), "8 messages of 8 MiB fill the 64 MiB");
        }
        assert!(!channel.post(vec![0]), "one more byte is past 64 MiB");
        assert_eq!(channel.take().map(|m| m.len()), Some(MAX_MESSAGE_BYTES));
        assert!(channel.post(vec![0; MAX_MESSAGE_BYTES]));
    }

    /// One message longer than the per-message bound is refused on its own.
    #[test]
    fn refuses_an_oversize_message() {
        let channel = Channel::default();
        assert!(!channel.post(vec![0; MAX_MESSAGE_BYTES + 1]));
        assert!(!channel.has_messages(), "the refused message was not queued");
        assert!(channel.post(vec![0; MAX_MESSAGE_BYTES]));
    }

    /// A closed channel accepts nothing but keeps what it holds.
    #[test]
    fn closing_refuses_new_messages_and_keeps_old_ones() {
        let channel = Channel::default();
        assert!(channel.post(vec![7]));
        channel.close();
        assert!(!channel.post(vec![8]));
        assert_eq!(channel.take(), Some(vec![7]));
        assert_eq!(channel.take(), None);
    }

    /// Messages come out in the order they went in.
    #[test]
    fn keeps_arrival_order() {
        let channel = Channel::default();
        for n in 0..5u8 {
            assert!(channel.post(vec![n]));
        }
        let taken: Vec<u8> = std::iter::from_fn(|| channel.take()).map(|m| m[0]).collect();
        assert_eq!(taken, [0, 1, 2, 3, 4]);
    }
}
