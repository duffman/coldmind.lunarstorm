//! Binary asset imports return URL modules without decoding the asset as UTF-8.
use devermind_server::{Config, DevServer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn binary_assets_can_be_imported_as_urls_and_fetched_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = b"\x89PNG\r\n\x1a\n\xff\xfe";
    std::fs::write(dir.path().join("portrait.png"), bytes).unwrap();
    let mut config = Config::new(dir.path());
    config.port = 0;
    config.watch = false;
    let server = DevServer::start(config).unwrap();
    for path in [
        "/portrait.png?import",
        "/portrait.png?import&t=2",
        "/portrait.png",
    ] {
        let mut stream = tokio::net::TcpStream::connect(server.addr()).await.unwrap();
        stream
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let offset = response.windows(4).position(|v| v == b"\r\n\r\n").unwrap();
        let headers = String::from_utf8_lossy(&response[..offset]);
        assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
        let body = &response[offset + 4..];
        if path.contains('?') {
            assert!(headers.contains("text/javascript"), "{headers}");
            assert_eq!(
                std::str::from_utf8(body).unwrap(),
                "export default \"/portrait.png\";\n"
            );
        } else {
            assert!(headers.contains("image/png"), "{headers}");
            assert_eq!(body, bytes);
        }
    }
}
