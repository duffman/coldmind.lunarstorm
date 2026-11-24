//! The Lunarstorm Runtime.
//!
//! ```text
//! HOST            owns the Runtime (calls down)      implements host services (calls up)
//!                 start, load, stop                  stdio, args, exit,
//!                 call(export, args) -> promise      fs/net policy,
//!                 post(task) from any thread         clock, entropy, environment,
//!                 poll() one loop step               log sink
//!                 register(bindings)
//! RUNTIME         event loop, timers, module loader, fs and net under host policy,
//!                 plus the bindings the host registered; generic over the VM
//! VM              `lunarstorm-vm`: the contract an engine adapter implements
//! ```
//!
//! The rule at both boundaries: this crate depends only on contracts. It names no
//! host type and no engine type; [`Runtime<V, H>`] is generic over a
//! [`lunarstorm_vm::Vm`] and a [`Host`], both bound at compile time, so every call
//! across either boundary is a direct (and usually inlined) call.
//!
//! | module     | role                                                               |
//! |------------|--------------------------------------------------------------------|
//! | `runtime`  | the Runtime interface and the event loop                           |
//! | `host`     | the host services contract                                         |
//! | `bindings` | host-supplied native functions and modules                         |
//! | `ops`      | the Runtime's own native functions                                 |
//! | `modules`  | module resolver and loader; the VM callbacks                       |
//! | `io`       | loop-owned state: sockets, write buffers, timers, deferred work    |
//! | `pool`     | bounded worker pool for blocking work (fs, DNS)                    |
//! | `fs`       | blocking filesystem operations, run inline or on the pool          |
//! | `channel`  | bounded byte queue between threads: `Inbox`, the host's side       |
//! | `ipc`      | `lunarstorm:ipc`, the program's byte channel to its host           |
//! | `worker`   | `lunarstorm:worker`, programs on their own threads and VMs         |
//!
//! HTTP/1.1 request framing and response-head serialization is the separate crate
//! `lunarstorm-http1` (`crates/http1`): it has no sockets and no JS, so the rest of
//! the monorepo can use it without the Runtime.

mod bindings;
mod channel;
mod fs;
mod host;
mod io;
mod ipc;
mod modules;
mod ops;
mod pool;
mod runtime;
mod worker;

pub use bindings::Bindings;
pub use channel::{Inbox, MAX_BYTES, MAX_MESSAGES, MAX_MESSAGE_BYTES};
pub use host::{Access, Host, Log, Stream};
pub use lunarstorm_vm as vm;
pub use runtime::{Arg, CallId, Poster, Runtime, Source, State, Wait};
