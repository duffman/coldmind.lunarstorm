//! The runtime's package for the `lunar` command line: `run`, `debug` and `jbn`.
//!
//! | module                          | role                                                              |
//! |---------------------------------|-------------------------------------------------------------------|
//! | [`lunar_runtime_commands`]      | the `runtime` package: specs and handlers driving the Runtime     |
//! | `command_line_host_services`    | the host services and the terminal bindings this host registers   |
//! | `terminal_ui_area`              | the terminal API as the HostApi area `ui` (`lunarstorm:ui`)       |
//!
//! Everything else is a package of its own: the Runtime (`event-loop-runtime/`), the VM
//! contract (`engine-contract/`), the QuickJS adapter (`engine-quickjs-adapter/`), the terminal UI
//! (`crates/cliui/`) and the command-line host (`crates/cli-host/`).

pub mod lunar_runtime_commands;
mod command_line_host_services;
mod terminal_ui_area;

pub use lunar_runtime_commands::{compile, engine, package, type_files};
