//! The Lunarstorm debugger.
//!
//! Taken from jetbunny (`coldmind.jetbunny`, `src/debug` and the parts of
//! `src/engine` it uses) with its module layout kept, so the code is unchanged:
//! [`debug`] is the coordinator, DAP translator, source registry and source maps;
//! [`engine`] holds the engine-neutral types they speak in (`JsString`,
//! `SourceDigest`, `FailureReport`, identities).
//!
//! An engine adapter implements [`debug::DebugBackend`] on its native hooks.

// jetbunny's tests import its crate by name.
#[cfg(test)]
extern crate self as coldmind_runtime;

pub mod debug;
pub mod engine;
pub mod engines;

/// The handle the debug target registry keeps per VM; Lunarstorm's host builds it
/// from its Runtime's interrupt.
#[cfg(not(test))]
#[path = "vm.rs"]
pub mod vm;

// lunarstorm patch: jetbunny's own VM (`VirtualMachine`, its loop transport, module
// graph and program runner), verbatim from jetbunny ced69b3, compiled only into this
// crate's unit tests. jetbunny's tests (tests/*.rs, tests/unit/*.rs) drive the engine
// through it; Lunarstorm runs programs through `lunarstorm-vm`, not through it.
// Decision: the test build swaps `vm` for jetbunny's at jetbunny's paths, so its text
// and the tests' `coldmind_runtime::vm::*` resolve unchanged. jetbunny's `VmHandle`
// has the same `runtime_epoch` / `request_stop` the registry calls (debug/targets.rs,
// identical in both trees).
#[cfg(test)]
#[path = "vm/mod.rs"]
pub mod vm;
#[cfg(test)]
mod arena;
#[cfg(test)]
pub mod modules;
#[cfg(test)]
#[doc(hidden)]
pub mod owner_io;
#[cfg(test)]
pub mod program;
#[cfg(test)]
pub mod transport;
#[cfg(test)]
mod config;
#[cfg(test)]
mod driver;
#[cfg(test)]
mod host;
#[cfg(test)]
mod ready;
#[cfg(test)]
mod runner;
#[cfg(test)]
mod timers;
#[cfg(test)]
pub mod trace;
/// Owner-thread scheduling, native TCP readiness, and bounded producer transport
/// (jetbunny's, test build only; see `vm`).
#[cfg(test)]
pub mod scheduler {
    pub use crate::{
        config::SchedulerConfig,
        driver::LoopDriver,
        host::{Host, LocalHost, TurnOutcome},
        runner::{EventLoop, LoopStats, LoopTick, ReportSink, ScriptErrorPolicy},
        timers::{Clock, MonotonicClock},
        transport::{LoopHandle, TransportStats},
    };
}
