//! Standalone module transpilation: native compilation without a project host.

use anyhow::{ensure, Context};
use devtools_core::{compile_source, map_json_to_data_url, CompileOptions};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Options {
    pub input: PathBuf,
    pub out: Option<PathBuf>,
    pub sourcemap: bool,
    pub loader: Option<String>,
}

/// Compiles before creating output and never loads native/Vite project configuration.
///
/// Imports remain module specifiers, not bundled dependencies. Stdout contains only
/// JavaScript (and an optional inline map); file writes replace generated output
/// atomically rather than following an output symlink or mutating a hardlink inode.
pub fn run(options: Options) -> anyhow::Result<()> {
    let stdin = options.input == Path::new("-");
    let path = if stdin {
        let loader = options.loader.as_deref().unwrap_or("ts");
        ensure!(
            matches!(loader, "ts" | "tsx" | "js" | "jsx"),
            "unsupported stdin loader {loader:?}; use ts, tsx, js, or jsx"
        );
        PathBuf::from(format!("stdin.{loader}"))
    } else {
        ensure!(
            options.loader.is_none(),
            "--loader is only supported with stdin input '-'"
        );
        let extension = options.input.extension().and_then(|value| value.to_str());
        ensure!(
            matches!(extension, Some("ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "mjs" | "cjs")),
            "unsupported input extension for {}; expected .ts, .tsx, .js, .jsx, .mts, .cts, .mjs, or .cjs",
            options.input.display()
        );
        ensure!(
            options.input.is_file(),
            "input {} is not a readable file",
            options.input.display()
        );
        options.input.clone()
    };
    let source = if stdin {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .context("cannot read UTF-8 source from stdin")?;
        source
    } else {
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?
    };
    let mut compile_options = CompileOptions::standalone();
    compile_options.sourcemap = options.sourcemap;
    let compiled = compile_source(&path, &source, &compile_options)?;
    let mut code = compiled.code;
    let output = options.out.filter(|path| path != Path::new("-"));

    if let Some(output) = output {
        let map_path = options.sourcemap.then(|| {
            let mut name = output.as_os_str().to_os_string();
            name.push(".map");
            PathBuf::from(name)
        });
        validate_output(&output, (!stdin).then_some(path.as_path()))?;
        if let Some(map_path) = &map_path {
            validate_output(map_path, (!stdin).then_some(path.as_path()))?;
            ensure!(
                !same_file(&output, map_path)?,
                "output and source map must be different files"
            );
        }
        let map = if let Some(map_path) = &map_path {
            let map = compiled
                .map_json
                .context("compiler did not produce a source map")?;
            let name = map_path
                .file_name()
                .context("source map needs a file name")?;
            code.push_str(&format!(
                "\n//# sourceMappingURL={}\n",
                url_segment(name.as_encoded_bytes())
            ));
            Some(PendingOutput::new(map_path, map.as_bytes())?)
        } else {
            None
        };
        let output = PendingOutput::new(&output, code.as_bytes())?;
        // Both writes have been prepared before replacing either existing file.
        // Publishing two directory entries is not an atomic filesystem transaction.
        if let Some(map) = map {
            map.persist()?;
        }
        output.persist()?;
    } else {
        if options.sourcemap {
            let map = compiled
                .map_json
                .context("compiler did not produce a source map")?;
            code.push_str(&format!(
                "\n//# sourceMappingURL={}\n",
                map_json_to_data_url(&map)
            ));
        }
        crate::write_stdout(code.as_bytes())?;
    }
    Ok(())
}

/// Protects the source and existing directory targets before any output is staged.
fn validate_output(output: &Path, input: Option<&Path>) -> anyhow::Result<()> {
    ensure!(
        output.file_name().is_some(),
        "output {} needs a file name",
        output.display()
    );
    ensure!(
        !output.is_dir(),
        "output {} is a directory",
        output.display()
    );
    if let Some(input) = input {
        ensure!(
            !same_file(input, output)?,
            "refusing to overwrite input {} with output {}",
            input.display(),
            output.display()
        );
    }
    Ok(())
}

/// Canonical names catch symlink aliases; inode identity also catches Unix hardlinks.
/// Atomic replacement below protects other hardlinks on platforms without inode access.
fn same_file(first: &Path, second: &Path) -> anyhow::Result<bool> {
    let metadata = |path: &Path| -> anyhow::Result<Option<fs::Metadata>> {
        match fs::metadata(path) {
            Ok(metadata) => Ok(Some(metadata)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("cannot inspect {}", path.display())),
        }
    };
    let (Some(first_meta), Some(second_meta)) = (metadata(first)?, metadata(second)?) else {
        return Ok(false);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if first_meta.dev() == second_meta.dev() && first_meta.ino() == second_meta.ino() {
            return Ok(true);
        }
    }
    #[cfg(not(unix))]
    let _ = (first_meta, second_meta);
    Ok(first.canonicalize()? == second.canonicalize()?)
}

/// Encodes the map's filename as one URL segment, including spaces, Unicode and '#'.
fn url_segment(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

static NEXT_OUTPUT: AtomicU64 = AtomicU64::new(0);

/// A same-directory, exclusively created output removed on failure or unwind.
struct PendingOutput {
    temporary: PathBuf,
    destination: PathBuf,
}

impl PendingOutput {
    fn new(destination: &Path, bytes: &[u8]) -> anyhow::Result<Self> {
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create output directory {}", parent.display()))?;
        let (temporary, mut file): (PathBuf, File) = loop {
            let id = NEXT_OUTPUT.fetch_add(1, Ordering::Relaxed);
            let temporary = parent.join(format!(".devtools-{}-{id}.tmp", std::process::id()));
            if temporary.file_name() == destination.file_name() {
                continue;
            }
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => break (temporary, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("cannot prepare output {}", destination.display())
                    })
                }
            }
        };
        let pending = Self {
            temporary,
            destination: destination.to_path_buf(),
        };
        let written = file
            .write_all(bytes)
            .with_context(|| format!("cannot write output {}", destination.display()));
        drop(file);
        written?;
        Ok(pending)
    }

    fn persist(self) -> anyhow::Result<()> {
        fs::rename(&self.temporary, &self.destination)
            .with_context(|| format!("cannot replace output {}", self.destination.display()))
    }
}

impl Drop for PendingOutput {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.temporary);
    }
}
