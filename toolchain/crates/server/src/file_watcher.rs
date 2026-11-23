//! Connects filesystem change batches to the web server's HMR handler.

use std::path::Path;
use std::sync::Arc;

use notify::{RecursiveMode, Watcher};

use crate::{watch_changes, State};

const IGNORED: &[&str] = &["node_modules", ".git", "target", "dist"];

pub fn start(state: Arc<State>, dir: &Path) -> notify::Result<Box<dyn Watcher + Send>> {
    let (changes, watcher) =
        watch_changes::subscribe(&[(dir.to_path_buf(), RecursiveMode::Recursive)], |path| {
            !path
                .components()
                .any(|component| IGNORED.iter().any(|name| component.as_os_str() == *name))
        })?;
    std::thread::Builder::new()
        .name("devermind-watch".into())
        .spawn(move || {
            while let Ok(change) = changes.recv() {
                match change {
                    Ok(paths) => {
                        crate::hot_reload::on_change(&state, &watch_changes::batch(paths, &changes))
                    }
                    Err(error) => eprintln!("file watcher: {error}"),
                }
            }
        })?;
    Ok(watcher)
}
