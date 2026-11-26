//! The runtime's package for the `lunar` command line: `run`, `debug` and `jbn`.
//!
//! | module        | role                                                                 |
//! |---------------|----------------------------------------------------------------------|
//! | [`command`]   | the `runtime` package: specs and handlers, driving the Runtime it owns |
//! | `host`        | the host services and the terminal bindings this host registers      |
//! | `ui`          | the terminal API as the HostApi area `ui` (`lunarstorm:ui`)          |
//!
//! Everything else is a package of its own: the Runtime (`core/`), the VM contract
//! (`vm/`), the QuickJS adapter (`engines/quickjs/`), the terminal UI
//! (`crates/cliui/`) and the command-line host (`crates/cli-host/`).

pub mod command;
mod host;
mod ui;

pub use command::{compile, engine, package};
