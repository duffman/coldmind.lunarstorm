//! Native host diagnostics use the existing invalidation pipeline and release open sockets.
use std::sync::{
    atomic::{AtomicU8, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use devermind_server::{Config, DevServer, HMR_PATH};
use futures_util::StreamExt;
use serde_json::Value;

#[tokio::test]
async fn validation_allows_refuses_and_recovers_without_another_watcher() {
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("index.html");
    std::fs::write(&page, "<html><body></body></html>").unwrap();
    let mode = Arc::new(AtomicU8::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut config = Config::new(dir.path());
    config.set_port(0);
    config.watch = false;
    let callback_mode = mode.clone();
    let callback_calls = calls.clone();
    let config = config.with_validation(move |files| {
        assert_eq!(files.len(), 1);
        callback_calls.fetch_add(1, Ordering::Relaxed);
        match callback_mode.load(Ordering::Relaxed) {
            1 => Err("SysApi capability denied".into()),
            2 => Ok(false),
            _ => Ok(true),
        }
    });
    let server = DevServer::start(config).unwrap();
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{}{HMR_PATH}", server.addr()))
            .await
            .unwrap();
    let connected = socket.next().await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(connected.to_text().unwrap()).unwrap()["type"],
        "connected"
    );

    server.invalidate(std::slice::from_ref(&page));
    let allowed = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(allowed.to_text().unwrap()).unwrap()["type"],
        "full-reload"
    );

    mode.store(1, Ordering::Relaxed);
    server.invalidate(std::slice::from_ref(&page));
    let refused = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let refused: Value = serde_json::from_str(refused.to_text().unwrap()).unwrap();
    assert_eq!(refused["type"], "error");
    assert_eq!(refused["err"]["message"], "SysApi capability denied");

    // An unrelated batch must not falsely clear the previous diagnostics failure.
    mode.store(2, Ordering::Relaxed);
    server.invalidate(std::slice::from_ref(&page));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), socket.next())
            .await
            .is_err()
    );

    mode.store(0, Ordering::Relaxed);
    server.invalidate(&[page]);
    let recovered = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let recovered: Value = serde_json::from_str(recovered.to_text().unwrap()).unwrap();
    assert_eq!(recovered["type"], "full-reload");
    assert_eq!(recovered["reason"], "host diagnostics resolved");
    assert_eq!(calls.load(Ordering::Relaxed), 4);
    server.stop();
}

#[tokio::test]
async fn stopping_server_does_not_wait_for_an_open_websocket() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::new(dir.path());
    config.set_port(0);
    config.watch = false;
    let server = DevServer::start(config).unwrap();
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{}{HMR_PATH}", server.addr()))
            .await
            .unwrap();
    socket.next().await.unwrap().unwrap();
    let (done, stopped) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        server.stop();
        let _ = done.send(());
    });
    tokio::time::timeout(Duration::from_secs(2), stopped)
        .await
        .expect("open socket prevented shutdown")
        .unwrap();
    drop(socket);
}
