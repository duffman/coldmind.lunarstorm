//! The runtime's package for the `lunarstorm` command line: the `run` command.
//!
//! | module | role                                                              |
//! |--------|-------------------------------------------------------------------|
//! | `cli`  | the `run` command's spec, and driving the Runtime it owns         |
//! | `host` | the host services and the terminal bindings this host registers   |
//!
//! Everything else is a package of its own: the Runtime (`core/`), the VM contract
//! (`vm/`), the QuickJS adapter (`engines/quickjs/`), the terminal UI
//! (`crates/cliui/`) and the command-line host (`crates/cli-host/`).

mod cli;
mod host;
mod ui;

pub use cli::{engine, package};
