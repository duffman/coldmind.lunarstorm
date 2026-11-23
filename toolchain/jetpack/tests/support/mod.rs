//! Hermetic npm registry fixture. Tests never contact the public npm registry.
use jetpack::transport::{Request, Response, Transport};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// Plain HTTP/1.1 GET over loopback, one connection per request. Test plumbing only:
/// jetpack has no HTTP stack of its own, and the fixtures serve `Connection: close`.
pub struct Loopback;

impl Transport for Loopback {
    fn fetch(&self, batch: Vec<Request>) -> Vec<anyhow::Result<Response>> {
        batch.into_iter().map(get).collect()
    }
}

/// The loopback transport, shared.
pub fn transport() -> Arc<dyn Transport> {
    Arc::new(Loopback)
}

fn get(request: Request) -> anyhow::Result<Response> {
    let url = url::Url::parse(&request.url)?;
    anyhow::ensure!(
        url.scheme() == "http",
        "loopback transport speaks plain HTTP only"
    );
    let host = url.host_str().unwrap_or_default();
    let mut stream = std::net::TcpStream::connect((host, url.port().unwrap_or(80)))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut head = format!(
        "GET {} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n",
        &url[url::Position::BeforePath..]
    );
    if let Some(accept) = request.accept {
        head.push_str(&format!("Accept: {accept}\r\n"));
    }
    if let Some(authorization) = &request.authorization {
        head.push_str(&format!("Authorization: {authorization}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    let mut raw = Vec::new();
    stream
        .take(request.limit + 64 * 1024)
        .read_to_end(&mut raw)?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("no HTTP header"))?;
    let header = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = header.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("bad status line"))?;
    let location = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("location"))
        .map(|(_, value)| value.trim().to_owned());
    let body = raw[split + 4..].to_vec();
    anyhow::ensure!(
        body.len() as u64 <= request.limit,
        "body exceeds {} bytes",
        request.limit
    );
    Ok(Response {
        status,
        location,
        body,
    })
}

pub struct FixtureRegistry {
    pub url: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FixtureRegistry {
    pub fn new(packages: Vec<(&str, Vec<Value>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let mut routes = BTreeMap::new();
        for (name, versions) in packages {
            let mut list = Map::new();
            let mut latest = String::new();
            for version in versions {
                let ver = version["version"].as_str().unwrap().to_string();
                latest.clone_from(&ver);
                list.insert(ver, version);
            }
            routes.insert(
                format!("/{}", name.replace('/', "%2F")),
                serde_json::to_vec(&json!({"dist-tags":{"latest":latest},"versions":list}))
                    .unwrap(),
            );
        }
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let shutdown = stop.clone();
        let thread = thread::spawn(move || {
            while !shutdown.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut client, _)) => {
                        client
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        // Accepted sockets may inherit nonblocking mode on some
                        // platforms. Read a complete bounded request header before
                        // replying, even when TCP splits it into several reads.
                        client.set_nonblocking(false).unwrap();
                        let mut head = Vec::new();
                        let mut buf = [0u8; 4096];
                        while head.len() <= 64 * 1024
                            && !head.windows(4).any(|part| part == b"\r\n\r\n")
                        {
                            match client.read(&mut buf) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => head.extend_from_slice(&buf[..n]),
                            }
                        }
                        if head.len() <= 64 * 1024
                            && head.windows(4).any(|part| part == b"\r\n\r\n")
                        {
                            let request = String::from_utf8_lossy(&head);
                            let uri = request.split_whitespace().nth(1).unwrap_or("/");
                            let body = routes.get(uri);
                            let status = if body.is_some() {
                                "200 OK"
                            } else {
                                "404 Not Found"
                            };
                            let contents = body.map(Vec::as_slice).unwrap_or(b"not found");
                            let header = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n", contents.len());
                            let _ = client.write_all(header.as_bytes());
                            let _ = client.write_all(contents);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("mock registry listener failure: {error}"),
                }
            }
        });
        Self {
            url: format!("http://127.0.0.1:{}", addr.port()),
            stop,
            thread: Some(thread),
        }
    }

    pub fn resolve(&self, root: Value) -> anyhow::Result<jetpack::lockfile::Lockfile> {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("package.json"),
            serde_json::to_vec(&root).unwrap(),
        )
        .unwrap();
        let manifest = jetpack::manifest::Manifest::open(&directory.path().join("package.json"))?;
        let mut registry = jetpack::registry::Registry::new(&self.url, transport())?;
        jetpack::resolve::resolve(&manifest, &mut registry)
    }
}

impl Drop for FixtureRegistry {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

/// Build a registry entry. All tarballs use synthetic URLs: resolution-only
/// tests check graph correctness and require no package downloads.
pub fn release(version: &str, deps: Value, peers: Value) -> Value {
    json!({
        "version":version,
        "dist":{"tarball":format!("https://fixtures.invalid/{version}.tgz"),
                "shasum":"0000000000000000000000000000000000000000"},
        "dependencies":deps,
        "peerDependencies":peers
    })
}

pub fn simple(version: &str) -> Value {
    release(version, json!({}), json!({}))
}
