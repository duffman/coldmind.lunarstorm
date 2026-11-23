//! The build pipeline as the debug runtime embeds it (spec 4, 6).
//!
//! [`Pipeline`] is the boundary: model extraction, frontend, backend and host
//! configuration generation (4a-4e) sit behind it. [`OutputDirPipeline`] is the
//! implementation shipped here: it loads an already generated output directory
//!
//! ```text
//! zebra.json      host configuration with the property and event tables
//! backend.js      the backend artifact (ES module)
//! frontend/**     HTML, JS, CSS and icons served to the WebViews
//! package.json    optional; its hash triggers dev.restart
//! ```
//!
//! Decision: dev reactions (6.1) are computed from the difference between two pipeline
//! outputs, not from which source file changed, so they stay correct whatever compiler
//! sits behind the trait.

use crate::manifest::HostConfig;
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Everything one pipeline run produces.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildOutput {
    /// Host configuration (4e) with the per-form tables (4d).
    pub config: HostConfig,
    /// Frontend files by path, no leading slash (4b).
    pub files: BTreeMap<String, Arc<[u8]>>,
    /// Backend artifact (4c).
    pub backend: Arc<str>,
    /// Hash of `package.json`; empty when absent.
    pub package_hash: String,
}

/// A compile error, shown in the shell as `dev.error`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Diagnostic {
    /// Message text.
    pub message: String,
    /// File the error is in, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// One compiler run over a project.
pub trait Pipeline: Send {
    /// Builds the project.
    ///
    /// # Returns
    /// The build output.
    ///
    /// # Errors
    /// Compile errors.
    fn build(&mut self) -> Result<BuildOutput, Vec<Diagnostic>>;

    /// Directory the file watcher observes.
    ///
    /// # Returns
    /// The project root.
    fn watch_root(&self) -> &Path;
}

/// Runs a pipeline with its panics caught at the boundary (6: a panic in the embedded
/// pipeline is shown as `dev.error`; the running app keeps its state).
///
/// # Arguments
/// * `pipeline` - the pipeline.
///
/// # Returns
/// The output, or diagnostics (a panic becomes one diagnostic).
pub fn run_guarded(pipeline: &mut dyn Pipeline) -> Result<BuildOutput, Vec<Diagnostic>> {
    match catch_unwind(AssertUnwindSafe(|| pipeline.build())) {
        Ok(result) => result,
        Err(panic) => {
            let text = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            Err(vec![Diagnostic { message: format!("compiler panicked: {text}"), file: None }])
        }
    }
}

/// Loads a generated output directory.
pub struct OutputDirPipeline {
    /// The output directory.
    root: PathBuf,
}

impl OutputDirPipeline {
    /// Creates the pipeline.
    ///
    /// # Arguments
    /// * `root` - the generated output directory.
    ///
    /// # Returns
    /// The pipeline.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        OutputDirPipeline { root: root.into() }
    }
}

impl Pipeline for OutputDirPipeline {
    /// Reads `zebra.json`, `backend.js`, `frontend/**` and `package.json`.
    ///
    /// # Returns
    /// The build output.
    ///
    /// # Errors
    /// A missing or unreadable file, or an invalid host configuration.
    fn build(&mut self) -> Result<BuildOutput, Vec<Diagnostic>> {
        let diag = |file: &Path, message: String| vec![Diagnostic { message, file: Some(file.display().to_string()) }];
        let config_path = self.root.join("zebra.json");
        let bytes = std::fs::read(&config_path).map_err(|e| diag(&config_path, e.to_string()))?;
        let config = HostConfig::parse(&bytes).map_err(|e| diag(&config_path, e))?;
        let backend_path = self.root.join("backend.js");
        let backend = std::fs::read_to_string(&backend_path).map_err(|e| diag(&backend_path, e.to_string()))?;
        let frontend = self.root.join("frontend");
        let mut files = BTreeMap::new();
        collect(&frontend, &frontend, &mut files).map_err(|e| diag(&frontend, e.to_string()))?;
        if !files.contains_key("index.html") {
            return Err(diag(&frontend, "frontend/index.html is missing".into()));
        }
        let package_hash = std::fs::read(self.root.join("package.json")).map(|b| crate::assets::sha256(&b)).unwrap_or_default();
        Ok(BuildOutput { config, files, backend: Arc::from(backend), package_hash })
    }

    /// The output directory.
    ///
    /// # Returns
    /// The root.
    fn watch_root(&self) -> &Path {
        &self.root
    }
}

/// Reads every file under a directory into the map.
///
/// # Arguments
/// * `base` - root the keys are relative to.
/// * `dir` - directory being read.
/// * `out` - destination map.
///
/// # Returns
/// Nothing.
///
/// # Errors
/// An I/O error.
fn collect(base: &Path, dir: &Path, out: &mut BTreeMap<String, Arc<[u8]>>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(base, &path, out)?;
        } else {
            let key = path.strip_prefix(base).expect("under base").to_string_lossy().replace('\\', "/");
            out.insert(key, Arc::from(std::fs::read(&path)?));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The example app shipped in `example/`, built through the real pipeline.
    ///
    /// # Returns
    /// Its build output.
    pub fn sample_build() -> BuildOutput {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("example");
        OutputDirPipeline::new(root).build().expect("example builds")
    }

    /// A pipeline that panics.
    struct Panicking;

    impl Pipeline for Panicking {
        /// Panics.
        ///
        /// # Returns
        /// Never.
        fn build(&mut self) -> Result<BuildOutput, Vec<Diagnostic>> {
            panic!("boom")
        }

        /// Any path.
        ///
        /// # Returns
        /// `/`.
        fn watch_root(&self) -> &Path {
            Path::new("/")
        }
    }

    #[test]
    fn example_builds() {
        let build = sample_build();
        assert_eq!(build.config.application.main_form, "MainForm");
        assert!(build.files.contains_key("index.html"));
    }

    #[test]
    fn a_compiler_panic_becomes_a_diagnostic() {
        let errors = run_guarded(&mut Panicking).unwrap_err();
        assert_eq!(errors[0].message, "compiler panicked: boom");
    }
}
