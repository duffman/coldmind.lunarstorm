//! Concrete implementations of the shared engine contract.

/// Direct native QuickJS-NG integration, available with the `quickjs-ng` feature.
#[cfg(feature = "quickjs-ng")]
pub mod quickjs_ng;

mod registry;
pub use registry::{EngineFactory, EngineLimits, EngineRegistry};
