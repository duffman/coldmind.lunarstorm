//! Host services: everything the Runtime needs from the world outside it.
//!
//! A host owns a Runtime and implements this trait; the Runtime calls up through
//! it and touches the process directly nowhere else. A CLI host answers with the
//! process's argv, stdio and exit; a desktop host answers with its own; a test
//! host answers with buffers.
//!
//! Decisions:
//! - Methods take `&self`. The Runtime calls them from inside native functions
//!   while other host calls may be in flight, so a host that keeps state uses
//!   interior mutability.
//! - The trait is a generic parameter of the Runtime, never a trait object: a
//!   host's answers are inlined into the loop.
//! - There is one fs and one net implementation, the Runtime's. The host does not
//!   replace them; it decides what they may touch ([`Host::allow`]).
//! - `exit` does not return. `process.exit()` must not run another line of JS, and
//!   unwinding the engine from inside a native call is not something every engine
//!   supports; a host that must survive the exit runs the Runtime on a thread it
//!   can abandon.

use std::net::SocketAddr;
use std::time::{Instant, SystemTime};

/// One of the two standard output streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    /// Standard output.
    Out,
    /// Standard error.
    Err,
}

/// Something the Runtime is about to do on a script's behalf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access<'a> {
    /// Read a file, list a directory or stat a path. Also module files.
    Read(&'a str),
    /// Create, change or remove a path.
    Write(&'a str),
    /// Accept connections on an address.
    Listen(SocketAddr),
    /// Connect to a host and port.
    Connect(&'a str, u16),
}

/// A record for the host's log sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Log<'a> {
    /// The Runtime recovered from something (a failed `accept`).
    Warning(&'a str),
    /// The Runtime cannot continue; `exit(1)` follows.
    Error(&'a str),
    /// A script error nobody caught. What follows is [`Host::uncaught`]'s choice;
    /// by default `exit(1)`, as in Node.
    Uncaught {
        /// Whether it was a rejected promise rather than a thrown exception.
        in_promise: bool,
        /// Name, message and stack as the script's `console` would show them.
        description: &'a str,
    },
}

/// The services a host provides to the Runtime it owns.
pub trait Host: 'static {
    /// Whether the Runtime publishes its pulse ([`crate::Runtime::pulse`]): a
    /// counter that is odd while a script is running. A host with an execution
    /// budget reads it from a watchdog thread and interrupts a script that stays
    /// in one entry too long.
    ///
    /// Decision: a constant of the host type, `false` by default, so the two
    /// counter writes per JS entry are compiled out of a host that has no budget
    /// (the CLI host) instead of being paid on every event.
    const PULSE: bool = false;

    /// # Returns
    /// `process.argv`: by Node's convention the runtime's name, the script's name,
    /// then the script's arguments.
    fn args(&self) -> Vec<String>;

    /// # Returns
    /// `process.env` as name/value pairs. Read once, on first use.
    fn environment(&self) -> Vec<(String, String)>;

    /// # Returns
    /// The working directory relative paths and `[eval]` code resolve against.
    fn working_directory(&self) -> String;

    /// Writes to a standard stream. The Runtime buffers stdout itself and calls
    /// this once per loop turn with everything printed in it; stderr is unbuffered.
    ///
    /// # Arguments
    /// * `stream` - which stream.
    /// * `bytes` - the bytes to write.
    fn write(&self, stream: Stream, bytes: &[u8]);

    /// Ends the program. Stdout has been flushed through [`Host::write`] already.
    ///
    /// # Arguments
    /// * `code` - the exit status.
    fn exit(&self, code: i32) -> !;

    /// The fs/net policy: decides whether a script may do something.
    ///
    /// # Arguments
    /// * `access` - what is about to happen.
    ///
    /// # Returns
    /// `false` to refuse; the script then sees an `EACCES` error.
    fn allow(&self, access: Access<'_>) -> bool;

    /// # Returns
    /// The monotonic clock that drives timers and `performance.now()`.
    #[inline]
    fn monotonic(&self) -> Instant {
        Instant::now()
    }

    /// # Returns
    /// The wall clock, used for the HTTP `date` header.
    #[inline]
    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }

    /// Fills a buffer with cryptographically secure random bytes
    /// (`crypto.getRandomValues`).
    ///
    /// # Arguments
    /// * `into` - the buffer to fill completely.
    fn entropy(&self, into: &mut [u8]);

    /// The log sink: receives what the Runtime has to say that is not script output.
    ///
    /// # Arguments
    /// * `record` - the record.
    fn log(&self, record: Log<'_>);

    /// Decides what an uncaught script error does: an exception thrown out of a
    /// callback, or a promise rejection nobody handled.
    ///
    /// The default is Node's: the error goes to the log sink and the program
    /// exits with status 1. A host that outlives its scripts' mistakes (a desktop
    /// application) reports the error itself and returns; the loop then goes on
    /// with the next event.
    ///
    /// Decision: this is a method of the host, not a flag on the Runtime, so the
    /// policy costs nothing on the event path: it is only reached once an error
    /// has already left the script.
    ///
    /// # Arguments
    /// * `in_promise` - whether it is a rejected promise rather than a thrown
    ///   exception.
    /// * `description` - the error as the script's `console` would show it.
    fn uncaught(&self, in_promise: bool, description: &str) {
        self.log(Log::Uncaught { in_promise, description });
        self.exit(1)
    }
}
