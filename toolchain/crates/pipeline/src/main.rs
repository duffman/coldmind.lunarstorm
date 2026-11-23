//! `lunarstorm-build <project> <out>`: runs the build pipeline on a model project.
//! `lunarstorm-build --types <project>`: writes `<project>/src/desktop.d.ts`, the
//! declarations `tsc` checks the project against.
//!
//! Exit status: 0 built, 1 compile errors or a write failure, 2 usage.

use std::path::PathBuf;
use std::process::ExitCode;

use lunarstorm_pipeline::{build, types, Failure};

/// Runs the pipeline and reports the result.
///
/// # Returns
/// The process exit status.
fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let (Some(project), Some(out), None) = (arguments.next(), arguments.next(), arguments.next()) else {
        eprintln!("usage: lunarstorm-build <project> <out>\n       lunarstorm-build --types <project>");
        return ExitCode::from(2);
    };
    if project == "--types" {
        let file = PathBuf::from(out).join("src/desktop.d.ts");
        return match std::fs::write(&file, types::declarations()) {
            Ok(()) => {
                println!("wrote {}", file.display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("lunarstorm-build: cannot write {}: {error}", file.display());
                ExitCode::FAILURE
            }
        };
    }
    let (project, out) = (PathBuf::from(project), PathBuf::from(out));
    match build(&project, &out) {
        Ok(model) => {
            println!("built {} form(s) into {}", model.forms.len(), out.display());
            ExitCode::SUCCESS
        }
        Err(Failure::Model(diagnostics)) => {
            // `file:line:column: message` is the form editors and terminals link.
            for diagnostic in &diagnostics {
                let file = project.join(&diagnostic.file);
                eprintln!("{}:{}:{}: {}", file.display(), diagnostic.at.line, diagnostic.at.column, diagnostic.message);
            }
            eprintln!("{} error(s)", diagnostics.len());
            ExitCode::FAILURE
        }
        Err(Failure::Backend(diagnostics)) => {
            eprintln!("{diagnostics}");
            ExitCode::FAILURE
        }
        Err(Failure::Io(error)) => {
            eprintln!("lunarstorm-build: cannot write {}: {error}", out.display());
            ExitCode::FAILURE
        }
    }
}
