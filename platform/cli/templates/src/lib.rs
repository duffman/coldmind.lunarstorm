//! The cli product's project templates for `lunar project init`.
//!
//! | template | writes |
//! |----------|--------|
//! | `program` | `src/main.ts`, a program `lunar` runs |
//! | `module`  | `src/index.ts`, a module other code imports |
//!
//! Each folder of this crate holding a `template.json` is one template; the format
//! is documented in `lunarstorm-project` (`platform/project/README.md`). `build.rs`
//! embeds the folders, so editing a template needs no Rust change.

include!(concat!(env!("OUT_DIR"), "/files.rs"));
