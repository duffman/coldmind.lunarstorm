//! The Lunarstorm debugger.
//!
//! Taken from jetbunny (`coldmind.jetbunny`, `src/debug` and the parts of
//! `src/engine` it uses) with its module layout kept, so the code is unchanged:
//! [`debug`] is the coordinator, DAP translator, source registry and source maps;
//! [`engine`] holds the engine-neutral types they speak in (`JsString`,
//! `SourceDigest`, `FailureReport`, identities).
//!
//! An engine adapter implements [`debug::DebugBackend`] on its native hooks.
//!
//! Decision: file names say what each file holds (`debug_session/pause_coordinator.rs`,
//! `engine_adapters/quickjs_ng_debug_engine/native_debugger_pump.rs`); module names
//! stay jetbunny's (`debug`, `engine`, `engines::quickjs_ng`, `vm`), bound to the files
//! by `#[path]`. The code and jetbunny's tests (`coldmind_runtime::debug::...`) stay
//! verbatim, so upstream changes still apply as they are.

// jetbunny's tests import its crate by name.
#[cfg(test)]
extern crate self as coldmind_runtime;

#[path = "debug_session.rs"]
pub mod debug;
#[path = "engine_neutral_types.rs"]
pub mod engine;
#[path = "engine_adapters.rs"]
pub mod engines;

/// The handle the debug target registry keeps per VM; Lunarstorm's host builds it
/// from its Runtime's interrupt.
#[cfg(not(test))]
#[path = "debug_target_vm_handle.rs"]
pub mod vm;

// lunarstorm patch: jetbunny's own VM (`VirtualMachine`, its loop transport, module
// graph and program runner), verbatim from jetbunny ced69b3, compiled only into this
// crate's unit tests. jetbunny's tests (tests/*.rs, tests/unit/*.rs) drive the engine
// through it; Lunarstorm runs programs through `lunarstorm-engine-contract`, not through it.
// Decision: the test build swaps `vm` for jetbunny's at jetbunny's paths, so its text
// and the tests' `coldmind_runtime::vm::*` resolve unchanged. jetbunny's `VmHandle`
// has the same `runtime_epoch` / `request_stop` the registry calls (debug/targets.rs,
// identical in both trees).
#[cfg(test)]
#[path = "owner_thread_vm_facade.rs"]
pub mod vm;
#[cfg(test)]
#[path = "index_arena.rs"]
mod arena;
#[cfg(test)]
#[path = "module_identity_and_graphs.rs"]
pub mod modules;
#[cfg(test)]
#[doc(hidden)]
#[path = "owner_thread_socket_reactor.rs"]
pub mod owner_io;
#[cfg(test)]
#[path = "admitted_program_handoff.rs"]
pub mod program;
#[cfg(test)]
#[path = "application_lanes.rs"]
pub mod transport;
#[cfg(test)]
#[path = "scheduling_budgets.rs"]
mod config;
#[cfg(test)]
#[path = "owner_thread_callbacks.rs"]
mod driver;
#[cfg(test)]
#[path = "owner_admission_controls.rs"]
mod host;
#[cfg(test)]
#[path = "ready_item_queue.rs"]
mod ready;
#[cfg(test)]
#[path = "owner_thread_pump.rs"]
mod runner;
#[cfg(test)]
#[path = "monotonic_timer_schedule.rs"]
mod timers;
#[cfg(test)]
#[path = "dev_tracing.rs"]
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

