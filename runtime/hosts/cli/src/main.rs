//! The `lunarstorm` command: the CLI app host.
//!
//! | module | role                                                              |
//! |--------|-------------------------------------------------------------------|
//! | `cli`  | the command line: options, screens, and driving the Runtime       |
//! | `host` | the host services and the terminal bindings this host registers   |
//!
//! Everything else is a package of its own: the Runtime (`core/`), the VM contract
//! (`vm/`), the QuickJS adapter (`vm-quickjs/`) and the terminal UI (`cliui/`).

mod cli;
mod host;

/// Runs the command line and exits with its status.
fn main() {
    std::process::exit(cli::run());
}
