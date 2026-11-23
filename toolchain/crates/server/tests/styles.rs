//! Real Tailwind HTTP and HMR integration; dependencies are supplied by the integration runner.
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires DEVERMIND_TAILWIND_NODE_MODULES pointing at installed Tailwind packages"]
async fn discovers_new_tailwind_utility_through_stylesheet_hmr() {
    assert_stylesheet_watch(StylesheetEdit::SourceUtility).await;
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires DEVERMIND_TAILWIND_NODE_MODULES pointing at installed Tailwind packages"]
async fn invalidates_cached_stylesheet_after_imported_css_edit() {
    assert_stylesheet_watch(StylesheetEdit::ImportedCss).await;
}

#[cfg(unix)]
enum StylesheetEdit {
    SourceUtility,
    ImportedCss,
}

/// Each scenario owns its server and socket, so an earlier edit cannot supply its HMR frame.
#[cfg(unix)]
async fn assert_stylesheet_watch(edit: StylesheetEdit) {
    use futures_util::StreamExt;
    use std::{fs, time::Duration};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let modules = std::env::var("DEVERMIND_TAILWIND_NODE_MODULES")
        .expect("set DEVERMIND_TAILWIND_NODE_MODULES to a project node_modules containing @tailwindcss/node and @tailwindcss/oxide");
    let modules = std::path::PathBuf::from(modules)
        .canonicalize()
        .expect("Tailwind dependency directory exists");
    for package in ["@tailwindcss/node", "@tailwindcss/oxide"] {
        assert!(
            modules.join(package).exists(),
            "integration prerequisite missing: {}",
            modules.join(package).display()
        );
    }
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(modules, dir.path().join("node_modules")).unwrap();
    fs::create_dir(dir.path().join("styles")).unwrap();
    fs::write(dir.path().join("package.json"), "{}").unwrap();
    fs::write(
        dir.path().join("index.html"),
        "<div class='flex p-4'></div>",
    )
    .unwrap();
    fs::write(
        dir.path().join("main.css"),
        "@import 'tailwindcss'; @import './styles/local.css';",
    )
    .unwrap();
    fs::write(
        dir.path().join("styles/local.css"),
        ".local { background: url('./pixel.svg'); color: red; }",
    )
    .unwrap();
    fs::write(dir.path().join("styles/pixel.svg"), "<svg/>").unwrap();
    fs::write(
        dir.path().join("main.ts"),
        "export const value = 'initial'; import.meta.hot.accept();",
    )
    .unwrap();
    let mut config = devermind_server::Config::new(dir.path());
    config.port = 0;
    config.watch = true;
    let server = devermind_server::DevServer::start(config).unwrap();

    /// Every network operation is bounded, so a missing response cannot hang the suite.
    async fn get(addr: std::net::SocketAddr, path: &str) -> String {
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            stream
                .write_all(
                    format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            response
        })
        .await
        .expect("HTTP request timed out")
    }
    let _ = get(server.addr(), "/main.ts").await;
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(format!(
            "ws://{}{}",
            server.addr(),
            devermind_server::HMR_PATH
        )),
    )
    .await
    .unwrap()
    .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let css = get(server.addr(), "/main.css?import").await;
    assert!(css.starts_with("HTTP/1.1 200"), "{css}");
    assert!(
        css.contains("display: flex") && css.contains(".p-4"),
        "{css}"
    );
    assert!(
        css.contains(".local") && css.contains("color: red") && css.contains("/styles/pixel.svg"),
        "{css}"
    );
    assert!(
        !css.contains("@import") && !css.contains(".text-red-500"),
        "{css}"
    );
    let asset = get(server.addr(), "/styles/pixel.svg").await;
    assert!(
        asset.starts_with("HTTP/1.1 200") && asset.ends_with("<svg/>"),
        "{asset}"
    );

    match &edit {
        StylesheetEdit::SourceUtility => fs::write(
            dir.path().join("main.ts"),
            "export const value = 'text-red-500'; import.meta.hot.accept();",
        ),
        StylesheetEdit::ImportedCss => fs::write(
            dir.path().join("styles/local.css"),
            ".local { background: url('./pixel.svg'); color: blue; }",
        ),
    }
    .unwrap();

    // There is exactly one edit in this fixture. Ignore unrelated frames, but
    // require its stylesheet update rather than accepting a full page reload.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let text = socket
                .next()
                .await
                .expect("HMR socket closed")
                .expect("HMR frame failed")
                .into_text()
                .unwrap();
            let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_ne!(frame["type"], "full-reload", "unexpected reload: {frame}");
            assert_ne!(frame["type"], "error", "unexpected compile error: {frame}");
            if frame["type"] == "update"
                && frame["updates"].as_array().is_some_and(|updates| {
                    updates.iter().any(|update| {
                        update["path"] == "/main.css?import" || update["path"] == "/main.css"
                    })
                })
            {
                break;
            }
        }
    })
    .await
    .expect("no stylesheet HMR update after edit");

    // Reuse the primed cache key: a new timestamp query would force compilation
    // and hide a missing watcher-driven cache invalidation.
    let updated = get(server.addr(), "/main.css?import").await;
    assert!(updated.starts_with("HTTP/1.1 200"), "{updated}");
    match edit {
        StylesheetEdit::SourceUtility => assert!(updated.contains(".text-red-500"), "{updated}"),
        StylesheetEdit::ImportedCss => assert!(
            updated.contains("color: blue")
                && !updated.contains("color: red")
                && updated.contains("/styles/pixel.svg"),
            "{updated}"
        ),
    }
}
