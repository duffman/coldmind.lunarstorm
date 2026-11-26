//! The values that cross the host boundary of a [`crate::Runtime`]: the entry
//! [`Source`], call arguments and results ([`Arg`], [`CallId`]), how a step waits
//! ([`Wait`]) and what it found ([`State`]), the cross-thread [`Poster`], and the
//! event codes the loop passes to the native dispatch ([`ev`]).

use crossbeam_channel::Sender;
use lunarstorm_engine_contract::Vm;
use mio::Waker;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Event codes passed to the native `dispatch` ([`node::dispatch`]), which
/// imports this table.
pub(crate) mod ev {
    /// `(key, one_shot)`: a timer fired.
    pub const TIMER: i32 = 0;
    /// `()`: run one `setImmediate` callback; JS returns whether more remain.
    pub const IMMEDIATE: i32 = 1;
    /// `(listener, conn, address, port)`: a TCP connection was accepted.
    pub const CONNECTION: i32 = 2;
    /// `(conn, chunk)`: bytes arrived on a TCP socket.
    pub const DATA: i32 = 3;
    /// `(conn)`: the peer finished sending.
    pub const END: i32 = 4;
    /// `(conn, message?, code?)`: a TCP socket closed, with the error if any.
    pub const CLOSE: i32 = 5;
    /// `(conn)`: a TCP socket's write buffer drained.
    pub const DRAIN: i32 = 6;
    /// `(conn)`: an outbound connect completed.
    pub const CONNECT: i32 = 7;
    /// `(listener, conn, method, url, fields, body)`: an HTTP request is complete.
    pub const HTTP: i32 = 8;
    /// `(request, value, message?, code?)`: a filesystem job finished.
    pub const FS: i32 = 9;
}

/// What [`Runtime::load`] loads as the entry module.
// Decision: `Package` stays unboxed. A `Source` is built once per load and moved
// into it, so its size costs nothing; boxing would only add a `Box::new` at every
// caller.
#[allow(clippy::large_enum_variant)]
pub enum Source {
    /// An ES module file.
    File(PathBuf),
    /// Source text.
    Text {
        /// Module name; relative imports resolve against the host's working
        /// directory joined with it (`[eval]` for a command line's `-e`).
        name: String,
        /// The module's source.
        code: String,
    },
    /// A verified `.jbn` package, from [`lunarstorm_jbn_package_format::Package::open`]: its entry
    /// module runs, and its modules resolve and load only from the package.
    Package(lunarstorm_jbn_package_format::Package),
}

/// The engine identity a package records (`adapter engine version contract`);
/// a package runs only on the same identity, since bytecode is engine-specific.
///
/// # Returns
/// The identity of `V`.
pub fn engine<V: Vm>() -> String {
    let d = V::descriptor();
    format!("{} {} {} contract {}", d.adapter, d.engine, d.engine_version, d.contract)
}

/// A value that crosses the host boundary: an argument to [`Runtime::call`] or the
/// value its promise settled with.
///
/// Decision: the host interface carries no engine handles. Objects an export
/// returns arrive as JSON [`Arg::Text`]; a host that needs live objects registers
/// bindings, which work on the VM's handles directly.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    /// `null`; also what `undefined` settles as.
    Null,
    /// A boolean.
    Bool(bool),
    /// A number.
    Number(f64),
    /// A string.
    Text(String),
    /// A `Uint8Array`.
    Bytes(Vec<u8>),
}

/// Identifies one [`Runtime::call`] until its result is collected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallId(pub(crate) u32);

/// How [`Runtime::poll`] waits for readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Sleep until an event, a timer or a posted task. For hosts whose loop is the
    /// Runtime's.
    Block,
    /// Take what is ready and return. For hosts that interleave their own loop.
    NoBlock,
}

/// What a loop step found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The step ran; something can still produce an event.
    Running,
    /// Nothing is open, armed or queued; only `call` or `post` can create work.
    Idle,
}

/// A unit of work posted to the loop thread. It receives the context handle.
pub(crate) type Task<V> = Box<dyn for<'s> FnOnce(&<V as Vm>::Scope<'s>) + Send>;

/// Posts tasks to a Runtime's loop from any thread.
///
/// Decision: the Runtime itself is single-threaded and cannot be shared; this
/// handle is the one part of it that is `Send`, and it holds nothing but a channel
/// and the loop's waker.
pub struct Poster<V: Vm> {
    /// Queue drained by the loop.
    pub(crate) tasks: Sender<Task<V>>,
    /// Wakes the loop out of `poll`.
    pub(crate) waker: Arc<Waker>,
    /// Whether a wake is already on its way; coalesces a burst into one syscall.
    pub(crate) pending: Arc<AtomicBool>,
}

impl<V: Vm> Clone for Poster<V> {
    /// # Returns
    /// Another handle to the same loop.
    fn clone(&self) -> Self {
        Self { tasks: self.tasks.clone(), waker: self.waker.clone(), pending: self.pending.clone() }
    }
}

impl<V: Vm> Poster<V> {
    /// Queues a task for the loop thread and wakes the loop.
    ///
    /// # Arguments
    /// * `task` - runs on the loop thread with the context handle; promise jobs it
    ///   queues are drained right after it.
    ///
    /// # Returns
    /// `false` when the Runtime no longer exists and the task was dropped.
    pub fn post(&self, task: impl for<'s> FnOnce(&V::Scope<'s>) + Send + 'static) -> bool {
        if self.tasks.send(Box::new(task)).is_err() {
            return false;
        }
        if !self.pending.swap(true, Ordering::AcqRel) {
            let _ = self.waker.wake();
        }
        true
    }
}
