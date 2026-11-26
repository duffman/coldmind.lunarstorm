//! Reactor-owned I/O state: sockets, write buffers, timers and deferred events.
//!
//! Everything here lives on the reactor thread behind one `RefCell` ([`Shared::io`]).
//! Native ops called from JS borrow it briefly; the event loop never holds a borrow
//! while JS runs, which is what makes the re-entrant calls sound.
//!
//! Design decisions:
//! - **One slab for every handle.** Listeners, connections and pending connects share
//!   one [`Slab`]; the slab key is the mio token and the handle id seen by JS.
//! - **Register once.** Sockets are registered for `READABLE | WRITABLE` when created
//!   and never re-registered. mio is edge-triggered, so an idle writable socket costs
//!   nothing, and a response never needs a `reregister` syscall.
//! - **Write first, buffer the tail.** [`Conn::send`] writes straight to the socket
//!   with one vectored syscall and copies only what the kernel did not accept.
//! - **Tombstones.** When the reactor closes a connection that JS still knows about,
//!   the slot becomes [`Entry::Closed`] until the close event is delivered, so the key
//!   cannot be reused by a new connection while a stale event is in flight.

use crate::host::{Host, Stream};
use lunarstorm_http1::{self as http, Framing, Phase};
use crate::pool::Pool;
use crate::runtime::Arg;
use bytes::{Buf, BytesMut};
use mio::net::{TcpListener, TcpStream};
use mio::{Interest, Registry, Token};
use slab::Slab;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};
use std::io::{self, IoSlice, Write};
use std::net::{Shutdown, SocketAddr};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Token reserved for the pool's waker. Slab keys never reach it.
pub const WAKER: Token = Token(usize::MAX);
/// Queued outbound bytes above which `socket.write` reports back-pressure.
const HIGH_WATER: usize = 64 * 1024;
/// Buffered stdout bytes above which the buffer is flushed without waiting for the loop.
const STDOUT_HIGH: usize = 64 * 1024;
/// Multiplier that packs a timer's slab key above its 32-bit sequence number in the
/// id handed to JS. Mirrored by `TIMER_SHIFT` in `js/bootstrap.js`.
const TIMER_SHIFT: f64 = 4_294_967_296.0;
/// Largest number of concurrently armed timers; keeps packed ids below 2^53.
const TIMER_KEYS: usize = 1 << 21;

/// Connection state bits, stored in [`Conn::flags`].
pub mod flag {
    /// The connection speaks HTTP and is driven by the request pump.
    pub const HTTP: u16 = 1;
    /// A request has been dispatched to JS and its response is not finished.
    pub const BUSY: u16 = 1 << 1;
    /// The connection may be reused after the current response.
    pub const KEEP_ALIVE: u16 = 1 << 2;
    /// Close (HTTP) or half-close (TCP) once the write buffer drains.
    pub const ENDING: u16 = 1 << 3;
    /// The peer has closed its sending side.
    pub const READ_EOF: u16 = 1 << 4;
    /// An outbound connect is in progress.
    pub const CONNECTING: u16 = 1 << 5;
    /// Responses are being batched; `send` only buffers.
    pub const CORKED: u16 = 1 << 6;
    /// The current request is `HEAD`: send headers, drop the body.
    pub const HEAD_ONLY: u16 = 1 << 7;
    /// The current response uses chunked transfer encoding.
    pub const CHUNKED_OUT: u16 = 1 << 8;
    /// JS was told to wait; emit `drain` when the write buffer empties.
    pub const WANT_DRAIN: u16 = 1 << 9;
    /// The request pump is on the stack for this connection.
    pub const IN_PUMP: u16 = 1 << 10;
    /// Our sending side has been shut down.
    pub const WRITE_SHUT: u16 = 1 << 11;
    /// The socket may hold unread bytes (edge-triggered readiness, tracked by hand).
    pub const READABLE: u16 = 1 << 12;
    /// The kernel reported the peer's close; keep reading until end of stream.
    pub const RDHUP: u16 = 1 << 13;
    /// The current request is HTTP/1.0.
    pub const V10: u16 = 1 << 14;
}

/// An error on its way to JS: a Node-style code and message.
pub struct Fault {
    /// Symbolic code such as `ENOENT` or `ECONNRESET`.
    pub code: &'static str,
    /// Human-readable message, beginning with the code.
    pub message: String,
}

