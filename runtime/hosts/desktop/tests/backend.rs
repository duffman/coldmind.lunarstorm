//! The desktop host without a desktop: a shell that records what the backend
//! does, driving a real Runtime through `load`, `dispatch` and `settle`.

use lunarstorm_host_desktop::{Backend, Program, Shell};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A shell that writes every call as one line into a channel.
struct Recorder(Mutex<Sender<String>>);

impl Recorder {
    /// Records one line.
    ///
    /// # Arguments
    /// * `line` - what the backend did.
    fn push(&self, line: String) {
        let _ = self.0.lock().expect("recorder lock").send(line);
    }
}

impl Shell for Recorder {
    /// Records `loaded <epoch> <error>`.
    ///
    /// # Arguments
    /// * `epoch` - the epoch.
    /// * `error` - the load error.
    fn loaded(&self, epoch: u64, error: Option<&str>) {
        self.push(format!("loaded {epoch} {}", error.unwrap_or("ok")));
    }

    /// Records `send <epoch> <envelope>`.
    ///
    /// # Arguments
    /// * `epoch` - the epoch.
    /// * `envelope` - the envelope.
    fn send(&self, epoch: u64, envelope: &str) {
        self.push(format!("send {epoch} {envelope}"));
    }

    /// Records `native <epoch> <call> <namespace>.<method> <arguments>`.
    ///
    /// # Arguments
    /// * `epoch` - the epoch.
    /// * `call` - the call id.
    /// * `namespace` - the service.
    /// * `method` - the method.
    /// * `arguments` - the arguments as JSON.
    fn native(&self, epoch: u64, call: u64, namespace: &str, method: &str, arguments: &str) {
        self.push(format!("native {epoch} {call} {namespace}.{method} {arguments}"));
    }

    /// Records `error <epoch> <first line>`.
    ///
    /// # Arguments
    /// * `epoch` - the epoch.
    /// * `message` - the description.
    fn error(&self, epoch: u64, message: &str) {
        self.push(format!("error {epoch} {}", message.lines().next().unwrap_or_default()));
    }

    /// Records `log <level> <message>`.
    ///
    /// # Arguments
    /// * `level` - the level.
    /// * `message` - the text.
    fn log(&self, level: &str, message: &str) {
        self.push(format!("log {level} {message}"));
    }

    /// Records `retired <epoch>`.
    ///
    /// # Arguments
    /// * `epoch` - the epoch.
    fn retired(&self, epoch: u64) {
        self.push(format!("retired {epoch}"));
    }
}

/// The backend under test: a handler for each behaviour.
const BACKEND: &str = "\
import { serve, setProperty, restored } from '@desktop/bridge';\n\
import { native } from '@desktop/api';\n\
serve(async (message) => {\n\
  const set = (value) => setProperty(message.form, 'Label', 'caption', value);\n\
  switch (message.event) {\n\
    case 'onClick': set('clicked ' + message.payload.n); setProperty(message.form, 'Label', 'enabled', false); break;\n\
    case 'onCloseQuery': return { cancel: true };\n\
    case 'onOpen': set(await native.dialog.open('title', 2)); break;\n\
    case 'onFail': try { await native.dialog.open(); } catch (error) { set(error.message); } break;\n\
    case 'onSpin': for (;;) {}\n\
    case 'onLater': setTimeout(() => { throw new Error('thrown later'); }, 0); break;\n\
    case 'onThrow': throw new Error('handler failed');\n\
    case 'onLog': console.log('restored', restored('Main').Label.caption); break;\n\
  }\n\
});\n";

/// Builds a program.
///
/// # Arguments
/// * `epoch` - its epoch.
/// * `code` - the backend module.
///
/// # Returns
/// The program, with a 100 ms budget.
fn program(epoch: u64, code: &str) -> Program {
    Program {
        epoch,
        code: Arc::from(code),
        restore: String::from("{\"Main\":{\"Label\":{\"caption\":\"kept\"}}}"),
        memory: Some(64 << 20),
        budget: Duration::from_millis(100),
        roots: Vec::new(),
        environment: Vec::new(),
        name: String::from("test.app"),
    }
}

