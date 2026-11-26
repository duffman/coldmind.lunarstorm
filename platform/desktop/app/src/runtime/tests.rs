//! LunarstormRuntime tests: the example backend and small inline backends run on the
//! real Lunarstorm Runtime, headless, with the real native router.
//!
//! Decision: these are the former JS thread's tests with only the harness changed, so
//! they prove the Runtime-based backend behaves as the archived thread did.

use super::lunarstorm::LunarstormRuntime;
use super::*;
use crate::envelope::{Envelope, Kind};
use crate::manifest::HostConfig;
use crate::native::NativeServices;
use crate::pipeline::tests::config_with;
use crossbeam_channel::Receiver;
use serde_json::{json, Value};
use std::sync::RwLock;
use std::time::Duration;

/// A runtime whose host messages arrive on a channel.
struct Harness {
    /// The runtime.
    js: LunarstormRuntime,
    /// Everything the runtime sent.
    rx: Receiver<RuntimeMessage>,
}

impl Harness {
    /// Starts a runtime with the real router; UI-thread calls answer `"ui:<method>"`.
    ///
    /// # Arguments
    /// * `config` - host configuration.
    ///
    /// # Returns
    /// The harness.
    fn new(config: &HostConfig) -> Harness {
        let (tx, rx) = crossbeam_channel::unbounded();
        let ui = Box::new(|call: NativeCall| {
            let answer = format!("ui:{}", call.method);
            call.done.complete(Ok(json!(answer)));
        });
        let natives = Arc::new(NativeServices::new(ui, Arc::new(RwLock::new(Arc::new(config.clone())))));
        let js = LunarstormRuntime::spawn(RuntimeContext { out: Arc::new(move |m| { let _ = tx.send(m); }), natives });
        Harness { js, rx }
    }

    /// Loads a backend and waits for the result.
    ///
    /// # Arguments
    /// * `backend` - artifact source.
    /// * `config` - host configuration.
    /// * `restore` - carried-over values.
    /// * `epoch` - epoch.
    ///
    /// # Returns
    /// The load error, if any.
    fn load(&self, backend: &str, config: &HostConfig, restore: Value, epoch: u64) -> Option<String> {
        self.js.send_blocking(HostMessage::Load { backend: Arc::from(backend), config: Arc::new(config.clone()), restore, epoch });
        loop {
            if let RuntimeMessage::Loaded { epoch: e, error } = self.next() {
                assert_eq!(e, epoch);
                return error;
            }
        }
    }

    /// Sends an envelope.
    ///
    /// # Arguments
    /// * `kind` - kind.
    /// * `method` - method.
    /// * `payload` - payload.
    /// * `epoch` - epoch.
    /// * `id` - correlation id.
    fn send(&self, kind: Kind, method: &str, payload: Value, epoch: u64, id: Option<u64>) {
        let mut env = Envelope::new(kind, "MainForm", epoch, method, payload);
        env.id = id;
        self.js.send(HostMessage::Envelope(env)).unwrap();
    }

    /// Next message, failing after 5 s.
    ///
    /// # Returns
    /// The message.
    fn next(&self) -> RuntimeMessage {
        self.rx.recv_timeout(Duration::from_secs(5)).expect("the runtime answered within 5 s")
    }

    /// Waits for an update whose controls contain `control.prop == value`.
    ///
    /// # Arguments
    /// * `control` - control name.
    /// * `prop` - property.
    /// * `value` - expected value.
    ///
    /// # Returns
    /// The epoch the update carried.
    fn expect_update(&self, control: &str, prop: &str, value: Value) -> u64 {
        loop {
            if let RuntimeMessage::Envelope(env) = self.next() {
                if env.kind == Kind::Update && env.payload["controls"][control][prop] == value {
                    return env.epoch;
                }
            }
        }
    }

    /// Waits for the response to request `id`.
    ///
    /// # Arguments
    /// * `id` - request id.
    ///
    /// # Returns
    /// Its payload.
    fn expect_response(&self, id: u64) -> Value {
        loop {
            if let RuntimeMessage::Envelope(env) = self.next() {
                if env.kind == Kind::Response && env.id == Some(id) {
                    return env.payload;
                }
            }
        }
    }

    /// Waits for an error report.
    ///
    /// # Returns
    /// Its message.
    fn expect_error(&self) -> String {
        loop {
            if let RuntimeMessage::Error { message, .. } = self.next() {
                return message;
            }
        }
    }
}

/// Source of the example backend.
///
/// # Returns
/// The artifact.
fn example_backend() -> String {
    crate::pipeline::tests::sample_build().backend.to_string()
}

