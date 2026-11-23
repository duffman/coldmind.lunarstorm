//! Generic project development and bundling through the existing toolchain.
//!
//! A [`BuildServer`] owns one project root. Development uses the compiler, module
//! resolver, watcher and HMR in `devtools_server`; bundles use `devtools_bundler`.
//! Builds on the same instance are serialized because the bundler replaces output.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use devtools_bundler::BuildOptions;
pub use devtools_bundler::BuildReport;
use devtools_server::{Config, DevServer};
use tokio::sync::Mutex;

/// Development listener settings. Omitted port preserves project configuration.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    pub port: Option<u16>,
    pub watch: bool,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            port: None,
            watch: true,
        }
    }
}

/// Explicit overrides for a production bundle.
#[derive(Debug, Clone, Default)]
pub struct BundleOptions {
    /// Relative to the effective application root; omitted uses project configuration.
    pub out: Option<PathBuf>,
    pub minify: Option<bool>,
}

/// Project-scoped coordinator for development and production builds.
///
/// The output directory is replaced by the existing bundler; build failures may
/// leave partial output. Use one instance per project to serialize its builds.
/// Dropping this value shuts down its development listener and watcher.
pub struct BuildServer {
    root: PathBuf,
    development: Option<DevServer>,
    building: Mutex<()>,
}

impl BuildServer {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .with_context(|| format!("project root {}", root.as_ref().display()))?;
        if !root.is_dir() {
            bail!("project root {} must be a directory", root.display());
        }
        Ok(Self {
            root,
            development: None,
            building: Mutex::new(()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Starts development serving and returns when the listener is ready.
    pub fn start(&mut self, options: ServeOptions) -> Result<SocketAddr> {
        if self.development.is_some() {
            bail!("development server is already running");
        }
        let mut config = Config::new(&self.root);
        config.watch = options.watch;
        if let Some(port) = options.port {
            config.set_port(port);
        }
        let server = DevServer::start(config)?;
        let address = server.addr();
        self.development = Some(server);
        Ok(address)
    }

    /// Access to the running server's graph, invalidation and application channels.
    pub fn development(&self) -> Option<&DevServer> {
        self.development.as_ref()
    }

    /// Stops the listener and watcher, waiting for their server thread.
    pub fn stop(&mut self) {
        if let Some(server) = self.development.take() {
            server.stop();
        }
    }

    /// Bundles this project using its existing native/Vite configuration support.
    /// Calls on this instance run in order; a failed build releases the lock.
    pub async fn bundle(&self, options: BundleOptions) -> Result<BuildReport> {
        let _building = self.building.lock().await;
        let mut build = BuildOptions::new(&self.root);
        build.out = options.out;
        build.minify = options.minify;
        devtools_bundler::build(&build).await
    }
}
