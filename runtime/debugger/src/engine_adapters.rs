//! Concrete implementations of the shared engine contract.

/// Direct native QuickJS-NG integration, available with the `quickjs-ng` feature.
#[cfg(feature = "quickjs-ng")]
#[path = "engine_adapters/quickjs_ng_debug_engine.rs"]
pub mod quickjs_ng;

#[path = "engine_adapters/engine_registry.rs"]
mod registry;
pub use registry::{EngineFactory, EngineLimits, EngineRegistry};
