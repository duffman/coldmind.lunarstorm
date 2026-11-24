//! The Lunarstorm debugger.
//!
//! Taken from jetbunny (`coldmind.jetbunny`, `src/debug` and the parts of
//! `src/engine` it uses) with its module layout kept, so the code is unchanged:
//! [`debug`] is the coordinator, DAP translator, source registry and source maps;
//! [`engine`] holds the engine-neutral types they speak in (`JsString`,
//! `SourceDigest`, `FailureReport`, identities).
//!
//! Left in jetbunny: `debug/host.rs`, `debug/targets.rs` and `debug/target_dap.rs`,
//! which drive jetbunny's own VM threads. An engine adapter implements
//! [`debug::DebugBackend`] on its native hooks.

// jetbunny's tests import its crate by name.
#[cfg(test)]
extern crate self as coldmind_runtime;

pub mod debug;
pub mod engine;
pub mod engines;
pub mod vm;