impl Fault {
    /// Builds a fault from an I/O error.
    ///
    /// # Arguments
    /// * `error` - the underlying error.
    /// * `call` - the operation that failed (`open`, `read`, `listen`, ...).
    /// * `detail` - path or address the operation was applied to; may be empty.
    ///
    /// # Returns
    /// A fault whose message reads like Node's: `ENOENT: no such file or directory, open 'x'`.
    pub fn new(error: &io::Error, call: &str, detail: &str) -> Self {
        use io::ErrorKind as K;
        let (code, text) = match error.kind() {
            K::NotFound => ("ENOENT", "no such file or directory"),
            K::PermissionDenied => ("EACCES", "permission denied"),
            K::AlreadyExists => ("EEXIST", "file already exists"),
            K::IsADirectory => ("EISDIR", "illegal operation on a directory"),
            K::NotADirectory => ("ENOTDIR", "not a directory"),
            K::DirectoryNotEmpty => ("ENOTEMPTY", "directory not empty"),
            K::AddrInUse => ("EADDRINUSE", "address already in use"),
            K::AddrNotAvailable => ("EADDRNOTAVAIL", "address not available"),
            K::ConnectionRefused => ("ECONNREFUSED", "connection refused"),
            K::ConnectionReset => ("ECONNRESET", "connection reset by peer"),
            K::BrokenPipe => ("EPIPE", "broken pipe"),
            K::TimedOut => ("ETIMEDOUT", "operation timed out"),
            _ => ("EIO", ""),
        };
        let text = if text.is_empty() { error.to_string() } else { text.to_owned() };
        let message = if detail.is_empty() {
            format!("{code}: {text}, {call}")
        } else {
            format!("{code}: {text}, {call} '{detail}'")
        };
        Self { code, message }
    }
}

/// Per-connection state owned exclusively by the reactor thread.
pub struct Conn {
    /// The non-blocking socket.
    pub stream: TcpStream,
    /// HTTP only: bytes of a request that did not arrive in one read. Empty on the
    /// fast path, where requests are parsed in place from the reactor's scratch buffer.
    pub read_buf: BytesMut,
    /// Outgoing bytes the kernel has not accepted yet.
    pub write_buf: BytesMut,
    /// Slab key of the listener that accepted this connection; `usize::MAX` for
    /// outbound connections.
    pub owner: usize,
    /// State bits from [`flag`].
    pub flags: u16,
    /// HTTP only: progress of the request being received.
    pub phase: Phase,
}

impl Conn {
    /// Wraps a registered socket.
    ///
    /// # Arguments
    /// * `stream` - the socket, already registered with the poll.
    /// * `owner` - slab key of the accepting listener, or `usize::MAX`.
    /// * `flags` - initial state bits.
    ///
    /// # Returns
    /// A connection with empty buffers. Neither buffer allocates until first used.
    pub fn new(stream: TcpStream, owner: usize, flags: u16) -> Self {
        Self {
            stream,
            read_buf: BytesMut::new(),
            write_buf: BytesMut::new(),
            owner,
            flags,
            phase: Phase::Head,
        }
    }

