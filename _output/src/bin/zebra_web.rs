//! `zebra-web <project> [--port <n>]`: serves a model project to a browser tab with its
//! in-page host (spec 6.3, Browser) on http://127.0.0.1:<port> (default 7878).

use std::net::TcpListener;
use std::process::exit;

use zebra_desktop::browser;
use zebra_desktop::pipeline::{run_guarded, OutputDirPipeline};

/// Default port; `0` picks a free one.
const DEFAULT_PORT: u16 = 7878;

/// Parses the arguments, builds the project once and serves it.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first().filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: zebra-web <project> [--port <n>]");
        exit(2);
    };
    let port = match args.iter().position(|a| a == "--port").map(|i| args.get(i + 1).and_then(|p| p.parse::<u16>().ok())) {
        None => DEFAULT_PORT,
        Some(Some(port)) => port,
        Some(None) => {
            eprintln!("zebra-web: --port needs a number");
            exit(2);
        }
    };
    let build = match run_guarded(&mut OutputDirPipeline::new(root)) {
        Ok(build) => build,
        Err(diagnostics) => {
            for d in diagnostics {
                eprintln!("{}: {}", d.file.unwrap_or_default(), d.message);
            }
            exit(1);
        }
    };
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap_or_else(|e| {
        eprintln!("zebra-web: cannot listen on 127.0.0.1:{port}: {e}");
        exit(1);
    });
    println!("http://{}", listener.local_addr().expect("bound address"));
    if let Err(e) = browser::serve(build, listener) {
        eprintln!("zebra-web: {e}");
        exit(1);
    }
}
