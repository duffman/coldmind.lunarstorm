//! Release step (spec 6.3): runs the pipeline, writes the archive, attaches it to the
//! prebuilt release runtime, and packages and signs for the target OS.
//!
//! Usage: `zebra-pack <project> --runtime <zebra-release> --out <dir>
//!         [--target macos|windows|linux] [--sign <identity>]`

use std::path::PathBuf;
use zebra_desktop::package::{package, Target};
use zebra_desktop::pipeline::{OutputDirPipeline, Pipeline};

/// Usage text.
const USAGE: &str = "usage: zebra-pack <project> --runtime <zebra-release> --out <dir> [--target macos|windows|linux] [--sign <identity>]";

/// Parses arguments, builds and packages.
fn main() {
    let mut args = std::env::args().skip(1);
    let (mut project, mut runtime, mut out, mut target, mut sign) = (None, None, None, Target::host(), None);
    while let Some(arg) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| fail(&format!("{arg} needs a value")));
        match arg.as_str() {
            "--runtime" => runtime = Some(PathBuf::from(value())),
            "--out" => out = Some(PathBuf::from(value())),
            "--sign" => sign = Some(value()),
            "--target" => {
                target = match value().as_str() {
                    "macos" => Target::MacOs,
                    "windows" => Target::Windows,
                    "linux" => Target::Linux,
                    other => fail(&format!("unknown target {other}")),
                }
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            _ if project.is_none() && !arg.starts_with('-') => project = Some(PathBuf::from(arg)),
            _ => fail(&format!("unexpected argument {arg}")),
        }
    }
    let (Some(project), Some(runtime), Some(out)) = (project, runtime, out) else { fail(USAGE) };
    let build = OutputDirPipeline::new(project).build().unwrap_or_else(|errors| {
        for e in errors {
            eprintln!("error: {}{}", e.file.map(|f| format!("{f}: ")).unwrap_or_default(), e.message);
        }
        std::process::exit(1);
    });
    match package(&build, &runtime, &out, target, sign.as_deref()) {
        Ok(path) => println!("{}", path.display()),
        Err(e) => fail(&e),
    }
}

/// Prints an error and exits with status 1.
///
/// # Arguments
/// * `message` - the error.
///
/// # Returns
/// Never.
fn fail(message: &str) -> ! {
    eprintln!("zebra-pack: {message}");
    std::process::exit(1)
}
