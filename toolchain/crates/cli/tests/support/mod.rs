//! Shared subprocess harness; each integration-test binary uses a different subset.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEADLINE: Duration = Duration::from_secs(30);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// Isolates parallel tests and removes only their own temporary project on unwind.
pub struct Fixture(pub PathBuf);
impl Fixture {
    pub fn new() -> Self {
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("devtools-cli-{}-{stamp}-{id}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    pub fn write(&self, path: &str, bytes: impl AsRef<[u8]>) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct Completed {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Drains both pipes concurrently, bounds readiness/exit waits, and reaps the
/// child even when an assertion panics. This prevents deadlocks on full stderr
/// pipes and prevents failed network tests from leaving listeners behind.
pub struct Cli {
    child: Child,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    lines: mpsc::Receiver<String>,
    readers: Vec<JoinHandle<()>>,
}
impl Cli {
    pub fn spawn(args: &[&str], no_javascript: bool) -> Self {
        Self::spawn_configured(args, no_javascript, None, None)
    }

    /// Pipes input on an owned thread and closes stdin at EOF, without blocking
    /// stdout/stderr draining or leaving a child waiting for inherited terminal input.
    pub fn spawn_with_input(args: &[&str], no_javascript: bool, input: &[u8]) -> Self {
        Self::spawn_configured(args, no_javascript, Some(input), None)
    }

    pub fn spawn_in(args: &[&str], no_javascript: bool, directory: &Path) -> Self {
        Self::spawn_configured(args, no_javascript, None, Some(directory))
    }

    fn spawn_configured(
        args: &[&str],
        no_javascript: bool,
        input: Option<&[u8]>,
        directory: Option<&Path>,
    ) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jetbunny"));
        command
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if no_javascript {
            command.env("PATH", "");
        }
        if let Some(directory) = directory {
            command.current_dir(directory);
        }
        let mut child = command.spawn().unwrap();
        let writer = input.map(|input| {
            let input = input.to_vec();
            let mut pipe = child.stdin.take().unwrap();
            thread::spawn(move || {
                // Early argument failures may close the pipe before reading;
                // their exit status and diagnostics are asserted by the caller.
                let _ = pipe.write_all(&input);
            })
        });
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let (send, lines) = mpsc::channel();
        let out = stdout.clone();
        let pipe = child.stdout.take().unwrap();
        let out_reader = thread::spawn(move || {
            let mut pipe = BufReader::new(pipe);
            let mut line = Vec::new();
            while let Ok(count) = pipe.read_until(b'\n', &mut line) {
                if count == 0 {
                    break;
                }
                out.lock().unwrap().extend_from_slice(&line);
                let _ = send.send(String::from_utf8_lossy(&line).into_owned());
                line.clear();
            }
        });
        let err = stderr.clone();
        let mut pipe = child.stderr.take().unwrap();
        let err_reader = thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(count) = pipe.read(&mut bytes) {
                if count == 0 {
                    break;
                }
                err.lock().unwrap().extend_from_slice(&bytes[..count]);
            }
        });
        let mut readers = vec![out_reader, err_reader];
        if let Some(writer) = writer {
            readers.push(writer);
        }
        Self {
            child,
            stdout,
            stderr,
            lines,
            readers,
        }
    }
    fn logs(&self) -> String {
        format!(
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&self.stdout.lock().unwrap()),
            String::from_utf8_lossy(&self.stderr.lock().unwrap())
        )
    }
    pub fn ready(&mut self) -> SocketAddr {
        let until = Instant::now() + DEADLINE;
        loop {
            let remaining = until.saturating_duration_since(Instant::now());
            let line = self.lines.recv_timeout(remaining).unwrap_or_else(|error| {
                panic!("CLI did not become ready: {error}; {}", self.logs())
            });
            if let Some((_, url)) = line.split_once("http://") {
                return url
                    .trim()
                    .trim_end_matches('/')
                    .parse()
                    .unwrap_or_else(|error| panic!("bad ready URL {line}: {error}"));
            }
        }
    }
    pub fn finish(mut self) -> Completed {
        let until = Instant::now() + DEADLINE;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < until,
                "CLI exit deadline exceeded; {}",
                self.logs()
            );
            thread::sleep(Duration::from_millis(5));
        };
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
        let stdout = String::from_utf8_lossy(&self.stdout.lock().unwrap()).into_owned();
        let stderr = String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned();
        Completed {
            status,
            stdout,
            stderr,
        }
    }
}
impl Drop for Cli {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

pub struct Response {
    pub headers: String,
    pub body: Vec<u8>,
}
impl Response {
    pub fn text(&self) -> String {
        String::from_utf8(self.body.clone()).unwrap()
    }
}

/// Uses binary-preserving HTTP reads and deadlines; generated images must not
/// accidentally pass via UTF-8 replacement or an HTML SPA fallback response.
pub fn get(address: SocketAddr, path: &str) -> Response {
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
    let mut bytes = Vec::new();
    socket.read_to_end(&mut bytes).unwrap();
    let at = bytes
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .expect("HTTP headers");
    Response {
        headers: String::from_utf8(bytes[..at].to_vec()).unwrap(),
        body: bytes[at + 4..].to_vec(),
    }
}

pub fn assert_success(output: &Completed) {
    assert!(
        output.status.success(),
        "{}\n{}",
        output.stdout,
        output.stderr
    );
}

pub fn project_arg(root: &Path) -> &str {
    root.to_str().unwrap()
}
