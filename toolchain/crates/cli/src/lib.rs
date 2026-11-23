//! `jetbunny --serve`, `--build`, `--preview` and `--transpile`: development, builds, preview
//! and native transpilation, run from the jetbunny binary.
mod arguments;
mod development;
mod preview;
mod transpile;

use arguments::Command;
use std::io::{self, Write};

pub use arguments::usage;

/// Runs one command. `args` starts with the command name (`dev`, `build`, `preview`,
/// `transpile`), followed by its options. Prints its own output and diagnostics and returns the
/// exit code: 0 on success, 2 for usage errors, 1 for operational failures with their context chain.
/// Stdout pipe handling is scoped to writes, never to operational errors.
pub fn run(args: Vec<String>) -> u8 {
    let command = match arguments::parse(args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("error: {error:#}\n\nRun `jetbunny --help` or `jetbunny <command> --help` for usage.");
            return 2;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: cannot start the async runtime: {error}");
            return 1;
        }
    };
    let code = match runtime.block_on(execute(command)) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error:#}");
            1
        }
    };
    let _ = io::stdout().flush();
    code
}

/// Only writes to our stdout may ignore a consumer closing its pipe. Broken pipes
/// from compiler subprocesses or other operations must remain real failures.
pub(crate) fn write_stdout(bytes: &[u8]) -> anyhow::Result<()> {
    match io::stdout().lock().write_all(bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(anyhow::Error::new(error).context("cannot write stdout")),
    }
}

async fn execute(command: Command) -> anyhow::Result<()> {
    match command {
        Command::Help(topic) => write_stdout(format!("{}\n", arguments::help(topic)).as_bytes())?,
        Command::Version => {
            write_stdout(format!("devtools {}\n", env!("CARGO_PKG_VERSION")).as_bytes())?
        }
        Command::Dev(config) => development::run(config).await?,
        Command::Build(options) => {
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
        Command::Preview { root, out, port } => preview::run(root, out, port).await?,
        Command::Transpile(options) => transpile::run(options)?,
    }
    Ok(())
}
