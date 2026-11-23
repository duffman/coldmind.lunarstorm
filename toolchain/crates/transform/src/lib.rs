//! Shared Devtools source transformations, with optional Bun native bindings.
//!
//! `devtools_transform` is a C-ABI symbol Bun calls on any bundler thread, concurrently. Its
//! configuration, counters and warnings live in a tagged napi external created by `createState`
//! and passed as the `external` of `build.onBeforeParse(...)`.
//!
//! Stages: replace (literal text), define (scope-aware constants) and inject (cminject `$inject`
//! from constructor types). Define and inject share one oxc parse.

//! By default this is a regular Rust transform library; enable the `bun` feature
//! only for the native Bun adapter:
//! no Bun/Node-API bindings, runtime or HTTP server. Use [`Transformer::new`]
//! and [`Transformer::transform_source`] from a Rust or Tauri host.

#[cfg(feature = "bun")]
use bun_native_plugin::define_bun_plugin;
#[cfg(feature = "bun")]
use napi_derive::napi;

mod bun_hook;
mod constant_defines;
mod dependency_injection;
#[cfg(feature = "bun")]
mod native_state;
mod source_edits;
mod text_replacements;
mod transformer;

pub use bun_hook::Loader;
pub use transformer::{
    DefinePair, ReplacePair, TransformOptions, TransformOutput, TransformState as Transformer,
    TransformStats,
};

#[cfg(feature = "bun")]
define_bun_plugin!("devtools");
