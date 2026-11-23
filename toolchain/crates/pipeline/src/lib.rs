//! The build pipeline (spec 4): from a model project to the output directory the
//! native host runs.
//!
//! ```text
//! zebra.json      host configuration (4e) with the per-form bridge tables (4d)
//! backend.js      backend artifact (4c)
//! frontend/**     WebView files (4b)
//! package.json    the project's, copied; a change restarts the app (spec 6.1)
//! ```
//!
//! [`command`] exports the pipeline as the `build` command of the `lunarstorm` command
//! line (`lunarstorm-cli-contract`).
//!
//! Decision: the pipeline never writes a partial build. A project with a compile
//! error produces diagnostics and leaves the output directory as it was, so the
//! running app keeps its last good build (spec 6, "Compiler failures in dev").

pub mod backend;
pub mod command;
pub mod frontend;
pub mod host;
pub mod types;

use std::fs;
use std::io;
use std::path::Path;

pub use lunarstorm_model::{Code, Diagnostic, Project};

/// Why a build produced no output.
#[derive(Debug)]
pub enum Failure {
    /// The project has compile errors.
    Model(Vec<Diagnostic>),
    /// The project's backend code does not bundle; the text is the bundler's
    /// diagnostics.
    Backend(String),
    /// The output could not be written.
    Io(io::Error),
}

impl From<io::Error> for Failure {
    /// Wraps a write error.
    ///
    /// # Arguments
    /// * `error` - the I/O error.
    ///
    /// # Returns
    /// The failure.
    fn from(error: io::Error) -> Self {
        Failure::Io(error)
    }
}

/// Builds a project into an output directory.
///
/// # Arguments
/// * `project_root` - the model project (the directory holding `src/`).
/// * `out` - the output directory; created when missing.
///
/// # Returns
/// The extracted model the output was generated from.
///
/// # Errors
/// [`Failure::Model`] with every compile error, [`Failure::Backend`] when the
/// backend code does not bundle, or [`Failure::Io`].
pub fn build(project_root: &Path, out: &Path) -> Result<Project, Failure> {
    let extraction = lunarstorm_model::extract(project_root);
    let Some(project) = extraction.project else { return Err(Failure::Model(extraction.diagnostics)) };

    // Everything that can fail on the project's code runs before the first write,
    // so a failed build leaves the previous output intact.
    let backend = backend::generate(&project, project_root).map_err(Failure::Backend)?;

    fs::create_dir_all(out)?;
    fs::write(out.join("backend.js"), backend)?;
    // The frontend directory is rewritten whole, so the module of a removed form
    // does not stay behind and keep being served.
    let frontend = out.join("frontend");
    match fs::remove_dir_all(&frontend) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    fs::create_dir_all(frontend.join("forms"))?;
    for (path, content) in frontend::generate(&project) {
        fs::write(frontend.join(path), content)?;
    }
    let mut config = serde_json::to_string_pretty(&host::host_config(&project)).expect("JSON values always serialize");
    config.push('\n');
    fs::write(out.join("zebra.json"), config)?;
    match fs::read(project_root.join("package.json")) {
        Ok(package) => fs::write(out.join("package.json"), package)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(project)
}
