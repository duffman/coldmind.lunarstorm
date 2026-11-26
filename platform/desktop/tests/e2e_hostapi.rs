//! End to end on the real `lunarstorm-debug`: a page in the WebView calls
//! `window.lunarstorm.hostapi.fs`, the call travels WebView -> IPC -> host ->
//! Runtime dispatcher -> HostApi area `fs` and the result comes back to the page.
//! The app has no backend code. The page reports by writing files through the same
//! API, so the test only reads the filesystem.
//!
//! Tests: the round trip with refusals (`a_webview_call_reaches_the_fs_area_through_the_runtime_and_back`),
//! a reloaded page against its predecessor's calls
//! (`a_reloaded_page_does_not_inherit_the_old_pages_calls`), and the per-form call bound
//! (`a_page_cannot_queue_more_than_max_calls`). The last two keep the backend busy for 3 s.
//!
//! Needs a desktop session: on macOS a logged-in session, on Windows WebView2, on Linux
//! WebKitGTK under a display (`xvfb-run cargo test ...`).

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// The app's host configuration; `__DATA__` becomes the one filesystem root.
const CONFIG: &str = r#"{
  "schema": 1,
  "application": { "id": "dev.lunarstorm.e2e.hostapi", "name": "HostApi e2e", "version": "0.1.0", "mainForm": "MainForm" },
  "forms": [{ "name": "MainForm", "props": { "caption": "HostApi e2e", "width": 320, "height": 200 }, "events": [], "controls": {} }],
  "permissions": { "fs": [__DATA__], "hostapi": ["fs"] }
}"#;

/// The page. In order: an escape attempt (its error code goes to `denied.txt`), a
/// raw `call` to an area the host does not offer (its code goes to `refused.txt`),
/// then `in.txt` read back into `out.txt`, last, as the signal that it is done.
const PAGE: &str = r#"<!doctype html><meta charset="utf-8"><title>e2e</title><script>
(async () => {
  const fs = window.lunarstorm.hostapi.fs;
  const code = async (promise) => { try { await promise; return 'ok'; } catch (error) { return error.code; } };
  await fs.writeText('denied.txt', await code(fs.readText('../escape.txt')));
  const raw = (method) => new Promise((resolve) => {
    const previous = window.__zebraReceive;
    window.__zebraReceive = (envelope) => {
      if (envelope.kind !== 'result' || envelope.id !== 9999) return previous(envelope);
      window.__zebraReceive = previous;
      resolve(envelope.payload.error.code);
    };
    window.ipc.postMessage(JSON.stringify({ version: 1, kind: 'call', id: 9999, windowId: 'MainForm', epoch: window.zebra.epoch, method, payload: { args: [] } }));
  });
  const hidden = window.lunarstorm.hostapi.nope === undefined ? '' : 'visible:';
  await fs.writeText('refused.txt', hidden + await raw('nope.x'));
  await fs.writeText('out.txt', 'got:' + await fs.readText('in.txt'));
})();
</script>"#;

/// Kills the app when the test ends, passed or not.
struct App(Child);

