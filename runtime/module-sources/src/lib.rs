//! Module sources for the Lunarstorm Runtime: every file a script can import
//! becomes an ES module the VM can compile, and every specifier resolves to a file.
//!
//! | module      | role                                                              |
//! |-------------|-------------------------------------------------------------------|
//! | `resolve`   | [`Modules`]: resolution and loading on the real filesystem        |
//! | `package`   | `package.json`: `type`, `main`, `exports`, `imports`, conditions   |
//! | `transform` | TypeScript stripping, JSON modules, the CommonJS body and facade  |
//!
//! | file                         | becomes                                               |
//! |------------------------------|-------------------------------------------------------|
//! | `.mjs`                       | itself                                                |
//! | `.js`, no `type`/no manifest | itself with module syntax, else CommonJS              |
//! | `.js` under `"module"`       | itself                                                |
//! | `.cjs`, `.js` under CommonJS | its body; `import` reaches a facade (`<file>#esm`)    |
//! | `.json`                      | `export default` of its value                         |
//! | `.ts`, `.mts`, `.cts`        | types stripped, then as `.js`, `.mjs`, `.cjs`         |
//!
//! Ported from jetbunny's source packages (`src/platform/package/`) and adapted to
//! the Runtime's loader: real paths under the host's read policy instead of a
//! frozen snapshot, Node's `node_modules` lookup instead of a declared closure.

mod package_json_exports;
mod filesystem_specifier_resolution;
pub mod load_time_esm_transforms;

pub use package_json_exports::{IMPORT, Manifest, REQUIRE as REQUIRE_CONDITIONS, Type};
pub use filesystem_specifier_resolution::{Builtin, Modules, Read};
pub use load_time_esm_transforms::{FACADE, REQUIRE};

use std::fmt;

/// How a file becomes a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An ES module, loaded as it is.
    Esm,
    /// CommonJS: a facade and a body.
    CommonJs,
    /// JSON: a default export.
    Json,
    /// TypeScript, stripped; `commonjs` when the output is CommonJS.
    TypeScript {
        /// Whether the stripped code runs as CommonJS.
        commonjs: bool,
    },
}

impl Kind {
    /// # Returns
    /// Whether the file runs as CommonJS.
    pub fn commonjs(self) -> bool {
        matches!(self, Kind::CommonJs | Kind::TypeScript { commonjs: true })
    }
}

/// Why a module could not be resolved or loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Nothing answers to the specifier.
    NotFound,
    /// A file, manifest or source that is there but wrong.
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound => f.write_str("module not found"),
            Error::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}
