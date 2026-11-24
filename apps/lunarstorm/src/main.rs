//! The `lunarstorm` command: mounts the packages enabled by Cargo features on the
//! command-line host and runs the process's command line.
//!
//! | feature   | package                    | commands                      |
//! |-----------|----------------------------|-------------------------------|
//! | `runtime` | `lunarstorm-host-cli`      | `run` (default command)       |
//! | `build`   | `lunarstorm-pipeline`      | `build`, `build types`, `build check` |
//!
//! The host adds `describe`, `completions`, `--color`, `--help`, `--version` and
//! external `lunarstorm-<name>` commands. See `doc/cli.md`.

use lunarstorm_cli_host::Cli;

/// Runs the command line and exits with its status.
fn main() {
    let cli = Cli::new("lunarstorm", "☾ lunarstorm", env!("CARGO_PKG_VERSION"), "JavaScript runtime and desktop build tools");
    #[cfg(feature = "runtime")]
    let cli = cli.detail(lunarstorm_host_cli::engine()).package(lunarstorm_host_cli::package());
    #[cfg(feature = "build")]
    let cli = cli.package(lunarstorm_pipeline::command::package());
    std::process::exit(cli.run(std::env::args().skip(1).collect()));
}
