//! End to end on the real `lunarstorm-debug`: a page in the WebView calls
//! `window.lunarstorm.hostapi.fs`, the call travels WebView -> IPC -> host ->
//! Runtime dispatcher -> HostApi area `fs` and the result comes back to the page.
//! The app has no backend code. The page reports by writing files through the same
//! API, so the test only reads the filesystem.
//!
//! Needs a desktop session (WKWebView on macOS, WebView2 on Windows, WebKitGTK and a
//! display on Linux).

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
