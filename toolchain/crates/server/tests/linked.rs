use devtools_server::{Config, DevServer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn get(server: &DevServer, path: &str) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(server.addr()).await.unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    (
        head.split_whitespace().nth(1).unwrap().parse().unwrap(),
        body.to_string(),
    )
}

#[tokio::test]
async fn graph_resolved_external_sources_are_served_without_exposing_neighbors() {
    let app = tempfile::tempdir().unwrap();
    let linked = tempfile::tempdir().unwrap();
    let external = linked.path().canonicalize().unwrap().join("entry.ts");
    let neighbor = external.with_file_name("secret.txt");
    std::fs::write(
        &external,
        "import {peer} from 'peer-package'; export const value = peer;",
    )
    .unwrap();
    std::fs::write(&neighbor, "must-not-serve").unwrap();
    std::fs::write(
        app.path().join("main.ts"),
        "import {value} from 'linked'; console.log(value);",
    )
    .unwrap();
    std::fs::create_dir_all(app.path().join("node_modules/peer-package")).unwrap();
    std::fs::write(
        app.path().join("node_modules/peer-package/package.json"),
        r#"{"name":"peer-package","type":"module","main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(
        app.path().join("node_modules/peer-package/index.js"),
        "export const peer = 'peer-found';",
    )
    .unwrap();
    let mut config = Config::new(app.path());
    config.port = 0;
    config.watch = false;
    config
        .alias
        .push(("linked".into(), external.to_string_lossy().into_owned()));
    let server = DevServer::start(config).unwrap();
    let external_url = format!("/@fs{}", external.display());
    let neighbor_url = format!("/@fs{}", neighbor.display());
    assert_eq!(get(&server, &external_url).await.0, 403);
    let (status, main) = get(&server, "/main.ts").await;
    assert_eq!(status, 200, "{main}");
    assert!(main.contains(&external_url), "{main}");
    let (status, code) = get(&server, &external_url).await;
    assert_eq!(status, 200, "{code}");
    assert!(code.contains("peer-package/index.js"), "{code}");
    assert_eq!(get(&server, &neighbor_url).await.0, 403);
    assert_eq!(
        get(&server, "/node_modules/peer-package/index.js").await.0,
        200
    );
}