#[test]
fn example_backend_handles_events_requests_and_natives() {
    let config = config_with(&["os.*"], &[]);
    let h = Harness::new(&config);
    assert_eq!(h.load(&example_backend(), &config, json!({}), 1), None);

    h.send(Kind::Event, "MainForm.onCreate", json!({}), 1, None);
    h.expect_update("NameEdit", "text", json!("Desktop component library"));
    // formCreated awaits native.os.info(), which runs inline and settles the promise.
    let status = format!("Ready · {}/{}", std::env::consts::OS, std::env::consts::ARCH);
    h.expect_update("StatusBar", "caption", json!(status));

    h.send(Kind::Event, "NameEdit.onChange", json!({"props": {"text": "Renamed"}}), 1, None);
    h.expect_update("SaveButton", "enabled", json!(true));

    h.send(Kind::Request, "MainForm.onCloseQuery", json!({}), 1, Some(41));
    assert_eq!(h.expect_response(41), json!({"cancel": true}));

    h.send(Kind::Event, "SaveButton.onClick", json!({}), 1, None);
    h.expect_update("StatusBar", "caption", json!("Saved “Renamed”."));
    h.send(Kind::Request, "MainForm.onCloseQuery", json!({}), 1, Some(42));
    assert_eq!(h.expect_response(42), json!({"cancel": false}));
}

#[test]
fn reload_carries_values_over_and_blocks_the_old_epoch() {
    let config = config_with(&["os.*"], &[]);
    let h = Harness::new(&config);
    let backend = "import { serve, setProperty, restored } from '@desktop/bridge';\n\
        serve((m) => { const kept = restored('MainForm').NameEdit?.text ?? 'none';\n\
          setProperty('MainForm', 'StatusBar', 'caption', `${m.target}.${m.event}:${kept}`); });";
    assert_eq!(h.load(backend, &config, json!({}), 1), None);
    assert_eq!(h.load(backend, &config, json!({"MainForm": {"NameEdit": {"text": "typed"}}}), 2), None);
    // Late message of epoch 1: dropped, so the next update comes from the epoch-2 event.
    h.send(Kind::Event, "SaveButton.onClick", json!({}), 1, None);
    h.send(Kind::Event, "MainForm.onCreate", json!({}), 2, None);
    let epoch = h.expect_update("StatusBar", "caption", json!("MainForm.onCreate:typed"));
    assert_eq!(epoch, 2);
}

#[test]
fn a_runaway_handler_is_stopped_by_the_budget_and_the_thread_survives() {
    let mut config = config_with(&[], &[]);
    config.limits.budget_ms = 200;
    let h = Harness::new(&config);
    let backend = "import { serve, setProperty } from '@desktop/bridge';\n\
        serve((m) => { if (m.event === 'onClick') for (;;) {} setProperty('MainForm', 'StatusBar', 'caption', 'alive'); });";
    assert_eq!(h.load(backend, &config, json!({}), 1), None);
    h.send(Kind::Event, "SaveButton.onClick", json!({}), 1, None);
    assert!(h.expect_error().contains("budget"));
    h.send(Kind::Event, "NameEdit.onChange", json!({}), 1, None);
    h.expect_update("StatusBar", "caption", json!("alive"));
}

#[test]
fn timers_natives_permissions_and_errors() {
    let config = config_with(&["window.setTitle"], &[]);
    let h = Harness::new(&config);
    let backend = "import { serve, setProperty } from '@desktop/bridge';\n\
        import { native } from '@desktop/api';\n\
        const put = (v) => setProperty('MainForm', 'StatusBar', 'caption', v);\n\
        setTimeout(() => put('timer'), 20);\n\
        serve(async (m) => {\n\
          if (m.event === 'onClick') put(await native.window.setTitle('MainForm', 'x'));\n\
          if (m.event === 'onChange') { try { await native.clipboard.readText(); } catch (e) { put(e.message); } }\n\
          if (m.event === 'onResize') throw new Error('handler failed');\n\
        });";
    assert_eq!(h.load(backend, &config, json!({}), 1), None);
    h.expect_update("StatusBar", "caption", json!("timer"));
    h.send(Kind::Event, "SaveButton.onClick", json!({}), 1, None);
    h.expect_update("StatusBar", "caption", json!("ui:setTitle"));
    h.send(Kind::Event, "NameEdit.onChange", json!({}), 1, None);
    h.expect_update(
        "StatusBar",
        "caption",
        json!("native method clipboard.readText is not permitted by the host configuration"),
    );
    h.send(Kind::Event, "MainForm.onResize", json!({}), 1, None);
    assert!(h.expect_error().contains("handler failed"));
}

