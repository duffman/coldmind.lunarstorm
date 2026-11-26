//! The cli product's project types, registered with `lunarstorm-project`.
//!
//! | type | actions | `create` writes |
//! |------|---------|-----------------|
//! | `program` | `create` | `src/main.ts`, a program `lunar` runs |
//! | `module`  | `create` | `src/index.ts`, a module other code imports |
//!
//! Each type's files live in the folder named after it; `build.rs` embeds them, so
//! editing a type's files needs no Rust change.

use lunarstorm_project::{create, Type};

include!(concat!(env!("OUT_DIR"), "/files.rs"));

/// Registers the cli types; the binary passes this function to
/// `lunarstorm_project::package`.
///
/// # Returns
/// `program` and `module`, each targeting `lunar` or `node`.
pub fn types() -> Vec<Type> {
    vec![
        Type::new("program", "a program lunar runs").action(create("program", "src/main.ts", &["lunar", "node"], FILES)),
        Type::new("module", "a module other code imports").action(create("module", "src/index.ts", &["lunar", "node"], FILES)),
    ]
}