/// Starts a backend thread with a recording shell.
///
/// # Returns
/// The backend and the recorded lines.
fn backend() -> (Backend<Recorder>, Receiver<String>) {
    let (sender, lines) = channel();
    (Backend::spawn(Arc::new(Recorder(Mutex::new(sender)))), lines)
}

/// # Arguments
/// * `lines` - the recorded lines.
///
/// # Returns
/// The next line; fails the test when none arrives in ten seconds.
fn next(lines: &Receiver<String>) -> String {
    lines.recv_timeout(Duration::from_secs(10)).expect("the backend did nothing for ten seconds")
}

/// # Arguments
/// * `method` - `Target.event`.
/// * `kind` - `event` or `request`.
/// * `payload` - the payload as JSON.
///
/// # Returns
/// An envelope for the form `Main`.
fn envelope(method: &str, kind: &str, payload: &str) -> String {
    format!("{{\"kind\":\"{kind}\",\"id\":7,\"windowId\":\"Main\",\"method\":\"{method}\",\"payload\":{payload}}}")
}

#[test]
fn events_update_properties_and_requests_are_answered() {
    let (backend, lines) = backend();
    backend.load(program(1, BACKEND));
    assert_eq!(next(&lines), "loaded 1 ok");

    backend.dispatch(1, envelope("Button.onClick", "event", "{\"n\":1}"));
    // Two writes in one handler are one envelope.
    assert_eq!(
        next(&lines),
        "send 1 {\"kind\":\"update\",\"windowId\":\"Main\",\"method\":\"update\",\"payload\":{\"controls\":{\"Label\":{\"caption\":\"clicked 1\",\"enabled\":false}}}}"
    );
    backend.dispatch(1, envelope("Main.onCloseQuery", "request", "{}"));
    assert_eq!(
        next(&lines),
        "send 1 {\"kind\":\"response\",\"id\":7,\"windowId\":\"Main\",\"method\":\"Main.onCloseQuery\",\"payload\":{\"cancel\":true}}"
    );
    backend.dispatch(1, envelope("Main.onLog", "event", "{}"));
    assert_eq!(next(&lines), "log info restored kept");
}

#[test]
fn native_calls_settle_from_another_thread() {
    let (backend, lines) = backend();
    backend.load(program(1, BACKEND));
    assert_eq!(next(&lines), "loaded 1 ok");
    let settler = backend.settler();

    backend.dispatch(1, envelope("Button.onOpen", "event", "{}"));
    assert_eq!(next(&lines), "native 1 1 dialog.open [\"title\",2]");
    let remote = settler.clone();
    std::thread::spawn(move || remote.settle(1, 1, Ok(String::from("\"/tmp/a \\\"file\\\"\"")))).join().expect("settle thread");
    assert!(next(&lines).contains("\"caption\":\"/tmp/a \\\"file\\\"\""));

    backend.dispatch(1, envelope("Button.onFail", "event", "{}"));
    assert_eq!(next(&lines), "native 1 2 dialog.open []");
    settler.settle(1, 2, Err(String::from("not \"permitted\"\nhere")));
    assert!(next(&lines).contains("\"caption\":\"not \\\"permitted\\\"\\nhere\""));

    // A result for another epoch is dropped: the next line is the next event's.
    settler.settle(9, 1, Ok(String::from("1")));
    backend.dispatch(1, envelope("Button.onClick", "event", "{\"n\":2}"));
    assert!(next(&lines).contains("clicked 2"));
}

