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
        area_roots: Vec::new(),
        environment: Vec::new(),
        name: String::from("test.app"),
        areas: Vec::new(),
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

/// A backend that reads the path an event names and shows the text, or the
/// error code, in `Label.caption`.
const READER: &str = "\
import { serve, setProperty } from '@desktop/bridge';\n\
import { readFileSync } from 'node:fs';\n\
serve((message) => {\n\
  let shown;\n\
  try { shown = readFileSync(message.payload.path, 'utf8'); } catch (error) { shown = error.code; }\n\
  setProperty(message.form, 'Label', 'caption', shown);\n\
});\n";

/// Creates an empty directory unique to one test under Cargo's temp directory.
///
/// # Arguments
/// * `name` - the test's name.
///
/// # Returns
/// The directory.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("host-desktop").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the test directory");
    dir
}

#[test]
fn a_path_that_climbs_out_of_a_root_is_refused() {
    let base = scratch("climb");
    let root = base.join("root");
    std::fs::create_dir_all(&root).expect("create the root");
    std::fs::write(root.join("inside.txt"), "inside").expect("write inside");
    std::fs::write(base.join("outside.txt"), "outside").expect("write outside");
    let (backend, lines) = backend();
    let mut program = program(1, READER);
    program.roots = vec![root.clone()];
    backend.load(program);
    assert_eq!(next(&lines), "loaded 1 ok");

    let read = |path: std::path::PathBuf| {
        backend.dispatch(1, envelope("Button.onRead", "event", &format!("{{\"path\":{:?}}}", path.to_string_lossy())));
        next(&lines)
    };
    assert!(read(root.join("inside.txt")).contains("\"caption\":\"inside\""), "a file under the root is readable");
    let climbed = read(root.join("..").join("outside.txt"));
    assert!(climbed.contains("\"caption\":\"EACCES\""), "`<root>/../outside.txt` must be refused: {climbed}");
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_a_root_that_points_out_of_it_is_refused() {
    let base = scratch("symlink");
    let root = base.join("root");
    std::fs::create_dir_all(&root).expect("create the root");
    std::fs::write(base.join("outside.txt"), "outside").expect("write outside");
    std::os::unix::fs::symlink(base.join("outside.txt"), root.join("link.txt")).expect("create the link");
    let (backend, lines) = backend();
    let mut program = program(1, READER);
    program.roots = vec![root.clone()];
    backend.load(program);
    assert_eq!(next(&lines), "loaded 1 ok");

    backend.dispatch(1, envelope("Button.onRead", "event", &format!("{{\"path\":{:?}}}", root.join("link.txt").to_string_lossy())));
    let line = next(&lines);
    assert!(line.contains("\"caption\":\"EACCES\""), "a link out of the root must be refused: {line}");
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

/// # Arguments
/// * `id` - the call's id.
/// * `method` - `<area>.<function>`.
/// * `args` - the arguments as a JSON array.
///
/// # Returns
/// A WebView `call` envelope from the form `Main`.
fn call(id: u64, method: &str, args: &str) -> String {
    format!("{{\"kind\":\"call\",\"id\":{id},\"windowId\":\"Main\",\"method\":\"{method}\",\"payload\":{{\"args\":{args}}}}}")
}

/// # Arguments
/// * `id` - the call's id.
/// * `method` - `<area>.<function>`.
/// * `payload` - the expected payload as JSON.
///
/// # Returns
/// The line the shell records for the `result` of that call in epoch 1.
fn result(id: u64, method: &str, payload: &str) -> String {
    format!("send 1 {{\"kind\":\"result\",\"id\":{id},\"windowId\":\"Main\",\"method\":\"{method}\",\"payload\":{payload}}}")
}

#[test]
fn webview_calls_reach_the_fs_area_through_the_runtime_without_backend_code() {
    let base = scratch("calls");
    let root = base.join("root");
    std::fs::create_dir_all(&root).expect("create the root");
    std::fs::write(base.join("outside.txt"), "outside").expect("write outside");
    let (backend, lines) = backend();
    let mut program = program(1, "export {};\n");
    program.area_roots = vec![root.clone()];
    program.areas = vec![String::from("fs")];
    backend.load(program);
    assert_eq!(next(&lines), "loaded 1 ok");

    let ask = |id: u64, method: &str, args: &str| {
        backend.dispatch(1, call(id, method, args));
        next(&lines)
    };
    assert_eq!(ask(1, "fs.writeText", "[\"out.txt\",\"got:hi\"]"), result(1, "fs.writeText", "{\"ok\":true,\"value\":null}"));
    assert_eq!(std::fs::read_to_string(root.join("out.txt")).expect("written"), "got:hi");
    assert_eq!(ask(2, "fs.readText", "[\"out.txt\"]"), result(2, "fs.readText", "{\"ok\":true,\"value\":\"got:hi\"}"));
    assert_eq!(ask(3, "fs.list", "[\".\"]"), result(3, "fs.list", "{\"ok\":true,\"value\":[\"out.txt\"]}"));
    let refused = |line: String, code: &str| assert!(line.contains(&format!("\"ok\":false,\"error\":{{\"code\":\"{code}\"")), "expected {code}: {line}");
    refused(ask(4, "fs.readText", "[\"../outside.txt\"]"), "EACCES");
    refused(ask(5, "fs.writeText", "[\"../outside.txt\",\"x\"]"), "EACCES");
    assert_eq!(std::fs::read_to_string(base.join("outside.txt")).expect("outside"), "outside", "nothing outside the root is written");
    refused(ask(6, "fs.readText", "[\"missing.txt\"]"), "ENOENT");
    refused(ask(7, "fs.readText", "[1]"), "EINVAL");
    refused(ask(8, "fs.readText", "[]"), "EINVAL");
    refused(ask(9, "fs.nope", "[]"), "EINVAL");
    refused(ask(10, "fs.readText.x", "[\"a\"]"), "EINVAL");
}

#[test]
fn an_area_the_app_is_not_permitted_is_refused_and_does_not_resolve() {
    let (backend, lines) = backend();
    backend.load(program(1, "export {};\n"));
    assert_eq!(next(&lines), "loaded 1 ok");
    backend.dispatch(1, call(1, "fs.readText", "[\"a.txt\"]"));
    let line = next(&lines);
    assert!(line.contains("\"ok\":false,\"error\":{\"code\":\"EPERM\""), "{line}");

    // Backend code cannot import it either.
    backend.load(program(2, "import { readText } from 'lunarstorm:hostapi/fs';\n"));
    assert_eq!(next(&lines), "retired 1");
    let line = next(&lines);
    assert!(line.starts_with("loaded 2 ") && !line.ends_with(" ok"), "{line}");

    // Permitted, the area is listed and importable.
    let mut program = program(3, "import { areas } from 'lunarstorm:hostapi';\nimport { readText } from 'lunarstorm:hostapi/fs';\nconsole.log(areas.join(), typeof readText);\n");
    program.areas = vec![String::from("fs")];
    backend.load(program);
    // The Runtime flushes stdout at the end of the loop turn, after the load.
    assert_eq!(next(&lines), "loaded 3 ok");
    assert_eq!(next(&lines), "log info fs function");
}

#[test]
fn the_spec_lists_the_fs_area() {
    let spec = lunarstorm_host_desktop::spec();
    let area = &spec["areas"][0];
    assert_eq!(area["name"], "fs");
    assert_eq!(area["module"], "lunarstorm:hostapi/fs");
    let names: Vec<_> = area["functions"].as_array().expect("functions").iter().map(|f| f["name"].as_str().expect("name").to_owned()).collect();
    assert_eq!(names, ["readText", "writeText", "list"]);
}

/// A backend that writes `x` to the path an event names and shows `ok`, or the
/// error code, in `Label.caption`.
const WRITER: &str = "\
import { serve, setProperty } from '@desktop/bridge';\n\
import { writeFileSync } from 'node:fs';\n\
serve((message) => {\n\
  let shown = 'ok';\n\
  try { writeFileSync(message.payload.path, 'x'); } catch (error) { shown = error.code; }\n\
  setProperty(message.form, 'Label', 'caption', shown);\n\
});\n";

/// Starts a backend whose `node:fs` and `fs` area are both scoped to `root`.
///
/// # Arguments
/// * `code` - the backend module.
/// * `root` - the one root.
///
/// # Returns
/// The backend and its recorded lines, after `loaded 1 ok`.
fn rooted(code: &str, root: &std::path::Path) -> (Backend<Recorder>, Receiver<String>) {
    let (backend, lines) = backend();
    let mut program = program(1, code);
    program.roots = vec![root.to_path_buf()];
    program.area_roots = vec![root.to_path_buf()];
    program.areas = vec![String::from("fs")];
    backend.load(program);
    assert_eq!(next(&lines), "loaded 1 ok");
    (backend, lines)
}

/// Asserts that a recorded `result` line failed with `code`.
///
/// # Arguments
/// * `line` - the line.
/// * `code` - the expected error code.
/// * `what` - what was attempted, for the message.
fn refused_with(line: &str, code: &str, what: &str) {
    let shown: String = line.chars().take(300).collect();
    assert!(line.contains(&format!("\"ok\":false,\"error\":{{\"code\":\"{code}\"")), "{what}: expected {code}, got {shown}");
}

#[cfg(unix)]
#[test]
fn a_broken_symlink_inside_a_root_is_refused() {
    let base = scratch("broken-link");
    let root = base.join("root");
    std::fs::create_dir_all(&root).expect("create the root");
    std::os::unix::fs::symlink(base.join("new"), root.join("link")).expect("create the link");
    let (backend, lines) = rooted(WRITER, &root);

    backend.dispatch(1, envelope("Button.onWrite", "event", &format!("{{\"path\":{:?}}}", root.join("link").to_string_lossy())));
    let line = next(&lines);
    assert!(line.contains("\"caption\":\"EACCES\""), "node:fs through a dangling link must be refused: {line}");
    backend.dispatch(1, call(1, "fs.writeText", "[\"link\",\"x\"]"));
    refused_with(&next(&lines), "EACCES", "fs.writeText through a dangling link");
    assert!(!base.join("new").exists(), "nothing was created outside the root");
}

#[cfg(unix)]
#[test]
fn a_hard_link_inside_a_root_is_refused() {
    let base = scratch("hard-link");
    let root = base.join("root");
    std::fs::create_dir_all(&root).expect("create the root");
    std::fs::write(base.join("secret"), "secret").expect("write the secret");
    std::fs::hard_link(base.join("secret"), root.join("hard")).expect("create the hard link");
    let (backend, lines) = rooted("export {};\n", &root);

    backend.dispatch(1, call(1, "fs.readText", "[\"hard\"]"));
    refused_with(&next(&lines), "EACCES", "fs.readText of a hard link");
    backend.dispatch(1, call(2, "fs.writeText", "[\"hard\",\"x\"]"));
    refused_with(&next(&lines), "EACCES", "fs.writeText of a hard link");
    assert_eq!(std::fs::read_to_string(base.join("secret")).expect("secret"), "secret", "the outside file is unchanged");
}

/// Opens a FIFO for writing without blocking when dropped, so a reader blocked in
/// `open` is released and a failing test ends instead of hanging.
#[cfg(unix)]
struct Release(std::path::PathBuf);

#[cfg(unix)]
impl Drop for Release {
    /// Opens and closes the write end; `ENXIO` (no reader) is ignored.
    fn drop(&mut self) {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&self.0);
    }
}

#[cfg(unix)]
#[test]
fn a_fifo_in_a_root_does_not_block_the_backend() {
    let base = scratch("fifo");
    let root = base.join("root");
    std::fs::create_dir_all(&root).expect("create the root");
    let made = std::process::Command::new("mkfifo").arg(root.join("pipe")).status().expect("run mkfifo");
    assert!(made.success(), "mkfifo failed");
    let (backend, lines) = rooted("export {};\n", &root);
    // Declared after the backend, so it is dropped first.
    let _release = Release(root.join("pipe"));

    backend.dispatch(1, call(1, "fs.readText", "[\"pipe\"]"));
    backend.dispatch(1, call(2, "fs.list", "[\".\"]"));
    let first = lines.recv_timeout(Duration::from_secs(10));
    assert!(first.is_ok(), "the backend blocked on a FIFO: no result within 10 s");
    let second = next(&lines);
    let mut both = [first.unwrap_or_default(), second];
    both.sort();
    refused_with(&both[0], "EINVAL", "fs.readText of a FIFO");
    assert_eq!(both[1], result(2, "fs.list", "{\"ok\":true,\"value\":[\"pipe\"]}"));
}

#[test]
fn a_file_over_the_text_limit_is_refused() {
    let base = scratch("text-limit");
    let root = base.join("root");
    std::fs::create_dir_all(&root).expect("create the root");
    // MAX_TEXT (16 MiB) + 1 byte.
    std::fs::write(root.join("big.txt"), vec![b'a'; (16 << 20) + 1]).expect("write big.txt");
    let (backend, lines) = rooted("export {};\n", &root);

    backend.dispatch(1, call(1, "fs.readText", "[\"big.txt\"]"));
    refused_with(&next(&lines), "E2BIG", "fs.readText of 16 MiB + 1");
}
