//! The `lunarstation` command: mounts the packages enabled by Cargo features on the
//! command-line host and runs the process's command line.
//!
//! | feature   | package                    | commands                      |
//! |-----------|----------------------------|-------------------------------|
//! | `runtime` | `lunarstorm-host-cli`      | `run` (default command)       |
//! | `build`   | `lunarstorm-pipeline`      | `build`, `build types`, `build check` |
//! | `frontend`| `devtools-cli`             | `serve`, `bundle`, `preview`, `transpile` |
//! | `project` | `lunarstorm-project`       | `project types`, `project <type> <action>`, `insight`, `compile`, `sign` |
//!
//! The host adds `describe`, `completions`, `--color`, `--help`, `--version` and
//! external `lunarstation-<name>` commands.

use lunarstorm_cli_host::Cli;

/// Runs the command line and exits with its status.
///
/// Decision: external commands use the prefix `lunarstation`, not `lunarstorm`, so
/// `lunarstation foo` runs `lunarstation-foo` and the two binaries' externals never
/// shadow each other.
fn main() {
    let cli = Cli::new("lunarstation", "☾ lunarstation", env!("CARGO_PKG_VERSION"), "Lunarstorm runtime, desktop build tools and frontend tools").external_prefix("lunarstation");
    #[cfg(feature = "runtime")]
    let cli = cli.detail(lunarstorm_host_cli::command::engine()).package(lunarstorm_host_cli::command::package());
    #[cfg(feature = "build")]
    let cli = cli.package(lunarstorm_pipeline::command::package());
    #[cfg(feature = "frontend")]
    let cli = cli.package(devtools_cli::command::package());
    #[cfg(feature = "project")]
    let cli = cli.package(lunarstorm_project::package(vec![lunarstorm_cli_types::types, lunarstorm_desktop_types::types]));
    std::process::exit(cli.run(std::env::args().skip(1).collect()));
}