// lunarstorm patch: jetbunny's integration tests (tests/*.rs), verbatim. jetbunny
// builds each as a test target of its own crate; here they are unit-test modules,
// so `use coldmind_runtime::...` resolves through `extern crate self`. Not
// mounted (they need jetbunny modules Lunarstorm does not take, or jetbunny's
// binary): builtin_worker, debugger_process, embed, embedded_builtins, jbn,
// jbn_cli, module_vm_contract, package_manager, program_boundary,
// quickjs_ng_conformance, quickjs_ng_debug_workers, quickjs_ng_jbn,
// quickjs_ng_module_vm, source_cache, source_packages, url_imports.
// Also not mounted: compiler_boundary, compiler_conformance and compiler_contract,
// which run jetbunny's repository scripts (scripts/compiler-*.sh, .py) over
// jetbunny's directory layout (crates/, vendor/, tests/compiler-contract).
// quickjs_ng_copy_alloc installs its own global allocator, so it is its own test
// target (tests/jetbunny_copy_alloc.rs), as in jetbunny.
#[cfg(test)]
#[path = "../tests/bridge.rs"]
mod jetbunny_bridge;
#[cfg(test)]
#[path = "../tests/bridge_failures.rs"]
mod jetbunny_bridge_failures;
#[cfg(test)]
#[path = "../tests/busy_stop.rs"]
mod jetbunny_busy_stop;
#[cfg(test)]
#[path = "../tests/compiler_build_identity.rs"]
mod jetbunny_compiler_build_identity;
#[cfg(test)]
#[path = "../tests/contract_shapes.rs"]
mod jetbunny_contract_shapes;
#[cfg(test)]
#[path = "../tests/debug_contract.rs"]
mod jetbunny_debug_contract;
#[cfg(test)]
#[path = "../tests/debug_coverage.rs"]
mod jetbunny_debug_coverage;
#[cfg(test)]
#[path = "../tests/debug_identities.rs"]
mod jetbunny_debug_identities;
#[cfg(test)]
#[path = "../tests/debug_sources.rs"]
mod jetbunny_debug_sources;
#[cfg(test)]
#[path = "../tests/engine_contract.rs"]
mod jetbunny_engine_contract;
#[cfg(test)]
#[path = "../tests/engine_coverage.rs"]
mod jetbunny_engine_coverage;
#[cfg(test)]
#[path = "../tests/engine_registry.rs"]
mod jetbunny_engine_registry;
#[cfg(test)]
#[path = "../tests/identity.rs"]
mod jetbunny_identity;
#[cfg(test)]
#[path = "../tests/module_images.rs"]
mod jetbunny_module_images;
#[cfg(test)]
#[path = "../tests/module_providers.rs"]
mod jetbunny_module_providers;
#[cfg(test)]
#[path = "../tests/ordering.rs"]
mod jetbunny_ordering;
#[cfg(test)]
#[path = "../tests/quickjs_ng.rs"]
mod jetbunny_quickjs_ng;
#[cfg(test)]
#[path = "../tests/quickjs_ng_artifacts.rs"]
mod jetbunny_quickjs_ng_artifacts;
#[cfg(test)]
#[path = "../tests/quickjs_ng_async.rs"]
mod jetbunny_quickjs_ng_async;
#[cfg(test)]
#[path = "../tests/quickjs_ng_callbacks.rs"]
mod jetbunny_quickjs_ng_callbacks;
#[cfg(test)]
#[path = "../tests/quickjs_ng_data.rs"]
mod jetbunny_quickjs_ng_data;
#[cfg(test)]
#[path = "../tests/quickjs_ng_debug.rs"]
mod jetbunny_quickjs_ng_debug;
#[cfg(test)]
#[path = "../tests/quickjs_ng_debug_async.rs"]
mod jetbunny_quickjs_ng_debug_async;
#[cfg(test)]
#[path = "../tests/quickjs_ng_debug_breakpoints.rs"]
mod jetbunny_quickjs_ng_debug_breakpoints;
#[cfg(test)]
#[path = "../tests/quickjs_ng_debug_editing.rs"]
mod jetbunny_quickjs_ng_debug_editing;
#[cfg(test)]
#[path = "../tests/quickjs_ng_debug_evaluation.rs"]
mod jetbunny_quickjs_ng_debug_evaluation;
#[cfg(test)]
#[path = "../tests/quickjs_ng_debug_inspection.rs"]
mod jetbunny_quickjs_ng_debug_inspection;
#[cfg(test)]
#[path = "../tests/quickjs_ng_debug_locations.rs"]
mod jetbunny_quickjs_ng_debug_locations;
#[cfg(test)]
#[path = "../tests/quickjs_ng_debug_lookup.rs"]
mod jetbunny_quickjs_ng_debug_lookup;
#[cfg(test)]
#[path = "../tests/quickjs_ng_debug_stepping.rs"]
mod jetbunny_quickjs_ng_debug_stepping;
#[cfg(test)]
#[path = "../tests/quickjs_ng_jobs.rs"]
mod jetbunny_quickjs_ng_jobs;
#[cfg(test)]
#[path = "../tests/quickjs_ng_lifecycle.rs"]
mod jetbunny_quickjs_ng_lifecycle;
#[cfg(test)]
#[path = "../tests/quickjs_ng_modules.rs"]
mod jetbunny_quickjs_ng_modules;
#[cfg(test)]
#[path = "../tests/quickjs_ng_vm.rs"]
mod jetbunny_quickjs_ng_vm;
#[cfg(test)]
#[path = "../tests/scheduler_coverage.rs"]
mod jetbunny_scheduler_coverage;
#[cfg(test)]
#[path = "../tests/shutdown.rs"]
mod jetbunny_shutdown;
#[cfg(test)]
#[path = "../tests/timers.rs"]
mod jetbunny_timers;
#[cfg(test)]
#[path = "../tests/transport.rs"]
mod jetbunny_transport;
#[cfg(test)]
#[path = "../tests/virtual_machine.rs"]
mod jetbunny_virtual_machine;
#[cfg(test)]
#[path = "../tests/vm_debug.rs"]
mod jetbunny_vm_debug;
