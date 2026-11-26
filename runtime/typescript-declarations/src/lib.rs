//! The `.d.ts` emitter: TypeScript declarations from the Runtime's [`ModuleDecl`]s.
//!
//! The emitter is [`dts_emitter`]; this root re-exports it.

mod dts_emitter;

pub use dts_emitter::*;
