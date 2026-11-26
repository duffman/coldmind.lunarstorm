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
    /// Filesystem roots the backend may touch.
    pub(crate) roots: Vec<PathBuf>,
    /// `process.env`.
    pub(crate) environment: Vec<(String, String)>,
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
    /// No extensions.
    type Extensions<V: lunarstorm_event_loop_runtime::vm::Vm> = ();

    /// # Returns
    /// The empty set.
    fn extensions<V: lunarstorm_event_loop_runtime::vm::Vm>(&self) -> Self::Extensions<V> {}

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
            Access::Read(path) | Access::Write(path) => self.roots.iter().any(|root| Path::new(path).starts_with(root)),
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
