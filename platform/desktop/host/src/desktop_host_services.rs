//! [`DesktopHost`]: the host services the desktop backend answers (stdio and
//! log through the [`Shell`], fs and net policy, clock, entropy).

use crate::*;

/// The host services of one backend instance.
pub(crate) struct DesktopHost<S> {
    /// The desktop process.
    pub(crate) shell: Arc<S>,
    /// The instance's epoch.
    pub(crate) epoch: u64,
    /// `process.argv[0]`.
    pub(crate) name: String,
    /// Filesystem roots the backend's `node:fs` may touch, canonical.
    pub(crate) roots: Vec<PathBuf>,
    /// `process.env`.
    pub(crate) environment: Vec<(String, String)>,
    /// The Runtime manifest: HostApi with the permitted areas.
    pub(crate) manifest: Manifest,
    /// Whether the entry module is being evaluated; an error then fails the load.
    pub(crate) loading: Cell<bool>,
    /// The first error raised while loading.
    pub(crate) failure: RefCell<Option<String>>,
}

/// Names an interrupted script for what it is.
///
/// # Arguments
/// * `description` - the engine's description of the error.
///
/// # Returns
/// The description; an interrupt reads as an exceeded budget.
pub(crate) fn explain(description: &str) -> String {
    if description.contains("interrupted") {
        format!("execution budget exceeded ({})", description.trim_end())
    } else {
        description.to_owned()
    }
}

impl<S: Shell> Host for DesktopHost<S> {
    /// HostApi with the areas this host offers.
    type Extensions<V: lunarstorm_event_loop_runtime::vm::Vm> = (HostApi<V, Areas>,);

    /// # Returns
    /// A fresh set for one Runtime. `fs` has no scope of its own: its jobs run in
    /// `ops.desktopWork`, which holds the area roots.
    fn extensions<V: lunarstorm_event_loop_runtime::vm::Vm>(&self) -> Self::Extensions<V> {
        (HostApi::new((Fs,)),)
    }

    /// # Returns
    /// HostApi with the areas the program lists.
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The watchdog reads the pulse to enforce the execution budget.
    const PULSE: bool = true;

    /// # Returns
    /// `process.argv`: the application's id.
    fn args(&self) -> Vec<String> {
        vec![self.name.clone()]
    }

    /// # Returns
    /// The environment the host configuration permits.
    fn environment(&self) -> Vec<(String, String)> {
        self.environment.clone()
    }

    /// # Returns
    /// The process's working directory, or `/` when it has none.
    fn working_directory(&self) -> String {
        std::env::current_dir().map_or_else(|_| String::from("/"), |path| path.to_string_lossy().into_owned())
    }

    /// Sends `console` output to the shell's log.
    ///
    /// # Arguments
    /// * `stream` - stdout is `info`, stderr is `error`.
    /// * `bytes` - what was printed.
    fn write(&self, stream: Stream, bytes: &[u8]) {
        let level = if stream == Stream::Out { "info" } else { "error" };
        self.shell.log(level, String::from_utf8_lossy(bytes).trim_end_matches('\n'));
    }

    /// `process.exit(code)` in the backend ends the application.
    ///
    /// # Arguments
    /// * `code` - the exit status.
    fn exit(&self, code: i32) -> ! {
        std::process::exit(code)
    }

    /// The fs/net policy of a desktop backend: paths under the permitted roots,
    /// and no sockets (the host configuration has no network permission).
    ///
    /// # Arguments
    /// * `access` - what the backend is about to do.
    ///
    /// # Returns
    /// Whether it may.
    fn allow(&self, access: Access<'_>) -> bool {
        match access {
            Access::Read(path) | Access::Write(path) => within(&self.roots, path),
            Access::Listen(_) | Access::Connect(..) => false,
        }
    }

