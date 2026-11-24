//! The frontend tools as a package of the `lunarstorm` command line: a development server
//! with hot updates (`serve`), production bundles (`bundle`), a local preview of a bundle
//! (`preview`) and standalone transpilation (`transpile`).
//!
//! | module        | role                                                          |
//! |---------------|---------------------------------------------------------------|
//! | [`command`]   | the commands' specs and handlers (`lunarstorm-cli-contract`)  |
//! | `development` | runs the dev server until Ctrl-C                              |
//! | `preview`     | serves an existing bundle, with SPA routes                    |
//! | `transpile`   | one module to JavaScript, without bundling                    |
//!
//! Decision: these were the `jetbunny --serve | --build | --preview | --transpile` flags
//! of a binary that did not survive the monorepo merge. They are now `lunarstorm`
//! commands; the former parser and help text are archived in
//! `.archive/2026-11-24/toolchain/crates/cli/src/arguments.rs`. `--build` is named
//! `bundle` because `build` is the desktop pipeline's command.

pub mod command;
mod development;
mod preview;
mod transpile;

use devtools_bundler::BuildOptions;
use devtools_server::Config;
use std::io::{self, Write};
use std::path::PathBuf;

pub use command::package;

/// A parsed frontend command, ready to run.
pub(crate) enum Task {
    /// Serve source modules with hot updates.
    Serve(Config),
    /// Bundle for production.
    Bundle(BuildOptions),
    /// Serve an existing bundle.
    Preview {
        /// The app root.
        root: PathBuf,
        /// The bundle directory, when not the configured or default one.
        out: Option<PathBuf>,
        /// The port; 0 selects a free one.
        port: u16,
    },
    /// Transpile one module.
    Transpile(transpile::Options),
}

/// Runs a task to completion on a multi-threaded async runtime.
///
/// # Arguments
/// * `task` - the task.
///
/// # Returns
/// `Ok` when it completed; stdout is flushed either way.
///
/// # Errors
/// The task's failure with its context chain, or a runtime that cannot start.
pub(crate) fn execute(task: Task) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| anyhow::anyhow!("cannot start the async runtime: {error}"))?;
    let result = runtime.block_on(run(task));
    let _ = io::stdout().flush();
    result
}

/// Only writes to our stdout may ignore a consumer closing its pipe. Broken pipes
/// from compiler subprocesses or other operations must remain real failures.
///
/// # Arguments
/// * `bytes` - what to write.
///
/// # Errors
/// Any write error other than a broken pipe.
pub(crate) fn write_stdout(bytes: &[u8]) -> anyhow::Result<()> {
    match io::stdout().lock().write_all(bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(anyhow::Error::new(error).context("cannot write stdout")),
    }
}

/// Runs one task.
///
/// # Arguments
/// * `task` - the task.
///
/// # Errors
/// The task's failure.
async fn run(task: Task) -> anyhow::Result<()> {
    match task {
        Task::Serve(config) => development::run(config).await?,
        Task::Bundle(options) => {
            let started = std::time::Instant::now();
            let report = devtools_bundler::build(&options).await?;
            for (source, file) in &report.entries {
                write_stdout(format!("  {source} -> {file}\n").as_bytes())?;
            }
            if let Some(stylesheet) = &report.stylesheet {
                write_stdout(format!("  styles -> {stylesheet}\n").as_bytes())?;
            }
            write_stdout(
                format!(
                    "devtools built {} chunk(s) into {} in {:.2?}\n",
                    report.chunks,
                    report.out.display(),
                    started.elapsed()
                )
                .as_bytes(),
            )?;
        }
        Task::Preview { root, out, port } => preview::run(root, out, port).await?,
        Task::Transpile(options) => transpile::run(options)?,
    }
    Ok(())
}
