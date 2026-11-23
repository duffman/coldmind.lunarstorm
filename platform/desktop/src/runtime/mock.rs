//! MockRuntime: stands in for the app runtime until the real one exists. It logs every
//! message the host delivers over IPC to stderr and runs no app logic.
//!
//! Decision: logging alone would leave the app stuck, so it does the minimum the
//! [`AppRuntime`] contract requires and nothing else: it reports `Loaded` for each `Load`
//! (the host mounts the main form only after that) and answers each `request` with an
//! empty payload, as a backend without a handler for it does. It never sends updates.

use serde_json::{json, Value};

use super::{AppRuntime, HostMessage, RuntimeContext, RuntimeMessage};
use crate::envelope::{Envelope, Kind};

/// Prefix of every log line.
const TAG: &str = "[mock-runtime]";

/// The mock runtime; it handles each message on the caller's thread.
pub struct MockRuntime {
    /// Output sink and native router from the host.
    ctx: RuntimeContext,
}

impl MockRuntime {
    /// Creates the mock.
    ///
    /// # Arguments
    /// * `ctx` - output sink and native router.
    ///
    /// # Returns
    /// The mock.
    pub fn new(ctx: RuntimeContext) -> MockRuntime {
        MockRuntime { ctx }
    }

    /// Logs one message and sends what the contract requires back.
    ///
    /// # Arguments
    /// * `message` - the message.
    fn handle(&self, message: HostMessage) {
        eprintln!("{}", describe(&message));
        match message {
            HostMessage::Load { epoch, .. } => (self.ctx.out)(RuntimeMessage::Loaded { epoch, error: None }),
            HostMessage::Envelope(envelope) if envelope.kind == Kind::Request => {
                let mut response = Envelope::new(Kind::Response, &envelope.window_id, envelope.epoch, &envelope.method, json!({}));
                response.id = envelope.id;
                (self.ctx.out)(RuntimeMessage::Envelope(response));
            }
            _ => {}
        }
    }
}

impl AppRuntime for MockRuntime {
    /// # Returns
    /// `mock`.
    fn name(&self) -> &'static str {
        "mock"
    }

    /// Logs and handles the message at once; it never refuses one.
    ///
    /// # Arguments
    /// * `message` - the message.
    ///
    /// # Errors
    /// None.
    fn send(&self, message: HostMessage) -> Result<(), String> {
        self.handle(message);
        Ok(())
    }

    /// Same as [`AppRuntime::send`].
    ///
    /// # Arguments
    /// * `message` - the message.
    fn send_blocking(&self, message: HostMessage) {
        self.handle(message);
    }
}

/// Formats the log line for one message.
///
/// # Arguments
/// * `message` - the message.
///
/// # Returns
/// The line: `Load` without the backend source (only its size), envelopes as JSON.
pub fn describe(message: &HostMessage) -> String {
    match message {
        HostMessage::Load { backend, restore, epoch, config } => format!(
            "{TAG} load epoch={epoch} app={} backend={} bytes restore={}",
            config.application.id,
            backend.len(),
            restore
        ),
        HostMessage::Envelope(envelope) => {
            format!("{TAG} {}", serde_json::to_value(envelope).unwrap_or(Value::Null))
        }
        HostMessage::Stop => format!("{TAG} stop"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{NativeCall, NativeRouter};
    use crate::pipeline::tests::sample_build;
    use std::sync::{Arc, Mutex};

    /// Router that refuses everything; the mock never calls it.
    struct NoNatives;
    impl NativeRouter for NoNatives {
        /// Refuses the call.
        ///
        /// # Arguments
        /// * `call` - the call.
        fn call(&self, call: NativeCall) {
            call.done.complete(Err("no natives in this test".into()));
        }
        /// Nothing to release.
        ///
        /// # Arguments
        /// * `_epoch` - ignored.
        fn release_epoch(&self, _epoch: u64) {}
    }

    /// Builds a mock whose output is collected.
    ///
    /// # Returns
    /// The mock and the collected output.
    fn mock() -> (MockRuntime, Arc<Mutex<Vec<RuntimeMessage>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let ctx = RuntimeContext { out: Arc::new(move |m| sink.lock().unwrap().push(m)), natives: Arc::new(NoNatives) };
        (MockRuntime::new(ctx), seen)
    }

    /// Load is acknowledged, a request gets an empty response with its id, an event gets
    /// nothing back.
    #[test]
    fn acknowledges_load_and_requests_only() {
        let (rt, seen) = mock();
        let build = sample_build();
        rt.send_blocking(HostMessage::Load { backend: build.backend.clone(), config: Arc::new(build.config), restore: json!({}), epoch: 3 });
        rt.send(HostMessage::Envelope(Envelope::new(Kind::Event, "MainForm", 3, "SaveButton.onClick", json!({ "props": {} })))).unwrap();
        let mut request = Envelope::new(Kind::Request, "MainForm", 3, "MainForm.onCloseQuery", json!({}));
        request.id = Some(7);
        rt.send(HostMessage::Envelope(request)).unwrap();

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], RuntimeMessage::Loaded { epoch: 3, error: None });
        let RuntimeMessage::Envelope(response) = &seen[1] else { panic!("expected a response, got {:?}", seen[1]) };
        assert_eq!((response.kind, response.id, response.epoch), (Kind::Response, Some(7), 3));
        assert_eq!(response.method, "MainForm.onCloseQuery");
    }

    /// The log line carries the IPC data: method, window, payload.
    #[test]
    fn log_line_carries_the_envelope() {
        let envelope = Envelope::new(Kind::Event, "MainForm", 1, "NameEdit.onChange", json!({ "props": { "text": "abc" } }));
        let line = describe(&HostMessage::Envelope(envelope));
        assert!(line.starts_with("[mock-runtime] {"), "{line}");
        assert!(line.contains(r#""method":"NameEdit.onChange""#) && line.contains(r#""windowId":"MainForm""#) && line.contains(r#""text":"abc""#), "{line}");
    }
}
