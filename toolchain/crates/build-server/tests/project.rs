use ls_build_server::{BuildServer, BundleOptions, ServeOptions};
use std::{
    io::{Read, Write},
    net::TcpStream,
    path::Path,
    time::Duration,
};

fn project() -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join("src")).unwrap();
    std::fs::create_dir(project.path().join("public")).unwrap();
    std::fs::write(project.path().join("index.html"), "<!doctype html><html><head></head><body><script type=\"module\" src=\"/src/main.ts\"></script></body></html>").unwrap();
    std::fs::write(
        project.path().join("src/message.ts"),
        "export const message: string = 'toolchain-build-server';",
    )
    .unwrap();
    std::fs::write(
        project.path().join("src/main.ts"),
        "import { message } from './message'; import './style.css'; console.log(message);",
    )
    .unwrap();
    std::fs::write(
        project.path().join("src/style.css"),
        "body { color: navy; }",
    )
    .unwrap();
    std::fs::write(project.path().join("public/robots.txt"), "User-agent: *\n").unwrap();
    project
}

fn get(address: std::net::SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    response
}

#[test]
fn development_serves_and_compiles_then_stops_and_restarts() {
    let project = project();
    let mut build_server = BuildServer::new(project.path()).unwrap();
    let options = ServeOptions {
        port: Some(0),
        watch: false,
    };
    let address = build_server.start(options.clone()).unwrap();
    assert!(get(address, "/").contains("/src/main.ts"));
    let module = get(address, "/src/message.ts");
    assert!(module.contains("toolchain-build-server"), "{module}");
    assert!(
        !module.contains(": string"),
        "TypeScript annotation was not compiled: {module}"
    );
    assert!(get(address, "/robots.txt").contains("User-agent: *"));
    assert!(build_server.start(options.clone()).is_err());
    build_server.stop();
    assert!(build_server.development().is_none());
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err());
    let restarted = build_server.start(options).unwrap();
    assert!(get(restarted, "/").contains("/src/main.ts"));
}

#[tokio::test]
async fn bundle_emits_linked_js_css_and_public_assets() {
    let project = project();
    let server = BuildServer::new(project.path()).unwrap();
    let report = server
        .bundle(BundleOptions {
            out: Some("site".into()),
            minify: Some(false),
        })
        .await
        .unwrap();
    assert_eq!(
        report.out,
        project.path().canonicalize().unwrap().join("site")
    );
    assert_eq!(report.entries.len(), 1);
    let html = std::fs::read_to_string(report.out.join("index.html")).unwrap();
    let javascript = std::fs::read_to_string(report.out.join(&report.entries[0].1)).unwrap();
    assert!(html.contains(&report.entries[0].1), "{html}");
    assert!(html.contains(report.stylesheet.as_ref().unwrap()), "{html}");
    assert!(
        javascript.contains("toolchain-build-server"),
        "{javascript}"
    );
    assert!(!javascript.contains(": string"), "{javascript}");
    assert_eq!(
        std::fs::read_to_string(report.out.join("robots.txt")).unwrap(),
        "User-agent: *\n"
    );
}

#[tokio::test]
async fn failed_build_releases_lock_and_unsafe_output_is_rejected() {
    let project = project();
    let server = BuildServer::new(project.path()).unwrap();
    std::fs::write(project.path().join("src/main.ts"), "const broken = ;").unwrap();
    assert!(server.bundle(BundleOptions::default()).await.is_err());
    std::fs::write(
        project.path().join("src/main.ts"),
        "console.log('recovered');",
    )
    .unwrap();
    let report = server.bundle(BundleOptions::default()).await.unwrap();
    assert!(report.out.join("index.html").is_file());
    assert!(server
        .bundle(BundleOptions {
            out: Some(".".into()),
            minify: None
        })
        .await
        .is_err());
    assert!(project.path().join("index.html").is_file());
}

#[test]
fn invalid_project_and_cli_arguments_fail() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, "content").unwrap();
    assert!(BuildServer::new(&file).is_err());
    assert!(BuildServer::new(dir.path().join("missing")).is_err());
    let binary = Path::new(env!("CARGO_BIN_EXE_ls-build-server"));
    assert!(std::process::Command::new(binary)
        .arg("--help")
        .status()
        .unwrap()
        .success());
    let invalid = std::process::Command::new(binary)
        .args(["serve", "--port", "invalid"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("--port"));
}

#[tokio::test]
async fn concurrent_bundles_on_one_server_are_serialized() {
    let project = project();
    let server = BuildServer::new(project.path()).unwrap();
    let (first, second) = tokio::join!(
        server.bundle(BundleOptions::default()),
        server.bundle(BundleOptions::default())
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.out, second.out);
    assert!(second.out.join("index.html").is_file());
    assert!(second.out.join(&second.entries[0].1).is_file());
}
