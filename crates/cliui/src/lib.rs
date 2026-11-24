//! Terminal UI for Lunarstorm command-line tools.
//!
//! The package owns how things look; the tool that uses it owns what is said.
//!
//! | part        | role                                                              |
//! |-------------|-------------------------------------------------------------------|
//! | [`term`]    | color detection per stream, styling pens, width, cursor restore   |
//! | [`screen`]  | help screens, brand mark, usage errors, "did you mean"            |
//! | [`report`]  | uncaught-error reports with source code frames                    |
//! | `js/ui.js`  | script-facing module: styles, progress bars, spinners             |
//!
//! Decisions:
//! - The package depends only on `libc`. It knows nothing about the JS engine or
//!   the reactor, so any Lunarstorm tool can use it.
//! - Call [`term::init`] once at startup; everything else reads the depths it set.
//! - The script-facing half is shipped as source ([`UI_SOURCE`]) for the host to
//!   compile as an ES module. It expects the host to provide, on
//!   `__lunarstorm.internal`:
//!   - `ops.color` and `ops.tty` - `[stdout, stderr]` color depths ([`term::Depth`]
//!     as integers) and tty flags;
//!   - `ops.columns()`, `ops.live(on)` - [`term::columns`] and [`term::live`];
//!   - `ops.print(fd, text)`, `ops.flush()`, `ops.now()` - write, flush stdout,
//!     monotonic milliseconds;
//!   - `live.wrap` - a slot the module sets to `(fd, text) => void` while bars are
//!     on screen; the host's console must route output through it when non-null.

pub mod report;
pub mod screen;
pub mod term;

/// Source of the script-facing UI module (`style`, `progress`, `spinner`).
pub const UI_SOURCE: &str = include_str!("../js/ui.js");
