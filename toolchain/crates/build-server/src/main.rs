use anyhow::{bail, Context, Result};
use ls_build_server::{BuildServer, BundleOptions, ServeOptions};
use std::path::PathBuf;

const HELP: &str = "ls-build-server serve [root] [--port PORT] [--no-watch]\nls-build-server bundle [root] [--out DIR] [--no-minify]\n\nroot defaults to the current directory. Project configuration is read by the toolchain.";

struct Arguments {
    command: String,
    root: PathBuf,
    serve: ServeOptions,
    bundle: BundleOptions,
}

fn arguments() -> Result<Option<Arguments>> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        println!("{HELP}");
        return Ok(None);
    };
    if command == "--help" || command == "-h" {
        println!("{HELP}");
        return Ok(None);
    }
    if command != "serve" && command != "bundle" {
        bail!("unknown command {command:?}\n{HELP}");
    }
    let mut result = Arguments {
        command,
        root: PathBuf::from("."),
        serve: ServeOptions::default(),
        bundle: BundleOptions::default(),
    };
    let mut root_given = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                println!("{HELP}");
                return Ok(None);
            }
            "--port" if result.command == "serve" => {
                let value = args.next().context("--port requires a value")?;
                result.serve.port = Some(
                    value
                        .parse()
                        .context("--port must be an integer from 0 to 65535")?,
                );
            }
            "--no-watch" if result.command == "serve" => result.serve.watch = false,
            "--out" if result.command == "bundle" => {
                result.bundle.out = Some(args.next().context("--out requires a directory")?.into())
            }
            "--no-minify" if result.command == "bundle" => result.bundle.minify = Some(false),
            _ if argument.starts_with('-') => {
                bail!("unknown option {argument:?} for {}", result.command)
            }
            _ if root_given => bail!("unexpected argument {argument:?}"),
            _ => {
                result.root = argument.into();
                root_given = true;
            }
        }
    }
    Ok(Some(result))
}

#[tokio::main]
async fn main() -> Result<()> {
    let Some(args) = arguments()? else {
        return Ok(());
    };
    let mut server = BuildServer::new(args.root)?;
    if args.command == "serve" {
        let address = server.start(args.serve)?;
        println!("Build server: http://{address}/");
        tokio::signal::ctrl_c()
            .await
            .context("waiting for shutdown")?;
        server.stop();
    } else {
        let report = server.bundle(args.bundle).await?;
        println!(
            "Bundled {} entry point(s), {} chunk(s) into {}",
            report.entries.len(),
            report.chunks,
            report.out.display()
        );
        if let Some(stylesheet) = report.stylesheet {
            println!("Stylesheet: {stylesheet}");
        }
    }
    Ok(())
}
