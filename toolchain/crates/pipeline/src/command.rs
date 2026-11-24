//! The build pipeline's package for the `lunarstorm` command line, and the reporting
//! it shares with the standalone `lunarstorm-build` binary.
//!
//! ```text
//! lunarstorm build [-p <dir>] [-o <dir>]     build the project into its output directory
//! lunarstorm build [-p <dir>] types          write <project>/src/desktop.d.ts
//! lunarstorm build [-p <dir>] check          extract the model and report compile errors
//! ```
//!
//! Decisions:
//! - `--project` is global and the `build` level's `enter` turns it into a [`Target`]
//!   in the context, so `types` and `check` read the project the same way whether
//!   the option comes before or after the subcommand.
//! - The project is an option, not a positional: a positional would be ambiguous with
//!   the subcommand names (a project directory called `types`).
//! - The output directory defaults to `<project>/dist`, computed by the handler
//!   because it depends on the project.

use std::path::{Path, PathBuf};

use lunarstorm_cli_contract::{Arg, Command, Failure, Invocation, Package, ValueType};

use crate::{build, types, Project};

/// The project the `build` level hands to its subcommands.
pub struct Target {
    /// The model project (the directory holding `src/`).
    pub project: PathBuf,
}

/// The build pipeline's package.
struct BuildPackage;

/// Returns the build pipeline's package for mounting on the command line.
///
/// # Returns
/// The package; it exports `build` with the subcommands `types` and `check`.
pub fn package() -> Box<dyn Package> {
    Box::new(BuildPackage)
}

impl Package for BuildPackage {
    /// # Returns
    /// `"build"`.
    fn id(&self) -> &'static str {
        "build"
    }

    /// # Returns
    /// The `build` command tree.
    fn commands(&self) -> Vec<Command> {
        let project = Arg::option("project")
            .short('p')
            .placeholder("<dir>")
            .value(ValueType::Path)
            .default(".")
            .about("the project: the directory holding src/")
            .global();
        let out = Arg::option("out")
            .short('o')
            .placeholder("<dir>")
            .value(ValueType::Path)
            .about("the output directory [default: <project>/dist]");
        vec![Command::new("build", "build a desktop project into the directory the native host runs")
            .arg(project)
            .arg(out)
            .enter(|invocation| {
                let project = invocation.values.path("project").unwrap_or_else(|| PathBuf::from("."));
                invocation.context.insert(Target { project });
                Ok(())
            })
            .run(|invocation| {
                let project = invocation.context.require::<Target>()?.project.clone();
                let out = invocation.values.path("out").unwrap_or_else(|| project.join("dist"));
                Ok(report("lunarstorm", &project, &out, build(&project, &out)))
            })
            .child(Command::new("types", "write src/desktop.d.ts, the declarations tsc checks the project against").run(
                |invocation| Ok(write_types("lunarstorm", &invocation.context.require::<Target>()?.project)),
            ))
            .child(Command::new("check", "check the project's model and report compile errors; writes nothing").run(check))
            .example("lunarstorm build -p templates/starters/desktop")
            .example("lunarstorm build -p app -o out")
            .example("lunarstorm build types -p app")]
    }
}

/// The `check` handler.
///
/// # Arguments
/// * `invocation` - carries the [`Target`].
///
/// # Returns
/// 0 when the model has no compile error, else 1 after printing them.
///
/// # Errors
/// None in practice; the [`Target`] is always in the context below `build`.
fn check(invocation: &mut Invocation) -> Result<i32, Failure> {
    let project = &invocation.context.require::<Target>()?.project;
    let extraction = lunarstorm_model::extract(project);
    match extraction.project {
        Some(model) => {
            println!("ok: {} form(s) in {}", model.forms.len(), project.display());
            Ok(0)
        }
        None => {
            print_diagnostics(project, &extraction.diagnostics);
            Ok(1)
        }
    }
}

/// Reports the result of a build.
///
/// # Arguments
/// * `program` - the prefix of I/O error messages.
/// * `project` - the project built.
/// * `out` - the output directory.
/// * `result` - what [`build`] returned.
///
/// # Returns
/// The exit status: 0 built, 1 compile errors or a write failure.
pub fn report(program: &str, project: &Path, out: &Path, result: Result<Project, crate::Failure>) -> i32 {
    match result {
        Ok(model) => {
            println!("built {} form(s) into {}", model.forms.len(), out.display());
            0
        }
        Err(crate::Failure::Model(diagnostics)) => {
            print_diagnostics(project, &diagnostics);
            1
        }
        Err(crate::Failure::Backend(diagnostics)) => {
            eprintln!("{diagnostics}");
            1
        }
        Err(crate::Failure::Io(error)) => {
            eprintln!("{program}: cannot write {}: {error}", out.display());
            1
        }
    }
}

/// Writes `<project>/src/desktop.d.ts`.
///
/// # Arguments
/// * `program` - the prefix of the error message.
/// * `project` - the project.
///
/// # Returns
/// 0 written, 1 when the file cannot be written.
pub fn write_types(program: &str, project: &Path) -> i32 {
    let file = project.join("src/desktop.d.ts");
    match std::fs::write(&file, types::declarations()) {
        Ok(()) => {
            println!("wrote {}", file.display());
            0
        }
        Err(error) => {
            eprintln!("{program}: cannot write {}: {error}", file.display());
            1
        }
    }
}

/// Prints compile errors as `file:line:column: message`, the form editors and
/// terminals link, then their count.
///
/// # Arguments
/// * `project` - the project the diagnostics' files are relative to.
/// * `diagnostics` - the errors.
fn print_diagnostics(project: &Path, diagnostics: &[crate::Diagnostic]) {
    for diagnostic in diagnostics {
        let file = project.join(&diagnostic.file);
        eprintln!("{}:{}:{}: {}", file.display(), diagnostic.at.line, diagnostic.at.column, diagnostic.message);
    }
    eprintln!("{} error(s)", diagnostics.len());
}