    /// Fills the buffer from the system entropy source.
    ///
    /// # Arguments
    /// * `into` - the buffer to fill completely.
    fn entropy(&self, into: &mut [u8]) {
        for chunk in into.chunks_mut(ENTROPY_CHUNK) {
            // SAFETY: `chunk` is a live, writable slice of at most 256 bytes, the
            // most `getentropy` accepts.
            let filled = unsafe { libc::getentropy(chunk.as_mut_ptr().cast(), chunk.len()) };
            assert!(filled == 0, "the system entropy source failed");
        }
    }

    /// Forwards a Runtime record to the shell.
    ///
    /// # Arguments
    /// * `record` - the record.
    fn log(&self, record: Log<'_>) {
        match record {
            Log::Warning(text) => self.shell.log("warn", text),
            Log::Error(text) => self.shell.error(self.epoch, text),
            Log::Uncaught { description, .. } => self.shell.error(self.epoch, &explain(description)),
        }
    }

    /// Reports a script error and lets the backend go on. While the entry module
    /// is loading, the first error is kept as the reason the load failed.
    ///
    /// # Arguments
    /// * `_in_promise` - whether it is a rejected promise.
    /// * `description` - the error as `console` would show it.
    fn uncaught(&self, _in_promise: bool, description: &str) {
        if self.loading.get() {
            self.failure.borrow_mut().get_or_insert_with(|| explain(description));
        } else {
            self.shell.error(self.epoch, &explain(description));
        }
    }
}

/// Whether a path the backend names lies under one of its roots.
///
/// Defect fixed here: the check was `Path::new(path).starts_with(root)` on the
/// text the script passed, and `starts_with` compares components without
/// resolving them, so `<root>/../outside` and a symlink under a root pointing
/// out of it both passed (tests `a_path_that_climbs_out_of_a_root_is_refused`,
/// `a_symlink_inside_a_root_that_points_out_of_it_is_refused`).
///
/// Decision: resolve the path the way the OS will before comparing. The longest
/// existing ancestor is canonicalised (symlinks and `..` resolved by the OS); the
/// components below it do not exist yet, so they hold no links and are applied
/// lexically. Lexical normalisation alone was rejected: it misses symlinks. The
/// check and the open are separate system calls, so a link swapped in between
/// can still race it; the host grants only directories the app owns.
///
/// # Arguments
/// * `roots` - the permitted roots, already canonical ([`canonical_roots`]).
/// * `path` - the path as the script wrote it; relative to the working directory.
///
/// # Returns
/// Whether the resolved path is a root or lies under one.
pub(crate) fn within(roots: &[PathBuf], path: &str) -> bool {
    let path = Path::new(path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(directory) => directory.join(path),
            Err(_) => return false,
        }
    };
    let mut existing = absolute.as_path();
    let mut rest = Vec::new();
    let mut resolved = loop {
        if let Ok(canonical) = std::fs::canonicalize(existing) {
            break canonical;
        }
        // Defect fixed (D5): a dangling or looping symlink failed to canonicalise,
        // so it was taken for a name not created yet and applied lexically; a write
        // through it then created its target outside the root (test
        // `a_broken_symlink_inside_a_root_is_refused`). Decision: a component that
        // exists but cannot be resolved refuses the path, even when its target
        // would lie inside a root. Following `read_link` was rejected: more code,
        // and the target can be created later anyway.
        if std::fs::symlink_metadata(existing).is_ok() {
            return false;
        }
        let (Some(parent), Some(name)) = (existing.parent(), existing.components().next_back()) else { return false };
        rest.push(name);
        existing = parent;
    };
    for component in rest.into_iter().rev() {
        match component {
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::Normal(name) => resolved.push(name),
            _ => {}
        }
    }
    roots.iter().any(|root| resolved.starts_with(root))
}

/// Canonicalises the permitted roots once per backend, so [`within`] compares
/// resolved paths with resolved roots (`/tmp` is `/private/tmp` on macOS).
///
/// # Arguments
/// * `roots` - the roots from the host configuration.
///
/// # Returns
/// The roots that exist, canonical. A root that does not exist grants nothing:
/// it could later be created as a link to anywhere.
pub(crate) fn canonical_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    roots.into_iter().filter_map(|root| std::fs::canonicalize(root).ok()).collect()
}