#[test]
fn script_errors_are_reported_and_the_backend_goes_on() {
    let (backend, lines) = backend();
    backend.load(program(1, BACKEND));
    assert_eq!(next(&lines), "loaded 1 ok");

    backend.dispatch(1, envelope("Button.onThrow", "event", "{}"));
    assert_eq!(next(&lines), "error 1 Error: handler failed");
    backend.dispatch(1, envelope("Button.onLater", "event", "{}"));
    assert!(next(&lines).starts_with("error 1 ") , "a throw in a timer is reported");
    // An endless handler is interrupted at its budget.
    backend.dispatch(1, envelope("Button.onSpin", "event", "{}"));
    let line = next(&lines);
    assert!(line.starts_with("error 1 execution budget exceeded"), "{line}");

    backend.dispatch(1, envelope("Button.onClick", "event", "{\"n\":3}"));
    assert!(next(&lines).contains("clicked 3"));
}

#[test]
fn a_backend_that_does_not_load_reports_why() {
    let (backend, lines) = backend();
    backend.load(program(1, "let = = 1;"));
    let line = next(&lines);
    assert!(line.starts_with("loaded 1 ") && line.contains("SyntaxError"), "{line}");
    backend.load(program(2, "import '@desktop/bridge';\nthrow new Error('top level');"));
    let line = next(&lines);
    assert!(line.starts_with("loaded 2 ") && line.contains("top level"), "{line}");
    // The thread is still usable.
    backend.load(program(3, BACKEND));
    assert_eq!(next(&lines), "loaded 3 ok");
}

/// Pins the batching of property writes: everything one handler writes before it
/// awaits goes out as one envelope per form, forms in first-write order; a write
/// after the `await` is a turn of its own and so an envelope of its own.
#[test]
fn property_writes_are_one_envelope_per_form_per_turn() {
    let code = "\
import { serve, setProperty } from '@desktop/bridge';\n\
serve(async () => {\n\
  setProperty('Main', 'Label', 'caption', 'a');\n\
  setProperty('Other', 'Edit', 'text', 'b');\n\
  setProperty('Main', 'Label', 'caption', 'c');\n\
  setProperty('Main', 'Button', 'enabled', false);\n\
  setProperty('Other', 'Edit', 'color', [1, 2]);\n\
  await null;\n\
  setProperty('Main', 'Label', 'caption', 'later');\n\
});\n";
    let (backend, lines) = backend();
    backend.load(program(1, code));
    assert_eq!(next(&lines), "loaded 1 ok");

    backend.dispatch(1, envelope("Button.onClick", "event", "{}"));
    assert_eq!(
        next(&lines),
        "send 1 {\"kind\":\"update\",\"windowId\":\"Main\",\"method\":\"update\",\"payload\":{\"controls\":{\"Label\":{\"caption\":\"c\"},\"Button\":{\"enabled\":false}}}}"
    );
    assert_eq!(
        next(&lines),
        "send 1 {\"kind\":\"update\",\"windowId\":\"Other\",\"method\":\"update\",\"payload\":{\"controls\":{\"Edit\":{\"text\":\"b\",\"color\":[1,2]}}}}"
    );
    assert_eq!(
        next(&lines),
        "send 1 {\"kind\":\"update\",\"windowId\":\"Main\",\"method\":\"update\",\"payload\":{\"controls\":{\"Label\":{\"caption\":\"later\"}}}}"
    );
    // Nothing else was sent: the next line is the reload's.
    backend.load(program(2, code));
    assert_eq!(next(&lines), "retired 1");
}

#[test]
fn a_reload_retires_the_old_epoch_and_drops_its_messages() {
    let (backend, lines) = backend();
    backend.load(program(1, BACKEND));
    assert_eq!(next(&lines), "loaded 1 ok");
    backend.load(program(2, BACKEND));
    assert_eq!(next(&lines), "retired 1");
    assert_eq!(next(&lines), "loaded 2 ok");

    backend.dispatch(1, envelope("Button.onClick", "event", "{\"n\":1}"));
    backend.dispatch(2, envelope("Button.onClick", "event", "{\"n\":2}"));
    let line = next(&lines);
    assert!(line.starts_with("send 2 ") && line.contains("clicked 2"), "{line}");
    drop(backend);
    assert_eq!(next(&lines), "retired 2");
}