    /// Sends up to three byte runs in order (typically head, body, trailer).
    ///
    /// When nothing is queued the runs go to the socket in one `writev`; only the part
    /// the kernel does not accept is copied into `write_buf`. When data is already
    /// queued, or the connection is corked or still connecting, the runs are appended
    /// so ordering is preserved.
    ///
    /// # Arguments
    /// * `parts` - byte runs to send; empty runs are allowed.
    ///
    /// # Errors
    /// The socket error when the connection is broken; the caller closes it.
    pub fn send(&mut self, parts: [&[u8]; 3]) -> io::Result<()> {
        if !self.write_buf.is_empty() || self.flags & (flag::CORKED | flag::CONNECTING) != 0 {
            for part in parts {
                self.write_buf.extend_from_slice(part);
            }
            return Ok(());
        }
        let mut slices = parts.map(IoSlice::new);
        let mut rest = &mut slices[..];
        let mut left: usize = parts.iter().map(|part| part.len()).sum();
        while left > 0 {
            match self.stream.write_vectored(rest) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => {
                    left -= n;
                    IoSlice::advance_slices(&mut rest, n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        if left > 0 {
            self.write_buf.reserve(left);
            for slice in rest.iter() {
                self.write_buf.extend_from_slice(slice);
            }
        }
        Ok(())
    }

    /// Writes queued bytes until the buffer is empty or the socket would block.
    ///
    /// # Returns
    /// `true` when the buffer is empty.
    ///
    /// # Errors
    /// The socket error when the connection is broken; the caller closes it.
    pub fn flush(&mut self) -> io::Result<bool> {
        while !self.write_buf.is_empty() {
            match self.stream.write(&self.write_buf) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => self.write_buf.advance(n),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }
}

/// One slot of the handle slab.
pub enum Entry {
    /// A listening socket.
    Listener {
        /// The non-blocking listener.
        sock: TcpListener,
        /// Whether accepted connections speak HTTP.
        http: bool,
    },
    /// An open connection.
    Conn(Conn),
    /// An outbound connection waiting for DNS on the worker pool.
    Resolving,
    /// A pending connect that JS destroyed; the slot is freed when DNS returns.
    Cancelled,
    /// A connection the reactor closed; the slot is freed when JS has been told.
    Closed,
}

/// Work the reactor must do after the current JS call returns.
pub enum Deferred {
    /// Tell JS a connection closed, with the error if there was one.
    Close(usize, Option<Fault>),
    /// Tell JS a connection's write buffer drained.
    Drain(usize),
    /// Resume the HTTP request pump for a connection whose response just finished.
    Resume(usize),
}

/// What became of a socket on a writable event.
pub enum Wake {
    /// Nothing JS needs to hear about.
    Quiet,
    /// An outbound connect completed.
    Connected,
    /// An outbound connect failed; the slot is already free.
    Failed(Fault),
}

/// An armed timer.
struct Timer {
    /// Sequence number of this arming; a heap entry is live only while it matches.
    seq: u32,
    /// Repeat period for intervals, `None` for one-shot timers.
    every: Option<Duration>,
}

/// Heap entry: when a timer is due.
struct Due {
    /// Deadline.
    at: Instant,
    /// Global arming order; breaks deadline ties so equal timers fire first-armed-first.
    order: u64,
    /// Slab key of the timer.
    key: u32,
    /// Sequence number the timer had when this entry was pushed.
    seq: u32,
}

impl PartialEq for Due {
    /// Entries are equal when deadline and arming order match.
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at && self.order == other.order
    }
}

impl Eq for Due {}

impl PartialOrd for Due {
    /// Defers to [`Ord`].
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Due {
    /// Reversed, so the max-heap `BinaryHeap` yields the earliest deadline first.
    fn cmp(&self, other: &Self) -> Ordering {
        other.at.cmp(&self.at).then_with(|| other.order.cmp(&self.order))
    }
}

/// State shared between the event loop and the native ops.
pub struct Shared<H> {
    /// All loop-owned I/O state.
    pub io: RefCell<Io>,
    /// Set by `process.nextTick` when the tick queue becomes non-empty.
    pub tick: Cell<bool>,
    /// Set by `setImmediate` when the immediate queue becomes non-empty.
    pub immediate: Cell<bool>,
    /// The host's monotonic clock at creation; origin of `performance.now()`.
    pub start: Instant,
    /// The host services. Reached from native functions and the loop alike.
    pub host: H,
    /// Settled `Runtime::call` results waiting to be collected: `(call id, result)`.
    pub calls: RefCell<Vec<(u32, Result<Arg, String>)>>,
    /// Whether the host holds the loop alive (`Runtime::hold`): it may still post
    /// work although no handle, timer or job is open.
    pub held: Cell<bool>,
    /// Native references that keep the loop alive: one per script-held receiver
    /// (`lunarstorm:ipc`'s `onMessage`). The loop is idle only when this is 0 and
    /// nothing else is open, armed, queued or held.
    pub refs: Cell<u32>,
    /// Set by a worker's `close()`: the loop reports idle at its next step.
    pub stop: Cell<bool>,
    /// Set from another thread when this Runtime is a worker that was terminated;
    /// the loop reports idle at its next step and an uncaught-error report is
    /// suppressed (the engine's interrupt surfaces as one).
    pub terminated: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Directory of the program's entry module, which worker paths resolve against.
    pub entry_dir: RefCell<String>,
    /// The pulse (`Runtime::pulse`): odd while a script is running. Written only
    /// when the host type asks for it (`Host::PULSE`).
    pub pulse: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

/// The reactor's I/O state.
pub struct Io {
    /// Registers sockets with the poll owned by the event loop.
    registry: Registry,
    /// Every listener, connection and pending connect, keyed by mio token.
    pub entries: Slab<Entry>,
    /// Armed timers.
    timers: Slab<Timer>,
    /// Deadlines of armed timers; may hold stale entries for cancelled timers.
    heap: BinaryHeap<Due>,
    /// Last timer sequence number handed out.
    timer_seq: u32,
    /// Last arming order handed out.
    timer_order: u64,
    /// Events to deliver to JS once the current JS call returns.
    deferred: VecDeque<Deferred>,
    /// Worker pool for blocking work.
    pub pool: Pool,
    /// Buffered `console.log` / `process.stdout.write` output, flushed before each poll.
    pub stdout: Vec<u8>,
    /// Reusable buffer for building response heads.
    head: Vec<u8>,
    /// Cached `date: ...\r\n` response line.
    date: [u8; 37],
    /// Unix second the cached date line was formatted for.
    date_secs: u64,
    /// Cleared once per loop turn so the clock is read at most once per turn.
    pub date_fresh: bool,
}

impl Io {
    /// Creates empty reactor state.
    ///
    /// # Arguments
    /// * `registry` - a clone of the event loop's poll registry.
    /// * `pool` - the worker pool.
    ///
    /// # Returns
    /// State with no handles and no timers.
    pub fn new(registry: Registry, pool: Pool) -> Self {
        Self {
            registry,
            entries: Slab::new(),
            timers: Slab::new(),
            heap: BinaryHeap::new(),
            timer_seq: 0,
            timer_order: 0,
            deferred: VecDeque::new(),
            pool,
            stdout: Vec::new(),
            head: Vec::with_capacity(512),
            date: [0; 37],
            date_secs: u64::MAX,
            date_fresh: false,
        }
    }

    /// Tells whether anything can still produce an event.
    ///
    /// # Returns
    /// True while a handle is open, a timer is armed or a pool job is outstanding.
    pub fn alive(&self) -> bool {
        !self.entries.is_empty() || !self.timers.is_empty() || self.pool.pending > 0
    }

    // ----------------------------------------------------------------- stdout

    /// Appends text to the stdout buffer, flushing when it grows large.
    ///
    /// # Arguments
    /// * `text` - bytes to print.
    /// * `host` - receives the buffer if it has to be flushed now.
    pub fn print<H: Host>(&mut self, text: &[u8], host: &H) {
        self.stdout.extend_from_slice(text);
        if self.stdout.len() >= STDOUT_HIGH {
            self.flush_stdout(host);
        }
    }

    /// Hands buffered stdout to the host.
    ///
    /// Decision: the Runtime owns the buffering (one host write per loop turn
    /// instead of one per `console.log`), the host owns the destination.
    ///
    /// # Arguments
    /// * `host` - the host whose stdout receives the bytes.
    pub fn flush_stdout<H: Host>(&mut self, host: &H) {
        if !self.stdout.is_empty() {
            host.write(Stream::Out, &self.stdout);
            self.stdout.clear();
        }
    }

    // ----------------------------------------------------------------- timers

    /// Arms a timer.
    ///
    /// # Arguments
    /// * `ms` - delay in milliseconds; values below 1, above 2^31-1 or NaN become 1,
    ///   matching Node.
    /// * `repeat` - whether the timer re-arms itself after firing.
    /// * `now` - the host's monotonic clock.
    ///
    /// # Returns
    /// The timer id for JS (`key * 2^32 + seq`), or `None` when [`TIMER_KEYS`] timers
    /// are already armed.
    pub fn timer_start(&mut self, ms: f64, repeat: bool, now: Instant) -> Option<f64> {
        if self.timers.vacant_key() >= TIMER_KEYS {
            return None;
        }
        let ms = if (1.0..=2_147_483_647.0).contains(&ms) { ms } else { 1.0 };
        let delay = Duration::from_micros((ms * 1000.0) as u64);
        self.timer_seq = self.timer_seq.wrapping_add(1);
        let seq = self.timer_seq;
        let key = self.timers.insert(Timer { seq, every: repeat.then_some(delay) });
        self.push_due(now + delay, key as u32, seq);
        Some(key as f64 * TIMER_SHIFT + f64::from(seq))
    }

    /// Cancels a timer.
    ///
    /// The heap entry is left in place and skipped when it surfaces; removal from the
    /// slab is O(1) and is what makes the timer dead.
    ///
    /// # Arguments
    /// * `id` - id returned by [`Io::timer_start`].
    ///
    /// # Returns
    /// True when the id named a live timer. A stale id (fired or already cleared, even
    /// if its key was reused since) returns false and cancels nothing.
    pub fn timer_cancel(&mut self, id: f64) -> bool {
        // NaN is rejected along with negatives: neither names a timer.
        if id.is_nan() || id < 0.0 {
            return false;
        }
        let key = (id / TIMER_SHIFT) as usize;
        let seq = (id % TIMER_SHIFT) as u32;
        match self.timers.get(key) {
            Some(timer) if timer.seq == seq => {
                self.timers.remove(key);
                true
            }
            _ => false,
        }
    }

    /// Pushes a deadline for a timer.
    ///
    /// # Arguments
    /// * `at` - when the timer is due.
    /// * `key` - the timer's slab key.
    /// * `seq` - the timer's current sequence number.
    fn push_due(&mut self, at: Instant, key: u32, seq: u32) {
        self.timer_order += 1;
        self.heap.push(Due { at, order: self.timer_order, key, seq });
    }

    /// Tests whether a heap entry still refers to an armed timer.
    ///
    /// # Arguments
    /// * `due` - the heap entry.
    ///
    /// # Returns
    /// True when the timer exists and has not been re-armed or replaced.
    fn live(&self, due: &Due) -> bool {
        self.timers.get(due.key as usize).is_some_and(|timer| timer.seq == due.seq)
    }

    /// Finds the earliest live deadline, discarding stale heap entries on the way.
    ///
    /// # Returns
    /// The next deadline, or `None` when no timer is armed.
    pub fn next_deadline(&mut self) -> Option<Instant> {
        if self.timers.is_empty() {
            self.heap.clear();
            return None;
        }
        while let Some(top) = self.heap.peek() {
            if self.live(top) {
                return Some(top.at);
            }
            self.heap.pop();
        }
        None
    }

    /// Takes one expired timer. Intervals are re-armed, one-shot timers are removed.
    ///
    /// Decision: an interval is re-armed at `now + period`, as Node does, so a slow
    /// callback delays the next run instead of making runs pile up.
    ///
    /// # Arguments
    /// * `now` - the time of the current timer phase; timers armed during the phase
    ///   are strictly later and run on a later turn.
    ///
    /// # Returns
    /// `(key, one_shot)` of a timer to fire, or `None` when none is due.
    pub fn timer_pop(&mut self, now: Instant) -> Option<(u32, bool)> {
        loop {
            if self.heap.peek()?.at > now {
                return None;
            }
            let due = self.heap.pop()?;
            if !self.live(&due) {
                continue;
            }
            return Some(match self.timers[due.key as usize].every {
                Some(every) => {
                    self.push_due(now + every, due.key, due.seq);
                    (due.key, false)
                }
                None => {
                    self.timers.remove(due.key as usize);
                    (due.key, true)
                }
            });
        }
    }

    // ------------------------------------------------------- handle lifecycle

    /// Opens a listening socket.
    ///
    /// # Arguments
    /// * `addr` - address to bind.
    /// * `http` - whether accepted connections speak HTTP.
    ///
    /// # Returns
    /// The listener's slab key and the bound port (useful when binding port 0).
    ///
    /// # Errors
    /// The bind or registration error.
    pub fn listen(&mut self, addr: SocketAddr, http: bool) -> io::Result<(usize, u16)> {
        let mut sock = TcpListener::bind(addr)?;
        let port = sock.local_addr()?.port();
        let slot = self.entries.vacant_entry();
        let key = slot.key();
        self.registry.register(&mut sock, Token(key), Interest::READABLE)?;
        slot.insert(Entry::Listener { sock, http });
        Ok((key, port))
    }

    /// Registers a freshly accepted socket.
    ///
    /// # Arguments
    /// * `stream` - the accepted socket.
    /// * `owner` - slab key of the listener.
    /// * `http` - whether the connection speaks HTTP.
    ///
    /// # Returns
    /// The connection's slab key.
    ///
    /// # Errors
    /// The registration error; the socket is dropped.
    pub fn adopt(&mut self, mut stream: TcpStream, owner: usize, http: bool) -> io::Result<usize> {
        // Disable Nagle: latency first.
        let _ = stream.set_nodelay(true);
        let slot = self.entries.vacant_entry();
        let key = slot.key();
        self.registry.register(&mut stream, Token(key), Interest::READABLE | Interest::WRITABLE)?;
        let flags = if http { flag::HTTP | flag::KEEP_ALIVE } else { 0 };
        slot.insert(Entry::Conn(Conn::new(stream, owner, flags)));
        Ok(key)
    }

    /// Reserves a slot for an outbound connection whose address is not known yet.
    ///
    /// # Returns
    /// The slab key JS will use for the socket.
    pub fn reserve(&mut self) -> usize {
        self.entries.insert(Entry::Resolving)
    }

    /// Starts the non-blocking connect for a reserved slot.
    ///
    /// # Arguments
    /// * `key` - slot from [`Io::reserve`].
    /// * `addr` - the resolved address, or why resolution failed.
    pub fn connect(&mut self, key: usize, addr: Result<SocketAddr, Fault>) {
        match self.entries.get(key) {
            Some(Entry::Resolving) => {}
            Some(Entry::Cancelled) => {
                self.entries.remove(key);
                return;
            }
            _ => return,
        }
        let registry = &self.registry;
        let opened = addr.and_then(|addr| {
            let fault = |e: io::Error| Fault::new(&e, "connect", &addr.to_string());
            let mut stream = TcpStream::connect(addr).map_err(fault)?;
            let _ = stream.set_nodelay(true);
            registry
                .register(&mut stream, Token(key), Interest::READABLE | Interest::WRITABLE)
                .map_err(fault)?;
            Ok(stream)
        });
        match opened {
            Ok(stream) => {
                self.entries[key] = Entry::Conn(Conn::new(stream, usize::MAX, flag::CONNECTING));
            }
            Err(fault) => self.close_notify(key, Some(fault)),
        }
    }

    /// Frees a slot without telling JS.
    ///
    /// Dropping the socket closes the descriptor, which also removes it from the
    /// kernel's poll set, so no `deregister` syscall is made.
    ///
    /// # Arguments
    /// * `key` - slab key; a vacant key is ignored.
    pub fn close(&mut self, key: usize) {
        self.entries.try_remove(key);
    }

    /// Closes a connection and queues the close event for JS.
    ///
    /// The slot stays reserved as [`Entry::Closed`] until the event is delivered.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `fault` - the error that caused the close, if any.
    pub fn close_notify(&mut self, key: usize, fault: Option<Fault>) {
        if let Some(entry) = self.entries.get_mut(key) {
            *entry = Entry::Closed;
            self.deferred.push_back(Deferred::Close(key, fault));
        }
    }

    /// Takes the next deferred event, freeing the tombstone of a delivered close.
    ///
    /// # Returns
    /// The next event, or `None` when the queue is empty.
    pub fn take_deferred(&mut self) -> Option<Deferred> {
        let next = self.deferred.pop_front()?;
        if let Deferred::Close(key, _) = next {
            if matches!(self.entries.get(key), Some(Entry::Closed)) {
                self.entries.remove(key);
            }
        }
        Some(next)
    }

    /// Destroys a handle on behalf of JS. JS emits its own `close`, so none is queued.
    ///
    /// # Arguments
    /// * `key` - slab key of a connection or pending connect.
    pub fn destroy(&mut self, key: usize) {
        match self.entries.get_mut(key) {
            Some(Entry::Conn(_)) => {
                self.entries.remove(key);
            }
            Some(entry @ Entry::Resolving) => *entry = Entry::Cancelled,
            _ => {}
        }
    }

    /// Closes a listener. Idle HTTP connections it accepted are closed with it; ones
    /// with a response in flight finish that response and then close.
    ///
    /// # Arguments
    /// * `key` - slab key of the listener.
    pub fn listener_close(&mut self, key: usize) {
        if !matches!(self.entries.get(key), Some(Entry::Listener { .. })) {
            return;
        }
        self.entries.remove(key);
        let mut idle = Vec::new();
        for (at, entry) in self.entries.iter_mut() {
            let Entry::Conn(conn) = entry else { continue };
            if conn.owner != key || conn.flags & flag::HTTP == 0 {
                continue;
            }
            conn.flags &= !flag::KEEP_ALIVE;
            if conn.flags & flag::BUSY == 0 {
                if conn.write_buf.is_empty() {
                    idle.push(at);
                } else {
                    conn.flags |= flag::ENDING;
                }
            }
        }
        for at in idle {
            self.entries.remove(at);
        }
    }

    // ------------------------------------------------------------ write side

    /// Handles a writable event: completes a pending connect and flushes queued bytes.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    ///
    /// # Returns
    /// What JS needs to hear about; see [`Wake`].
    pub fn writable(&mut self, key: usize) -> Wake {
        let Some(Entry::Conn(conn)) = self.entries.get_mut(key) else { return Wake::Quiet };
        let mut wake = Wake::Quiet;
        if conn.flags & flag::CONNECTING != 0 {
            let peer = match conn.stream.take_error() {
                Ok(None) => conn.stream.peer_addr(),
                Ok(Some(e)) | Err(e) => Err(e),
            };
            match peer {
                Ok(_) => {
                    conn.flags &= !flag::CONNECTING;
                    wake = Wake::Connected;
                }
                // Spurious wake-up: the connect is still in progress.
                Err(e)
                    if e.kind() == io::ErrorKind::NotConnected
                        || e.raw_os_error() == Some(libc::EINPROGRESS) =>
                {
                    return Wake::Quiet;
                }
                Err(e) => {
                    let fault = Fault::new(&e, "connect", "");
                    self.entries.remove(key);
                    return Wake::Failed(fault);
                }
            }
        }
        match conn.flush() {
            Ok(true) => self.finish_write(key),
            Ok(false) => {}
            Err(e) => self.fail(key, &e, "write"),
        }
        wake
    }

    /// Reports a broken connection: HTTP connections close silently (JS holds no
    /// handle for them), TCP sockets get a close event with the error.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `error` - the socket error.
    /// * `call` - the operation that failed.
    fn fail(&mut self, key: usize, error: &io::Error, call: &str) {
        match self.entries.get(key) {
            Some(Entry::Conn(conn)) if conn.flags & flag::HTTP != 0 => self.close(key),
            Some(Entry::Conn(_)) => self.close_notify(key, Some(Fault::new(error, call, ""))),
            _ => {}
        }
    }

    /// Runs the actions that wait for an empty write buffer: `drain`, and the close or
    /// half-close requested by `end()`.
    ///
    /// # Arguments
    /// * `key` - slab key of a connection whose write buffer is empty.
    pub fn finish_write(&mut self, key: usize) {
        let Some(Entry::Conn(conn)) = self.entries.get_mut(key) else { return };
        if conn.flags & flag::WANT_DRAIN != 0 {
            conn.flags &= !flag::WANT_DRAIN;
            self.deferred.push_back(Deferred::Drain(key));
        }
        if conn.flags & flag::ENDING == 0 {
            return;
        }
        if conn.flags & flag::HTTP != 0 {
            self.close(key);
            return;
        }
        if conn.flags & flag::WRITE_SHUT == 0 {
            let _ = conn.stream.shutdown(Shutdown::Write);
            conn.flags |= flag::WRITE_SHUT;
        }
        if conn.flags & flag::READ_EOF != 0 {
            self.close_notify(key, None);
        }
    }

    /// Records the peer's end of stream on a TCP socket.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    pub fn read_eof(&mut self, key: usize) {
        if let Some(Entry::Conn(conn)) = self.entries.get_mut(key) {
            conn.flags |= flag::READ_EOF;
        }
    }

    /// Closes a TCP socket once both directions are finished.
    ///
    /// # Arguments
    /// * `key` - slab key of a connection that has seen end of stream.
    pub fn finish_read(&mut self, key: usize) {
        if let Some(Entry::Conn(conn)) = self.entries.get(key) {
            if conn.flags & (flag::READ_EOF | flag::WRITE_SHUT) == flag::READ_EOF | flag::WRITE_SHUT
            {
                self.close_notify(key, None);
            }
        }
    }

    /// `socket.write`: sends bytes on a TCP socket.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `data` - bytes to send.
    ///
    /// # Returns
    /// False when the caller should wait for `drain` (more than [`HIGH_WATER`] bytes
    /// are queued) or the socket is gone.
    pub fn tcp_write(&mut self, key: usize, data: &[u8]) -> bool {
        let Some(Entry::Conn(conn)) = self.entries.get_mut(key) else { return false };
        if conn.flags & flag::ENDING != 0 {
            return false;
        }
        match conn.send([data, &[], &[]]) {
            Ok(()) if conn.write_buf.len() > HIGH_WATER => {
                conn.flags |= flag::WANT_DRAIN;
                false
            }
            Ok(()) => true,
            Err(e) => {
                self.fail(key, &e, "write");
                false
            }
        }
    }

    /// `socket.end`: half-closes a TCP socket after queued bytes are sent.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    pub fn tcp_end(&mut self, key: usize) {
        let Some(Entry::Conn(conn)) = self.entries.get_mut(key) else { return };
        conn.flags |= flag::ENDING;
        if conn.write_buf.is_empty() && conn.flags & flag::CONNECTING == 0 {
            self.finish_write(key);
        }
    }

    // ------------------------------------------------------------- HTTP side

    /// Refreshes the cached `date` line, reading the host's wall clock at most once
    /// per loop turn.
    ///
    /// # Arguments
    /// * `host` - provides the wall clock.
    fn refresh_date<H: Host>(&mut self, host: &H) {
        if !self.date_fresh {
            self.date_fresh = true;
            let secs = host.wall().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            if secs != self.date_secs {
                self.date_secs = secs;
                http::write_date(secs, &mut self.date);
            }
        }
    }

    /// Sends a complete response: head and body in one vectored write.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `status` - HTTP status code.
    /// * `fields` - serialized header fields from JS.
    /// * `body` - response body; ignored for `HEAD` and bodiless statuses.
    /// * `close` - whether JS asked for `connection: close`.
    /// * `host` - provides the wall clock for the `date` header.
    pub fn http_respond<H: Host>(
        &mut self,
        key: usize,
        status: u16,
        fields: &str,
        body: &[u8],
        close: bool,
        host: &H,
    ) {
        self.refresh_date(host);
        let Some(Entry::Conn(conn)) = self.entries.get_mut(key) else { return };
        if conn.flags & flag::BUSY == 0 {
            return;
        }
        if close {
            conn.flags &= !flag::KEEP_ALIVE;
        }
        let framing = if http::bodiless(status) { Framing::None } else { Framing::Length(body.len()) };
        let body = match framing {
            Framing::Length(_) if conn.flags & flag::HEAD_ONLY == 0 => body,
            _ => &[],
        };
        self.head.clear();
        let closing = conn.flags & flag::KEEP_ALIVE == 0;
        http::write_head(&mut self.head, status, fields, &self.date, framing, closing);
        let sent = conn.send([&self.head, body, &[]]);
        self.http_done(key, sent);
    }

    /// Starts a streamed response: sends the head and selects chunked framing
    /// (HTTP/1.1) or close-delimited framing (HTTP/1.0).
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `status` - HTTP status code.
    /// * `fields` - serialized header fields from JS.
    /// * `close` - whether JS asked for `connection: close`.
    /// * `host` - provides the wall clock for the `date` header.
    pub fn http_head<H: Host>(&mut self, key: usize, status: u16, fields: &str, close: bool, host: &H) {
        self.refresh_date(host);
        let Some(Entry::Conn(conn)) = self.entries.get_mut(key) else { return };
        if conn.flags & flag::BUSY == 0 {
            return;
        }
        if close || conn.flags & flag::V10 != 0 {
            conn.flags &= !flag::KEEP_ALIVE;
        }
        let framing = if http::bodiless(status) {
            conn.flags |= flag::HEAD_ONLY;
            Framing::None
        } else if conn.flags & flag::V10 != 0 {
            Framing::Close
        } else {
            conn.flags |= flag::CHUNKED_OUT;
            Framing::Chunked
        };
        self.head.clear();
        let closing = conn.flags & flag::KEEP_ALIVE == 0;
        http::write_head(&mut self.head, status, fields, &self.date, framing, closing);
        if conn.send([&self.head, &[], &[]]).is_err() {
            self.close(key);
        }
    }

    /// Sends one piece of a streamed response body.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `data` - body bytes; an empty piece is skipped so it cannot be mistaken for
    ///   the terminating chunk.
    pub fn http_chunk(&mut self, key: usize, data: &[u8]) {
        let Some(Entry::Conn(conn)) = self.entries.get_mut(key) else { return };
        if conn.flags & flag::BUSY == 0 || conn.flags & flag::HEAD_ONLY != 0 || data.is_empty() {
            return;
        }
        let sent = if conn.flags & flag::CHUNKED_OUT != 0 {
            let mut line = [0u8; 18];
            let n = http::chunk_line(&mut line, data.len());
            conn.send([&line[..n], data, b"\r\n"])
        } else {
            conn.send([data, &[], &[]])
        };
        if sent.is_err() {
            self.close(key);
        }
    }

    /// Finishes a streamed response.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    pub fn http_end(&mut self, key: usize) {
        let Some(Entry::Conn(conn)) = self.entries.get_mut(key) else { return };
        if conn.flags & flag::BUSY == 0 {
            return;
        }
        let chunked = conn.flags & (flag::CHUNKED_OUT | flag::HEAD_ONLY) == flag::CHUNKED_OUT;
        let sent = if chunked { conn.send([b"0\r\n\r\n", &[], &[]]) } else { Ok(()) };
        self.http_done(key, sent);
    }

    /// Completes the exchange on an HTTP connection: closes it, or makes it ready for
    /// the next request.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `sent` - result of sending the last part of the response.
    fn http_done(&mut self, key: usize, sent: io::Result<()>) {
        let Some(Entry::Conn(conn)) = self.entries.get_mut(key) else { return };
        conn.flags &= !(flag::BUSY | flag::HEAD_ONLY | flag::CHUNKED_OUT);
        if sent.is_err() {
            self.close(key);
        } else if conn.flags & flag::KEEP_ALIVE == 0 {
            conn.flags |= flag::ENDING;
            if conn.write_buf.is_empty() {
                self.close(key);
            }
        } else if conn.flags & flag::IN_PUMP == 0
            && (!conn.read_buf.is_empty() || conn.flags & flag::READABLE != 0)
        {
            // The response finished outside the pump (async handler) and more input
            // may be waiting: pipelined requests, or bytes left unread while busy.
            self.deferred.push_back(Deferred::Resume(key));
        }
    }

    /// Answers a malformed or oversized request and closes the connection.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `status` - HTTP status code to answer with.
    /// * `host` - provides the wall clock for the `date` header.
    pub fn http_reject<H: Host>(&mut self, key: usize, status: u16, host: &H) {
        self.refresh_date(host);
        let Some(Entry::Conn(conn)) = self.entries.get_mut(key) else { return };
        self.head.clear();
        http::write_head(&mut self.head, status, "", &self.date, Framing::Length(0), true);
        conn.flags = conn.flags & !flag::CORKED | flag::ENDING;
        let drained = conn.send([&self.head, &[], &[]]).and_then(|()| conn.flush());
        if !matches!(drained, Ok(false)) {
            self.close(key);
        }
    }

    /// Ends response batching on a connection and flushes what was batched.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    pub fn uncork(&mut self, key: usize) {
        let Some(Entry::Conn(conn)) = self.entries.get_mut(key) else { return };
        if conn.flags & flag::CORKED == 0 {
            return;
        }
        conn.flags &= !flag::CORKED;
        match conn.flush() {
            Ok(true) => self.finish_write(key),
            Ok(false) => {}
            Err(_) => self.close(key),
        }
    }
}
