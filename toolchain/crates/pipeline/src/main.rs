//! `lunarstorm-build <project> <out>`: runs the build pipeline on a model project.
//! `lunarstorm-build --types <project>`: writes `<project>/src/desktop.d.ts`, the
//! declarations `tsc` checks the project against.
//!
//! The same pipeline is `lunarstorm build` on the unified command line
//! ([`lunarstorm_pipeline::command`]); both report through the functions there.
//!
//! Exit status: 0 built, 1 compile errors or a write failure, 2 usage.

use std::path::PathBuf;
use std::process::ExitCode;

use lunarstorm_pipeline::{build, command};

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
    let status = if project == "--types" {
        command::write_types("lunarstorm-build", &PathBuf::from(out))
    } else {
        let (project, out) = (PathBuf::from(project), PathBuf::from(out));
        command::report("lunarstorm-build", &project, &out, build(&project, &out))
    };
    ExitCode::from(status as u8)
}
