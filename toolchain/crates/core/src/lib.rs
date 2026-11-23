//! Shared Devermind configuration and single-file compilation utilities.
//!
//! Development serving and production bundling use the same native configuration
//! contract; their host-specific pipelines live in the server and bundler crates.

pub mod config;
mod toml_config;
mod vite_compat;

/// Compatibility module name; new hosts should use [`config`].
pub use config as vite;

use std::fs;
use std::path::{Path, PathBuf};

pub use oj_compiler::{map_json_to_data_url, CompileOptions, CompileOutput, JsxConfig};

/// Which pipeline a compile runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// React Refresh registration, HMR hot-context injection, inline-able source maps.
    Development,
    /// No refresh or HMR code; defines for production.
    Production,
}

impl Mode {
    /// `oj_compiler` options for this mode.
    pub fn options(self) -> CompileOptions {
        match self {
            Mode::Development => CompileOptions::dev(),
            Mode::Production => CompileOptions::prod(),
        }
    }
}

/// Failure to read, compile or write one module.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Compile(#[from] oj_compiler::CompileError),
}

/// Compiles the source text of `path` (TS, TSX, JSX or JS by extension).
pub fn compile_source(
    path: &Path,
    source: &str,
    opts: &CompileOptions,
) -> Result<CompileOutput, CoreError> {
    Ok(oj_compiler::compile(path, source, opts)?)
}

/// Reads and compiles `path`.
pub fn compile_file(path: &Path, opts: &CompileOptions) -> Result<CompileOutput, CoreError> {
    let source = fs::read_to_string(path).map_err(|source| CoreError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    compile_source(path, &source, opts)
}

/// Compiles `entry` in production mode and writes the ESM output to `out`, with the source
/// map beside it as `<out>.map` when one is produced. Imports are left as written.
pub fn build_file(entry: &Path, out: &Path) -> Result<CompileOutput, CoreError> {
    let output = compile_file(entry, &Mode::Production.options())?;
    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir).map_err(|source| CoreError::Write {
            path: dir.to_path_buf(),
            source,
        })?;
    }
    let mut code = output.code.clone();
    if let Some(map) = &output.map_json {
        let map_path = out.with_extension("js.map");
        fs::write(&map_path, map).map_err(|source| CoreError::Write {
            path: map_path.clone(),
            source,
        })?;
        let name = map_path.file_name().unwrap_or_default().to_string_lossy();
        code.push_str(&format!("\n//# sourceMappingURL={name}\n"));
    }
    fs::write(out, code).map_err(|source| CoreError::Write {
        path: out.to_path_buf(),
        source,
    })?;
    Ok(output)
}