#[test]
fn a_backend_that_throws_while_loading_reports_it() {
    let config = config_with(&[], &[]);
    let h = Harness::new(&config);
    let error = h.load("throw new Error('broken backend');", &config, json!({}), 1).unwrap();
    assert!(error.contains("broken backend"), "{error}");
}

#[test]
fn a_cancelled_request_gets_no_response() {
    let config = config_with(&[], &[]);
    let h = Harness::new(&config);
    let backend = "import { serve, setProperty } from '@desktop/bridge';\n\
        let release;\n\
        serve((m) => {\n\
          if (m.event === 'onCloseQuery') return new Promise((r) => { release = r; });\n\
          if (m.event === 'onClick') { release({ cancel: true }); setProperty('MainForm', 'StatusBar', 'caption', 'released'); }\n\
        });";
    assert_eq!(h.load(backend, &config, json!({}), 1), None);
    h.send(Kind::Request, "MainForm.onCloseQuery", json!({}), 1, Some(7));
    h.send(Kind::Cancel, "MainForm.onCloseQuery", json!({}), 1, Some(7));
    h.send(Kind::Event, "SaveButton.onClick", json!({}), 1, None);
    loop {
        match h.next() {
            RuntimeMessage::Envelope(env) if env.kind == Kind::Response => panic!("cancelled request answered: {env:?}"),
            RuntimeMessage::Envelope(env) if env.payload["controls"]["StatusBar"]["caption"] == json!("released") => break,
            _ => {}
        }
    }
}

/// Creates an empty directory unique to one test under the repo's `.scratch/`.
///
/// # Arguments
/// * `name` - the test's name.
///
/// # Returns
/// The directory.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.scratch/platform-runtime").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the test directory");
    dir
}

#[test]
fn the_backend_gets_no_fs_roots_from_permissions_fs() {
    let dir = scratch("backend-fs");
    std::fs::write(dir.join("a.txt"), "content").expect("write a.txt");
    let mut config = config_with(&[], &[]);
    config.permissions.fs = vec![dir.to_string_lossy().into_owned()];
    config.permissions.hostapi = vec![String::from("fs")];
    let h = Harness::new(&config);
    let backend = format!(
        "import {{ serve, setProperty }} from '@desktop/bridge';\nimport {{ readFileSync }} from 'node:fs';\n\
         serve(() => {{ let shown; try {{ shown = readFileSync({:?}, 'utf8'); }} catch (error) {{ shown = error.code; }}\n\
         setProperty('MainForm', 'StatusBar', 'caption', shown); }});\n",
        dir.join("a.txt").to_string_lossy()
    );
    assert_eq!(h.load(&backend, &config, json!({}), 1), None);
    h.send(Kind::Event, "SaveButton.onClick", json!({}), 1, None);
    // permissions.fs scopes the WebView's `fs` area, not the backend's own `node:fs`.
    let shown = loop {
        if let RuntimeMessage::Envelope(env) = h.next() {
            if env.kind == Kind::Update {
                break env.payload["controls"]["StatusBar"]["caption"].clone();
            }
        }
    };
    assert_eq!(shown, json!("EACCES"), "the backend read a permissions.fs file");
}

#[test]
fn a_result_over_the_envelope_limit_becomes_e2big() {
    let dir = scratch("result-limit");
    // 3 MiB passes the `fs` text limit; escaped as JSON (`\u0001`) it is ~18 MiB.
    std::fs::write(dir.join("big.txt"), vec![1u8; 3 << 20]).expect("write big.txt");
    let mut config = config_with(&[], &[]);
    config.permissions.fs = vec![dir.to_string_lossy().into_owned()];
    config.permissions.hostapi = vec![String::from("fs")];
    let h = Harness::new(&config);
    assert_eq!(h.load("export {};\n", &config, json!({}), 1), None);
    h.send(Kind::Call, "fs.readText", json!({ "args": ["big.txt"] }), 1, Some(5));
    let result = loop {
        if let RuntimeMessage::Envelope(env) = h.next() {
            if env.kind == Kind::Result && env.id == Some(5) {
                break env;
            }
        }
    };
    let size = result.to_json().len();
    assert_eq!(result.payload["error"]["code"], json!("E2BIG"), "a result of {size} bytes reached the ui");
    assert_eq!(result.window_id, "MainForm");
}
