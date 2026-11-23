//! Resolve, download, verify and install one package from a registry on loopback.
#[allow(dead_code)]
mod support;
use base64::{engine::general_purpose::STANDARD, Engine};
use jetpack::{
    install,
    lockfile::{Lockfile, LOCK_NAME},
    manifest::Manifest,
    registry::Registry,
    resolve,
};
use sha2::{Digest, Sha512};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    thread,
};

#[test]
fn resolves_downloads_verifies_and_installs_from_mock_registry() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("package.json"),
        r#"{"name":"test-root","version":"1.0.0","dependencies":{"test-pkg":"^1.0.0"}}"#,
    )
    .unwrap();
    let tarball = make_test_tarball();
    let integrity = format!("sha512-{}", STANDARD.encode(Sha512::digest(&tarball)));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let registry_url = format!("http://127.0.0.1:{}", address.port());
    let metadata = serde_json::to_vec(&serde_json::json!({
        "dist-tags": {"latest": "1.0.0"},
        "versions": {"1.0.0": {
            "version": "1.0.0",
            "dist": {"tarball":format!("{registry_url}/test-pkg.tgz"),"integrity":integrity},
            "bin": {"test-tool": "bin/cli.js"}
        }}
    }))
    .unwrap();
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0_u8; 8192];
            let size = socket.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..size]);
            let path = request.split_whitespace().nth(1).unwrap();
            let bytes = if path == "/test-pkg" {
                &metadata
            } else {
                &tarball
            };
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/octet-stream\r\n\r\n",
                bytes.len()
            );
            socket.write_all(header.as_bytes()).unwrap();
            socket.write_all(bytes).unwrap();
        }
    });

    let manifest = Manifest::open(&tmp.path().join("package.json")).unwrap();
    let mut registry = Registry::new(&registry_url, support::transport()).unwrap();
    let lock = resolve::resolve(&manifest, &mut registry).unwrap();
    assert_eq!(lock.packages["node_modules/test-pkg"].version, "1.0.0");
    install::install(tmp.path(), &lock, &support::Loopback).unwrap();
    assert!(tmp
        .path()
        .join("node_modules/test-pkg/bin/cli.js")
        .is_file());
    #[cfg(unix)]
    assert!(tmp.path().join("node_modules/.bin/test-tool").exists());
    #[cfg(windows)]
    assert!(tmp.path().join("node_modules/.bin/test-tool.cmd").exists());
    lock.write(&tmp.path().join(LOCK_NAME)).unwrap();
    assert_eq!(
        Lockfile::open(&tmp.path().join(LOCK_NAME))
            .unwrap()
            .packages
            .len(),
        1
    );
    server.join().unwrap();
}

fn make_test_tarball() -> Vec<u8> {
    use flate2::{write::GzEncoder, Compression};
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut archive = tar::Builder::new(encoder);
    for (name, content) in [
        (
            "package/package.json",
            b"{\"name\":\"test-pkg\",\"version\":\"1.0.0\"}".as_slice(),
        ),
        (
            "package/bin/cli.js",
            b"#!/usr/bin/env node\nconsole.log('works');\n".as_slice(),
        ),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive.append_data(&mut header, name, content).unwrap();
    }
    archive.into_inner().unwrap().finish().unwrap()
}
