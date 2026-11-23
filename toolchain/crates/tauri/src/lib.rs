//! Devermind's Tauri host: permission-controlled source transforms and an optional embedded frontend.
//!
//! [`init`] installs text-only transforms. [`Builder::development`] also prepares the frontend
//! compiler when `dev-server` is enabled. The host mounts [`development_router`] on its own
//! listener; the plugin never binds a second socket. Dropping the frontend on app exit stops
//! its watcher. Transform IPC is restricted by the `devermind:default` capability.

/// Compatibility access to production bundling; new hosts can depend on `devermind_bundler`.
#[cfg(feature = "bundler")]
pub use devermind_bundler as bundle;

#[cfg(feature = "plugin")]
mod frontend_host;
#[cfg(feature = "plugin")]
mod transform_commands;
#[cfg(feature = "plugin")]
pub use frontend_host::{development_router, Builder};
#[cfg(feature = "plugin")]
pub use transform_commands::{
    transform_source, Inject, InjectAll, Options, Replacement, SourceLoader, TransformRequest,
    TransformResult,
};

/// Register the shared plugin without an embedded frontend.
///
/// Window capabilities must grant `devermind:default` or `devermind:allow-transform` before
/// webviews can invoke `plugin:devermind|transform`. No filesystem paths are read by that command.
#[cfg(feature = "plugin")]
pub fn init<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    Builder::new().build()
}
