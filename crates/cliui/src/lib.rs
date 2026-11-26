//! Terminal UI for Lunarstorm command-line tools.
//!
//! The package owns how things look; the tool that uses it owns what is said.
//!
//! | part        | role                                                              |
//! |-------------|-------------------------------------------------------------------|
//! | [`term`]    | color detection per stream, styling pens, width, cursor restore   |
//! | [`screen`]  | help screens, brand mark, usage errors, "did you mean"            |
//! | [`report`]  | uncaught-error reports with source code frames                    |
//!
//! Decisions:
//! - The package depends only on `libc`. It knows nothing about the JS engine or
//!   the reactor, so any Lunarstorm tool can use it.
//! - Call [`term::init`] once at startup; everything else reads the depths it set.
//! - The script-facing module (`lunarstorm:ui`: styles, progress bars, spinners)
//!   is native and lives with the host that serves it
//!   (`platform/cli/host/src/terminal_ui_module.rs`), because it binds to
//!   the JS engine; it draws with [`term`]. The source module `js/ui.js` is archived.

pub mod report;
pub mod screen;
pub mod term;