impl Drop for App {
    /// Kills the process and reaps it.
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Waits for a file the page writes.
///
/// # Arguments
/// * `path` - the file.
/// * `until` - when to give up.
///
/// # Returns
/// Its content; fails the test at `until`.
fn wait_for(path: &Path, until: Instant) -> String {
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
        assert!(Instant::now() < until, "{} was not written within 20 s", path.display());
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_webview_call_reaches_the_fs_area_through_the_runtime_and_back() {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("e2e-hostapi");
    let _ = std::fs::remove_dir_all(&base);
    let (project, data): (PathBuf, PathBuf) = (base.join("project"), base.join("data"));
    std::fs::create_dir_all(project.join("frontend")).expect("create the project");
    std::fs::create_dir_all(&data).expect("create the data root");
    let data_json = format!("{:?}", data.to_string_lossy());
    std::fs::write(project.join("zebra.json"), CONFIG.replace("__DATA__", &data_json)).expect("write zebra.json");
    std::fs::write(project.join("frontend/index.html"), PAGE).expect("write index.html");
    std::fs::write(data.join("in.txt"), "hello from disk").expect("write in.txt");
    std::fs::write(base.join("escape.txt"), "secret").expect("write escape.txt");

    let _app = App(Command::new(env!("CARGO_BIN_EXE_lunarstorm-debug")).arg(&project).spawn().expect("start lunarstorm-debug"));
    let until = Instant::now() + Duration::from_secs(20);
    assert_eq!(wait_for(&data.join("out.txt"), until), "got:hello from disk");
    assert_eq!(wait_for(&data.join("denied.txt"), until), "EACCES");
    assert_eq!(wait_for(&data.join("refused.txt"), until), "EREFUSED");
    assert_eq!(std::fs::read_to_string(base.join("escape.txt")).expect("escape.txt"), "secret");
}

/// Host configuration of the busy fixture: a `Busy` button whose `onClick` keeps
/// the backend busy for 3 s (budget 5 s), so calls stay in flight meanwhile.
const BUSY_CONFIG: &str = r#"{
  "schema": 1,
  "application": { "id": "dev.lunarstorm.e2e.busy", "name": "HostApi busy e2e", "version": "0.1.0", "mainForm": "MainForm" },
  "forms": [{ "name": "MainForm", "props": { "caption": "HostApi busy e2e", "width": 320, "height": 200 }, "events": [],
    "controls": { "Busy": { "type": "button", "props": [], "events": ["onClick"] } } }],
  "permissions": { "fs": [__DATA__], "hostapi": ["fs"] },
  "limits": { "budgetMs": 5000 }
}"#;

/// The busy fixture's backend: `Busy.onClick` runs for 3 s.
const BUSY_BACKEND: &str = "import { serve } from '@desktop/bridge';\n\
serve((m) => { if (m.event === 'onClick') { const end = Date.now() + 3000; while (Date.now() < end) {} } });\n";

/// Writes a project with the busy fixture and `page`, and starts the app on it.
///
/// # Arguments
/// * `name` - the test's directory under Cargo's temp directory.
/// * `page` - `frontend/index.html`.
///
/// # Returns
/// The running app and the data root the page writes to.
fn busy_app(name: &str, page: &str) -> (App, PathBuf) {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&base);
    let (project, data) = (base.join("project"), base.join("data"));
    std::fs::create_dir_all(project.join("frontend")).expect("create the project");
    std::fs::create_dir_all(&data).expect("create the data root");
    let data_json = format!("{:?}", data.to_string_lossy());
    std::fs::write(project.join("zebra.json"), BUSY_CONFIG.replace("__DATA__", &data_json)).expect("write zebra.json");
    std::fs::write(project.join("backend.js"), BUSY_BACKEND).expect("write backend.js");
    std::fs::write(project.join("frontend/index.html"), page).expect("write index.html");
    std::fs::write(data.join("in.txt"), "hello from disk").expect("write in.txt");
    let app = App(Command::new(env!("CARGO_BIN_EXE_lunarstorm-debug")).arg(&project).spawn().expect("start lunarstorm-debug"));
    (app, data)
}

/// First load: a call queued behind the busy backend, then a reload to `#two`.
/// Second load: one write, then the value that write resolved with.
const RELOAD_PAGE: &str = r#"<!doctype html><meta charset="utf-8"><title>e2e</title><script>
(async () => {
  const fs = window.lunarstorm.hostapi.fs;
  if (location.hash !== '#two') {
    window.zebra.emit('Busy', 'onClick');
    fs.readText('in.txt');
    setTimeout(() => { location.hash = 'two'; location.reload(); }, 200);
    return;
  }
  let value;
  try { value = await fs.writeText('reload.txt', 'ok'); } catch (error) { await fs.writeText('reload.txt', error.code); return; }
  await fs.writeText('value.txt', JSON.stringify(value));
})();
</script>"#;

#[test]
fn a_reloaded_page_does_not_inherit_the_old_pages_calls() {
    let (_app, data) = busy_app("e2e-hostapi-reload", RELOAD_PAGE);
    let until = Instant::now() + Duration::from_secs(20);
    assert_eq!(wait_for(&data.join("reload.txt"), until), "ok", "the new page's call collided with the old page's");
    assert_eq!(wait_for(&data.join("value.txt"), until), "null", "the new page got the old page's result");
}

/// 65 reads while the backend is busy; the codes, counted, go to `codes.txt`.
const FLOOD_PAGE: &str = r#"<!doctype html><meta charset="utf-8"><title>e2e</title><script>
(async () => {
  const fs = window.lunarstorm.hostapi.fs;
  window.zebra.emit('Busy', 'onClick');
  const codes = await Promise.all(Array.from({ length: 65 }, () => fs.readText('in.txt').then(() => 'ok', (error) => error.code)));
  const counts = {};
  for (const code of codes) counts[code] = (counts[code] ?? 0) + 1;
  await fs.writeText('codes.txt', Object.entries(counts).sort().map(([code, n]) => `${code}:${n}`).join(','));
})();
</script>"#;

#[test]
fn a_page_cannot_queue_more_than_max_calls() {
    let (_app, data) = busy_app("e2e-hostapi-flood", FLOOD_PAGE);
    let until = Instant::now() + Duration::from_secs(20);
    assert_eq!(wait_for(&data.join("codes.txt"), until), "EAGAIN:1,ok:64");
}
