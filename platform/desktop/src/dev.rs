//! Debug runtime machinery (spec 6): the file watcher thread and the compiler thread that
//! runs the embedded pipeline and hands each result to the UI thread, which applies the
//! change table of 6.1.

use crate::pipeline::{run_guarded, BuildOutput, Pipeline};
use crate::ui::{DevStarter, UserEvent};
use crossbeam_channel::{unbounded, Receiver};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::time::Duration;

/// Quiet time after the last file event before a build starts, so one editor save
/// (often several events) gives one build.
const DEBOUNCE: Duration = Duration::from_millis(80);

/// Watches a directory recursively.
///
/// # Arguments
/// * `root` - directory.
///
/// # Returns
/// The watcher (keep it alive) and the change signal.
///
/// # Errors
/// The OS watcher could not start.
fn watch(root: &Path) -> notify::Result<(RecommendedWatcher, Receiver<()>)> {
    let (tx, rx) = unbounded();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        let relevant = matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_))
            && event.paths.iter().any(|p| !ignored(p));
        if relevant {
            let _ = tx.send(());
        }
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok((watcher, rx))
}

/// Paths a build never depends on: hidden files and directories, editor swap and backup
/// files.
///
/// # Arguments
/// * `path` - changed path.
///
/// # Returns
/// `true` when the change is ignored.
fn ignored(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        || path.extension().is_some_and(|e| e == "swp" || e == "tmp")
        || path.to_string_lossy().ends_with('~')
}

/// Waits for a change and its quiet period.
///
/// # Arguments
/// * `rx` - change signal.
///
/// # Returns
/// `false` when the watcher is gone.
fn settle(rx: &Receiver<()>) -> bool {
    if rx.recv().is_err() {
        return false;
    }
    while rx.recv_timeout(DEBOUNCE).is_ok() {}
    true
}

/// Builds until the project compiles, printing errors and waiting for a change between
/// attempts (there is no running app to show `dev.error` in yet).
///
/// # Arguments
/// * `pipeline` - the embedded pipeline.
///
/// # Returns
/// The first good build.
pub fn first_build(pipeline: &mut dyn Pipeline) -> BuildOutput {
    let mut watcher = None;
    loop {
        match run_guarded(pipeline) {
            Ok(build) => return build,
            Err(errors) => {
                for e in errors {
                    eprintln!("zebra: compile error: {}{}", e.file.map(|f| format!("{f}: ")).unwrap_or_default(), e.message);
                }
                eprintln!("zebra: waiting for a change in {}", pipeline.watch_root().display());
                if watcher.is_none() {
                    match watch(pipeline.watch_root()) {
                        Ok(w) => watcher = Some(w),
                        Err(e) => {
                            eprintln!("zebra: cannot watch {}: {e}", pipeline.watch_root().display());
                            std::process::exit(1);
                        }
                    }
                }
                if !settle(&watcher.as_ref().expect("set above").1) {
                    std::process::exit(1);
                }
            }
        }
    }
}

/// Creates the starter the UI thread calls once its event loop runs: it spawns the
/// compiler thread, which watches the project and rebuilds on every change.
///
/// # Arguments
/// * `pipeline` - the embedded pipeline.
///
/// # Returns
/// The starter.
pub fn starter(mut pipeline: Box<dyn Pipeline>) -> DevStarter {
    Box::new(move |proxy| {
        std::thread::Builder::new()
            .name("lunarstorm-compiler".into())
            .spawn(move || {
                let (_watcher, rx) = match watch(pipeline.watch_root()) {
                    Ok(w) => w,
                    Err(e) => {
                        eprintln!("zebra: cannot watch {}: {e}; dev reload is off", pipeline.watch_root().display());
                        return;
                    }
                };
                while settle(&rx) {
                    let result = run_guarded(pipeline.as_mut());
                    if proxy.send_event(UserEvent::Build(Box::new(result))).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn the compiler thread");
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_and_temporary_files_are_ignored() {
        assert!(ignored(Path::new("/p/.git/index")));
        assert!(ignored(Path::new("/p/frontend/main.js.swp")));
        assert!(ignored(Path::new("/p/backend.js~")));
        assert!(!ignored(Path::new("/p/frontend/main.js")));
    }

    #[test]
    fn watcher_reports_a_change() {
        let dir = std::env::temp_dir().join(format!("lunarstorm-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (_w, rx) = watch(&dir).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(dir.join("backend.js"), "x").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok(), "no file event within 5 s");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
