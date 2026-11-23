//! Listener-independent filesystem batches used by the web and desktop dev hosts.

use notify::{Config, PollWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub(crate) type Change = notify::Result<Vec<PathBuf>>;
pub(crate) type Subscription = (mpsc::Receiver<Change>, Box<dyn Watcher + Send>);

pub(crate) fn subscribe(
    roots: &[(PathBuf, RecursiveMode)],
    relevant: impl Fn(&Path) -> bool + Send + Sync + 'static,
) -> notify::Result<Subscription> {
    let (sender, receiver) = mpsc::channel();
    let relevant = std::sync::Arc::new(relevant);
    let handler = || {
        let sender = sender.clone();
        let relevant = relevant.clone();
        move |event: notify::Result<notify::Event>| {
            let paths = event.map(|event| {
                if event.kind.is_access() {
                    return Vec::new();
                }
                event
                    .paths
                    .into_iter()
                    .filter(|path| relevant(path))
                    .collect()
            });
            if !paths.as_ref().is_ok_and(Vec::is_empty) {
                let _ = sender.send(paths);
            }
        }
    };
    let watch_roots = |watcher: &mut dyn Watcher| {
        for (root, mode) in roots {
            watcher.watch(root, *mode)?;
        }
        Ok::<(), notify::Error>(())
    };
    let native = notify::recommended_watcher(handler()).and_then(|mut watcher| {
        watch_roots(&mut watcher)?;
        Ok(watcher)
    });
    if let Ok(watcher) = native {
        return Ok((receiver, Box::new(watcher)));
    }
    let mut watcher = PollWatcher::new(
        handler(),
        Config::default()
            .with_poll_interval(Duration::from_millis(250))
            .with_compare_contents(true),
    )?;
    watch_roots(&mut watcher)?;
    Ok((receiver, Box::new(watcher)))
}

pub(crate) fn batch(mut paths: Vec<PathBuf>, changes: &mpsc::Receiver<Change>) -> Vec<PathBuf> {
    // Editors can write and rename a file in several operations. Bound the wait
    // so sustained changes still reach the compiler.
    let deadline = Instant::now() + Duration::from_millis(50);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match changes.recv_timeout(remaining) {
            Ok(Ok(more)) => paths.extend(more),
            Ok(Err(error)) => eprintln!("file watcher: {error}"),
            Err(_) => break,
        }
    }
    paths.sort_unstable();
    paths.dedup();
    paths
}
