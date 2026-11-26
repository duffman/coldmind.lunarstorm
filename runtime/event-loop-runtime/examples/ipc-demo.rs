//! Demo and harness for `lunarstorm:ipc` and `lunarstorm:worker`.
//!
//! The harness embeds a Runtime on one thread and plays the application on the
//! main thread: it posts messages through `Runtime::inbox()` and prints what the
//! program sends back through `Host::ipc_send`. The program is
//! `examples/ipc-demo/main.js`, which starts `worker.js` on a thread of its own.
//!
//! ```text
//! cargo run -p lunarstorm-event-loop-runtime --example ipc-demo
//! ```

use lunarstorm_event_loop_runtime::{Access, Host, Log, Runtime, Source, Stream};
use lunarstorm_engine_quickjs_adapter::QuickJs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// A host with real stdio whose `ipc_send` forwards to the harness.
struct Demo {
    /// Where the program's `send` output goes; shared by the main program and its workers.
    sent: Arc<Mutex<Sender<Vec<u8>>>>,
    /// The directory the program's files are in.
    dir: String,
}

impl Host for Demo {
    /// No extensions.
    type Extensions<V: lunarstorm_event_loop_runtime::vm::Vm> = ();

    /// # Returns
    /// The empty set.
    fn extensions<V: lunarstorm_event_loop_runtime::vm::Vm>(&self) -> Self::Extensions<V> {}

    const IPC: bool = true;
    const WORKERS: bool = true;

    /// # Returns
    /// A factory for a worker's host: same sink and directory, built on the worker's thread.
    fn worker_host(&self) -> Option<Box<dyn FnOnce() -> Self + Send>> {
        let (sent, dir) = (self.sent.clone(), self.dir.clone());
        Some(Box::new(move || Demo { sent, dir }))
    }
    /// # Returns
    /// The harness' `ipc_send` target receives the bytes.
    fn ipc_send(&self, bytes: &[u8]) {
        let _ = self.sent.lock().unwrap().send(bytes.to_vec());
    }
    fn args(&self) -> Vec<String> {
        vec![String::from("ipc-demo")]
    }
    fn environment(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn working_directory(&self) -> String {
        self.dir.clone()
    }
    fn write(&self, stream: Stream, bytes: &[u8]) {
        let _ = match stream {
            Stream::Out => std::io::stdout().write_all(bytes),
            Stream::Err => std::io::stderr().write_all(bytes),
        };
    }
    fn exit(&self, code: i32) -> ! {
        std::process::exit(code)
    }
    fn allow(&self, _access: Access<'_>) -> bool {
        true
    }
    fn entropy(&self, into: &mut [u8]) {
        into.fill(4);
    }
    fn log(&self, record: Log<'_>) {
        eprintln!("[log] {record:?}");
    }
}

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/ipc-demo");
    let (sent, from_program) = channel();
    let host = Demo { sent: Arc::new(Mutex::new(sent)), dir: dir.to_string_lossy().into_owned() };

    let (inbox_tx, inbox_rx) = channel();
    let program = thread::spawn(move || {
        let mut runtime = Runtime::<QuickJs, Demo>::new(host).expect("event loop");
        inbox_tx.send(runtime.inbox()).expect("harness is alive");
        runtime.start().expect("start");
        runtime.load(Source::File(dir.join("main.js"))).expect("load main.js");
        runtime.run()
    });
    let inbox = inbox_rx.recv().expect("inbox");

    // The application side: post a message, wait for the program's answers.
    let ask = |line: &str, answers: usize| {
        println!("host    -> {line}");
        assert!(inbox.post(line.as_bytes().to_vec()), "the program's channel refused the message");
        for _ in 0..answers {
            let reply = from_program.recv_timeout(Duration::from_secs(10)).expect("the program answered");
            println!("program -> {}", String::from_utf8_lossy(&reply));
        }
    };
    let ready = from_program.recv_timeout(Duration::from_secs(10)).expect("the program started");
    println!("program -> {}", String::from_utf8_lossy(&ready));
    ask("hello from the host", 1);
    ask("sum 1 2 3 4 5 6 7 8 9 10", 1);
    ask("sum 100 200 300", 1);
    ask("quit", 1);

    let code = program.join().expect("the program thread ended cleanly");
    println!("program ended with status {code}; the loop went idle because the channel was released");
}
