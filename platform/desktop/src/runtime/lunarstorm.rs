//! The default AppRuntime: the app's backend on the Lunarstorm Runtime, run by the
//! desktop app host (`runtime/hosts/desktop`, crate `lunarstorm-host-desktop`).
//!
//! ```text
//!  ui thread ── HostMessage ──> LunarstormRuntime ── Backend::load / dispatch ──> backend thread
//!  ui thread <── RuntimeMessage ── Bridge (Shell) <── loaded / send / native / error / log ──┘
//!  native services ── Completer ──> Settler::settle ──> backend thread
//! ```
//!
//! Decisions:
//! - The host crate speaks JSON text and epochs, never the platform's types; this
//!   adapter is the one place the two meet. Outbound envelopes are stamped with
//!   `version` and `epoch` here, as the host's `Shell::send` contract leaves them out.
//! - Permissions are checked here, before any native service sees a call, against the
//!   host configuration of the calling epoch: services never trust the backend.
//! - The backend gets no filesystem roots, no environment and no sockets. The host
//!   configuration has no permission that would grant them, and the former JS thread
//!   offered none either.
//! - `send` never reports a full queue: the host posts straight to the Runtime's loop,
//!   which is unbounded. It fails only once the runtime is stopped.

use super::{AppRuntime, Completer, HostMessage, NativeCall, NativeRouter, RuntimeContext, RuntimeMessage, Settle};
use crate::envelope::{Envelope, VERSION};
use crate::manifest::HostConfig;
use lunarstorm_host_desktop::{Backend, Program, Settler, Shell};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The platform side of the host's `Shell` contract.
struct Bridge {
    /// The ui thread's sink.
    out: Arc<dyn Fn(RuntimeMessage) + Send + Sync>,
    /// Native services.
    natives: Arc<dyn NativeRouter>,
    /// Host configuration of each loaded or loading epoch, for permission checks.
    configs: Mutex<HashMap<u64, Arc<HostConfig>>>,
    /// Settles native calls on the backend thread.
    ///
    /// Decision: the settler holds an `Arc` of this bridge, so storing it here is a
    /// cycle; [`LunarstormRuntime::stop`] clears it to break the cycle.
    settler: Mutex<Option<Settler<Bridge>>>,
}

/// Turns an envelope the backend sent into a platform envelope of `epoch`.
///
/// # Errors
/// The text is not an envelope.
fn stamp(epoch: u64, json: &str) -> Result<Envelope, serde_json::Error> {
    let mut value: Value = serde_json::from_str(json)?;
    if let Some(fields) = value.as_object_mut() {
        fields.insert("version".into(), VERSION.into());
        fields.insert("epoch".into(), epoch.into());
    }
    serde_json::from_value(value)
}

impl Shell for Bridge {
    fn loaded(&self, epoch: u64, error: Option<&str>) {
        if error.is_some() {
            // A failed load never becomes live, so it is never retired.
            self.configs.lock().expect("configs lock").remove(&epoch);
        }
        (self.out)(RuntimeMessage::Loaded { epoch, error: error.map(str::to_owned) });
    }

    fn send(&self, epoch: u64, envelope: &str) {
        let message = match stamp(epoch, envelope) {
            Ok(envelope) => RuntimeMessage::Envelope(envelope),
            Err(e) => RuntimeMessage::Error { epoch, message: format!("bad backend envelope: {e}") },
        };
        (self.out)(message);
    }

    fn native(&self, epoch: u64, call: u64, namespace: &str, method: &str, arguments: &str) {
        // No settler means the runtime is stopping; nobody would receive the result.
        let Some(settler) = self.settler.lock().expect("settler lock").clone() else { return };
        let settle: Settle = Arc::new(move |id, epoch, result: Result<Value, String>| {
            // `null` reaches the backend as `undefined`, as from the former JS thread.
            let result = result.map(|value| if value.is_null() { String::new() } else { value.to_string() });
            settler.settle(epoch, id, result);
        });
        let done = Completer::new(settle, call, epoch);
        let args = match serde_json::from_str::<Vec<Value>>(arguments) {
            Ok(args) => args,
            Err(e) => return done.complete(Err(format!("bad arguments: {e}"))),
        };
        let allowed = self.configs.lock().expect("configs lock").get(&epoch).is_some_and(|c| c.allows_native(namespace, method));
        if allowed {
            self.natives.call(NativeCall { ns: namespace.to_owned(), method: method.to_owned(), args, done });
        } else {
            done.complete(Err(format!("native method {namespace}.{method} is not permitted by the host configuration")));
        }
    }

    fn error(&self, epoch: u64, message: &str) {
        (self.out)(RuntimeMessage::Error { epoch, message: message.to_owned() });
    }

    fn log(&self, level: &str, message: &str) {
        (self.out)(RuntimeMessage::Log { level: level.to_owned(), message: message.to_owned() });
    }

    fn retired(&self, epoch: u64) {
        self.configs.lock().expect("configs lock").remove(&epoch);
        self.natives.release_epoch(epoch);
    }
}

/// The app's backend on the Lunarstorm Runtime. Dropping it stops the backend thread.
pub struct LunarstormRuntime {
    /// The backend thread; `None` once stopped.
    backend: Mutex<Option<Backend<Bridge>>>,
    /// The shell the backend thread calls.
    bridge: Arc<Bridge>,
}

impl LunarstormRuntime {
    /// Starts the backend thread; it idles until the first `Load`.
    pub fn spawn(ctx: RuntimeContext) -> LunarstormRuntime {
        let bridge = Arc::new(Bridge {
            out: ctx.out,
            natives: ctx.natives,
            configs: Mutex::default(),
            settler: Mutex::new(None),
        });
        let backend = Backend::spawn(bridge.clone());
        *bridge.settler.lock().expect("settler lock") = Some(backend.settler());
        LunarstormRuntime { backend: Mutex::new(Some(backend)), bridge }
    }

    /// Stops the backend thread and waits for it; later messages are refused.
    fn stop(&self) {
        let backend = self.backend.lock().expect("backend lock").take();
        // Dropped outside the lock: it joins the thread, which may still call the bridge.
        drop(backend);
        self.bridge.settler.lock().expect("settler lock").take();
    }
}

impl Drop for LunarstormRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

impl AppRuntime for LunarstormRuntime {
    fn name(&self) -> &'static str {
        "lunarstorm"
    }

    fn send(&self, message: HostMessage) -> Result<(), String> {
        let guard = self.backend.lock().expect("backend lock");
        let backend = guard.as_ref().ok_or("backend thread has stopped")?;
        match message {
            HostMessage::Load { backend: code, config, restore, epoch } => {
                self.bridge.configs.lock().expect("configs lock").insert(epoch, config.clone());
                backend.load(Program {
                    epoch,
                    code,
                    restore: restore.to_string(),
                    memory: Some((config.limits.heap_mb as usize) << 20),
                    budget: Duration::from_millis(config.limits.budget_ms),
                    roots: Vec::new(),
                    environment: Vec::new(),
                    name: config.application.id.clone(),
                });
            }
            HostMessage::Envelope(envelope) => backend.dispatch(envelope.epoch, envelope.to_json()),
            HostMessage::Stop => {
                drop(guard);
                self.stop();
            }
        }
        Ok(())
    }

    fn send_blocking(&self, message: HostMessage) {
        // Nothing to wait for: the host's queues are unbounded. A stopped runtime has
        // no one left to receive the message.
        let _ = self.send(message);
    }
}
