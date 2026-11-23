//! The Tauri plugin: development frontend served from the host's gateway.

use std::path::PathBuf;
use std::sync::Mutex;

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Runtime};

/// Configures the shared transform plugin and optional host-mounted development frontend.
#[derive(Default)]
pub struct Builder {
    root: Option<PathBuf>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Serve `root` (its `index.html` and the source it imports) in development.
    pub fn development(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    /// Install transform IPC and retain the optional frontend until the host exits.
    ///
    /// Setup errors fail plugin initialization; no separate listener or runtime is created here.
    pub fn build<R: Runtime>(self) -> TauriPlugin<R> {
        let root = self.root;
        tauri::plugin::Builder::new("devermind")
            .invoke_handler(tauri::generate_handler![
                crate::transform_commands::transform
            ])
            .setup(move |app, _api| {
                let frontend = match root {
                    Some(root) => start(root)?,
                    None => None,
                };
                app.manage(Frontend(Mutex::new(frontend)));
                Ok(())
            })
            .on_event(|app, event| {
                if let tauri::RunEvent::Exit = event {
                    if let Some(frontend) = app.try_state::<Frontend>() {
                        frontend.0.lock().unwrap().take();
                    }
                }
            })
            .build()
    }
}

#[cfg(feature = "dev-server")]
type Server = devermind_server::Embedded;
#[cfg(not(feature = "dev-server"))]
type Server = std::convert::Infallible;

struct Frontend(Mutex<Option<Server>>);

#[cfg(feature = "dev-server")]
fn start(root: PathBuf) -> Result<Option<Server>, Box<dyn std::error::Error>> {
    let config = devermind_server::Config::new(&root);
    let server = devermind_server::Embedded::start(&config)
        .map_err(|e| format!("devermind cannot serve {}: {e}", root.display()))?;
    eprintln!("[devermind] serving {}", server.state().root.display());
    Ok(Some(server))
}

#[cfg(not(feature = "dev-server"))]
fn start(_root: PathBuf) -> Result<Option<Server>, Box<dyn std::error::Error>> {
    eprintln!("[devermind] built without `dev-server`; development root ignored");
    Ok(None)
}

/// The development frontend's router, or `None` when no development root was
/// set, the `dev-server` feature is off, or the app is exiting.
pub fn development_router<R: Runtime>(app: &AppHandle<R>) -> Option<axum::Router> {
    let frontend = app.try_state::<Frontend>()?;
    let guard = frontend.0.lock().unwrap();
    #[cfg(feature = "dev-server")]
    {
        guard.as_ref().map(|server| server.router())
    }
    #[cfg(not(feature = "dev-server"))]
    {
        guard.as_ref().map(|never| match *never {})
    }
}

#[cfg(all(test, feature = "dev-server"))]
mod tests {
    use super::*;

    #[test]
    fn development_frontend_starts_and_releases_on_exit() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../server/tests/fixtures/app");
        let frontend = Frontend(Mutex::new(start(root.clone()).unwrap()));
        {
            let guard = frontend.0.lock().unwrap();
            let server = guard.as_ref().expect("development frontend started");
            assert_eq!(server.state().root, root.canonicalize().unwrap());
            let _router = server.router();
        }
        // The same transition used by the plugin's Exit event drops the watcher.
        frontend.0.lock().unwrap().take();
        assert!(frontend.0.lock().unwrap().is_none());
    }
}
