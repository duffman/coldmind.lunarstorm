//! The desktop product's project types, registered with `lunarstorm-project`.
//!
//! | type | actions | `create` writes |
//! |------|---------|-----------------|
//! | `app` | `create` | a forms project `lunar build` builds: `src/app.tsx`, one form view and its form class |
//!
//! Each type's files live in the folder named after it; `build.rs` embeds them.
//! `app/` is also the starter the model and `lunar build` tests build as it stands,
//! so its source files carry no `{{name}}` placeholder outside `README.md`.

use lunarstorm_project::{create, Type};

include!(concat!(env!("OUT_DIR"), "/files.rs"));

/// Registers the desktop types; the binary passes this function to
/// `lunarstorm_project::package`.
///
/// # Returns
/// `app`, targeting `lunar` only: a desktop app runs on lunar.
pub fn types() -> Vec<Type> {
    vec![Type::new("app", "a desktop app: one window with a form, built by lunar build").action(create("app", "src/app.tsx", &["lunar"], FILES))]
}
