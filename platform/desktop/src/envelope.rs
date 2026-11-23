//! IPC transport (spec 6): the one envelope every message travels in, the checks every
//! WebView message passes, and the property mirror the host keeps for carry-over.
//!
//! Wire shape: `{version, kind, id, windowId, epoch, method, payload}`.
//!
//! | kind       | direction            | method                    | payload                      |
//! |------------|----------------------|---------------------------|------------------------------|
//! | `hello`    | WebView -> host      | `session.start`           | `{}`                         |
//! | `hello`    | host -> WebView      | `session.ready`           | `{controls}` current values  |
//! | `event`    | WebView -> host -> JS| `<Control>.<onEvent>`     | `{props?, args?}`            |
//! | `event`    | host -> JS           | `<Form>.<onEvent>`        | event arguments              |
//! | `update`   | JS -> host -> WebView| `update`                  | `{controls: {Name: {p: v}}}` |
//! | `request`  | host -> JS           | `<Form>.onCloseQuery`     | `{}`                         |
//! | `response` | JS -> host           | same as the request       | `{cancel}`                   |
//! | `cancel`   | host -> JS           | same as the request       | `{}`                         |
//! | `dev`      | host -> WebView      | `dev.*`, `backend.epoch`  | per message                  |
//!
//! Decision: there is no second channel. Dev messages, property updates and control
//! events share this envelope, so ordering between them is the queue order.

use crate::manifest::{FormConfig, HostConfig};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Envelope version this host speaks.
pub const VERSION: u32 = 1;

/// Capacity of each bounded queue between the threads.
pub const QUEUE_CAPACITY: usize = 1024;

/// Largest WebView message accepted, in bytes; larger ones are rejected unparsed.
pub const MAX_MESSAGE: usize = 1 << 20;

/// What an envelope is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Session start and its answer.
    Hello,
    /// A control or form event.
    Event,
    /// Property values for controls.
    Update,
    /// A host request that expects a `response`.
    Request,
    /// The answer to a `request`.
    Response,
    /// The host gave up on a `request`.
    Cancel,
    /// Dev update queue messages and `backend.epoch`.
    Dev,
    /// A rejected message, sent back to its WebView.
    Error,
}

/// The single message shape on every hop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    /// Protocol version; see [`VERSION`].
    pub version: u32,
    /// Message kind.
    pub kind: Kind,
    /// Correlation id of a request/response pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    /// Form name, or `@application`.
    pub window_id: String,
    /// Backend epoch the sender knew.
    pub epoch: u64,
    /// What the message is about (see the module table).
    pub method: String,
    /// Kind-specific data.
    #[serde(default)]
    pub payload: Value,
}

impl Envelope {
    /// Builds an envelope.
    ///
    /// # Arguments
    /// * `kind` - message kind.
    /// * `window_id` - form name or `@application`.
    /// * `epoch` - current backend epoch.
    /// * `method` - method name.
    /// * `payload` - data.
    ///
    /// # Returns
    /// The envelope, without a correlation id.
    pub fn new(kind: Kind, window_id: &str, epoch: u64, method: &str, payload: Value) -> Envelope {
        Envelope { version: VERSION, kind, id: None, window_id: window_id.to_owned(), epoch, method: method.to_owned(), payload }
    }

    /// Serializes to JSON.
    ///
    /// # Returns
    /// The JSON text.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("an envelope always serializes")
    }
}

/// Why a WebView or backend message was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reject {
    /// Not JSON, or not an envelope.
    Malformed(String),
    /// Larger than [`MAX_MESSAGE`].
    TooLarge(usize),
    /// Wrong `version`.
    Version(u32),
    /// A kind this sender may not send.
    Kind(Kind),
    /// `windowId` is not the sender's form.
    Window(String),
    /// Sent under an epoch that is no longer current; dropped silently.
    Stale(u64),
    /// No such control in the form's table.
    Control(String),
    /// The control has no such event.
    Event(String),
    /// The control has no such property.
    Property(String),
}

impl std::fmt::Display for Reject {
    /// Formats the reason for logs and `error` envelopes.
    ///
    /// # Arguments
    /// * `f` - formatter.
    ///
    /// # Returns
    /// The formatter result.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reject::Malformed(e) => write!(f, "malformed message: {e}"),
            Reject::TooLarge(n) => write!(f, "message of {n} bytes exceeds {MAX_MESSAGE}"),
            Reject::Version(v) => write!(f, "unsupported version {v}"),
            Reject::Kind(k) => write!(f, "kind {k:?} not allowed here"),
            Reject::Window(w) => write!(f, "window {w} does not match the sender"),
            Reject::Stale(e) => write!(f, "stale epoch {e}"),
            Reject::Control(c) => write!(f, "unknown control {c}"),
            Reject::Event(e) => write!(f, "unknown event {e}"),
            Reject::Property(p) => write!(f, "unknown property {p}"),
        }
    }
}

/// A WebView message that passed validation.
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    /// The shell started its session; answer with the mirror.
    Hello,
    /// A control event; `props` are the values it changed (already validated).
    Event {
        /// Control name.
        control: String,
        /// Event name.
        event: String,
        /// Changed property values.
        props: Map<String, Value>,
        /// The envelope to forward to the backend.
        envelope: Envelope,
    },
}

/// Parses and validates one message from a form's WebView.
///
/// A WebView sends control events and session starts, nothing else (6, Security). Every
/// control, event and property is checked against the form's table.
///
/// # Arguments
/// * `text` - raw message text from the WebView.
/// * `form` - the form whose WebView sent it (from the host, not the message).
/// * `epoch` - current backend epoch.
///
/// # Returns
/// The validated message.
///
/// # Errors
/// The [`Reject`] reason.
pub fn validate_inbound(text: &str, form: &FormConfig, epoch: u64) -> Result<Inbound, Reject> {
    if text.len() > MAX_MESSAGE {
        return Err(Reject::TooLarge(text.len()));
    }
    let envelope: Envelope = serde_json::from_str(text).map_err(|e| Reject::Malformed(e.to_string()))?;
    if envelope.version != VERSION {
        return Err(Reject::Version(envelope.version));
    }
    if envelope.window_id != form.name {
        return Err(Reject::Window(envelope.window_id));
    }
    match envelope.kind {
        Kind::Hello if envelope.method == "session.start" => Ok(Inbound::Hello),
        Kind::Event => {
            // Decision: a stale epoch is checked after the kind, so a session start is
            // never refused; it is how a shell learns the current epoch.
            if envelope.epoch != epoch {
                return Err(Reject::Stale(envelope.epoch));
            }
            let (control, event) = envelope
                .method
                .split_once('.')
                .ok_or_else(|| Reject::Event(envelope.method.clone()))?;
            let entry = form.controls.get(control).ok_or_else(|| Reject::Control(control.to_owned()))?;
            if !entry.events.iter().any(|e| e == event) {
                return Err(Reject::Event(envelope.method.clone()));
            }
            let props = match envelope.payload.get("props") {
                None | Some(Value::Null) => Map::new(),
                Some(Value::Object(map)) => map.clone(),
                Some(_) => return Err(Reject::Malformed("payload.props is not an object".into())),
            };
            if let Some(bad) = props.keys().find(|p| !entry.props.iter().any(|q| q == *p)) {
                return Err(Reject::Property(format!("{control}.{bad}")));
            }
            let (control, event) = (control.to_owned(), event.to_owned());
            Ok(Inbound::Event { control, event, props, envelope })
        }
        other => Err(Reject::Kind(other)),
    }
}

/// Validates an `update` from the backend against the form's table.
///
/// # Arguments
/// * `form` - target form.
/// * `controls` - `payload.controls` of the update.
///
/// # Returns
/// Nothing when every control and property is in the table.
///
/// # Errors
/// The first unknown control or property.
pub fn validate_update(form: &FormConfig, controls: &Map<String, Value>) -> Result<(), Reject> {
    for (name, props) in controls {
        let entry = form.controls.get(name).ok_or_else(|| Reject::Control(name.clone()))?;
        let props = props.as_object().ok_or_else(|| Reject::Malformed(format!("{name} is not an object")))?;
        if let Some(bad) = props.keys().find(|p| !entry.props.iter().any(|q| q == *p)) {
            return Err(Reject::Property(format!("{name}.{bad}")));
        }
    }
    Ok(())
}

/// Last known value of every control property, by form and control name.
///
/// Fed by backend updates and by values a WebView event changed. It answers a shell's
/// `hello`, and it is what a backend reload carries over (6.2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mirror {
    /// form -> control -> property -> value.
    forms: BTreeMap<String, BTreeMap<String, Map<String, Value>>>,
}

impl Mirror {
    /// Records property values.
    ///
    /// # Arguments
    /// * `form` - form name.
    /// * `control` - control name.
    /// * `props` - values to record.
    pub fn set(&mut self, form: &str, control: &str, props: &Map<String, Value>) {
        let slot = self.forms.entry(form.to_owned()).or_default().entry(control.to_owned()).or_default();
        for (k, v) in props {
            slot.insert(k.clone(), v.clone());
        }
    }

    /// Current values of one form, shaped as `update` payload controls.
    ///
    /// # Arguments
    /// * `form` - form name.
    ///
    /// # Returns
    /// `{Control: {prop: value}}`.
    pub fn form(&self, form: &str) -> Map<String, Value> {
        let mut out = Map::new();
        if let Some(controls) = self.forms.get(form) {
            for (name, props) in controls {
                out.insert(name.clone(), Value::Object(props.clone()));
            }
        }
        out
    }

    /// Drops one form's values (its window closed for good).
    ///
    /// # Arguments
    /// * `form` - form name.
    pub fn forget(&mut self, form: &str) {
        self.forms.remove(form);
    }

    /// Keeps only values whose form, control and property still exist in `config` (6.2:
    /// carried over by form and control name where the control still exists).
    ///
    /// # Arguments
    /// * `config` - the new host configuration.
    pub fn retain_existing(&mut self, config: &HostConfig) {
        self.forms.retain(|form, controls| match config.form(form) {
            None => false,
            Some(fc) => {
                controls.retain(|name, props| match fc.controls.get(name) {
                    None => false,
                    Some(entry) => {
                        props.retain(|p, _| entry.props.iter().any(|q| q == p));
                        true
                    }
                });
                true
            }
        });
    }

    /// The whole mirror as `{Form: {Control: {prop: value}}}`, the restore payload a new
    /// backend instance receives.
    ///
    /// # Returns
    /// The JSON value.
    pub fn snapshot(&self) -> Value {
        let mut out = Map::new();
        for form in self.forms.keys() {
            out.insert(form.clone(), Value::Object(self.form(form)));
        }
        Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ControlEntry;
    use serde_json::json;

    /// A form with one edit control.
    ///
    /// # Returns
    /// The form.
    fn form() -> FormConfig {
        let mut controls = BTreeMap::new();
        controls.insert(
            "NameEdit".to_owned(),
            ControlEntry { kind: "edit".into(), props: vec!["text".into(), "enabled".into()], events: vec!["onChange".into()], popup_menu: None },
        );
        FormConfig { name: "MainForm".into(), props: Map::new(), events: vec![], controls }
    }

    /// Serializes a WebView event.
    ///
    /// # Arguments
    /// * `window` - window id.
    /// * `epoch` - epoch.
    /// * `method` - method.
    /// * `payload` - payload.
    ///
    /// # Returns
    /// The JSON text.
    fn event(window: &str, epoch: u64, method: &str, payload: Value) -> String {
        Envelope::new(Kind::Event, window, epoch, method, payload).to_json()
    }

    #[test]
    fn accepts_a_tabled_event_with_its_changed_props() {
        let text = event("MainForm", 3, "NameEdit.onChange", json!({"props": {"text": "abc"}}));
        match validate_inbound(&text, &form(), 3).unwrap() {
            Inbound::Event { control, event, props, .. } => {
                assert_eq!((control.as_str(), event.as_str()), ("NameEdit", "onChange"));
                assert_eq!(props.get("text"), Some(&json!("abc")));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_everything_outside_the_table() {
        let f = form();
        assert_eq!(validate_inbound(&event("MainForm", 1, "Other.onChange", json!({})), &f, 1), Err(Reject::Control("Other".into())));
        assert!(matches!(validate_inbound(&event("MainForm", 1, "NameEdit.onClick", json!({})), &f, 1), Err(Reject::Event(_))));
        assert!(matches!(
            validate_inbound(&event("MainForm", 1, "NameEdit.onChange", json!({"props": {"caption": "x"}})), &f, 1),
            Err(Reject::Property(_))
        ));
        assert_eq!(validate_inbound(&event("Other", 1, "NameEdit.onChange", json!({})), &f, 1), Err(Reject::Window("Other".into())));
        assert_eq!(validate_inbound(&event("MainForm", 1, "NameEdit.onChange", json!({})), &f, 2), Err(Reject::Stale(1)));
        assert!(matches!(validate_inbound("{", &f, 1), Err(Reject::Malformed(_))));
        let update = Envelope::new(Kind::Update, "MainForm", 1, "update", json!({})).to_json();
        assert_eq!(validate_inbound(&update, &f, 1), Err(Reject::Kind(Kind::Update)));
        let big = "x".repeat(MAX_MESSAGE + 1);
        assert_eq!(validate_inbound(&big, &f, 1), Err(Reject::TooLarge(MAX_MESSAGE + 1)));
    }

    #[test]
    fn hello_is_accepted_under_any_epoch() {
        let text = Envelope::new(Kind::Hello, "MainForm", 0, "session.start", json!({})).to_json();
        assert_eq!(validate_inbound(&text, &form(), 9), Ok(Inbound::Hello));
    }

    #[test]
    fn updates_are_checked_against_the_table() {
        let f = form();
        assert!(validate_update(&f, json!({"NameEdit": {"text": "a"}}).as_object().unwrap()).is_ok());
        assert!(validate_update(&f, json!({"NameEdit": {"items": []}}).as_object().unwrap()).is_err());
        assert!(validate_update(&f, json!({"Gone": {"text": "a"}}).as_object().unwrap()).is_err());
    }

    #[test]
    fn mirror_keeps_only_controls_that_still_exist() {
        let mut mirror = Mirror::default();
        mirror.set("MainForm", "NameEdit", json!({"text": "kept", "enabled": false}).as_object().unwrap());
        mirror.set("MainForm", "Removed", json!({"text": "dropped"}).as_object().unwrap());
        mirror.set("GoneForm", "X", json!({"text": "dropped"}).as_object().unwrap());
        let mut f = form();
        f.controls.get_mut("NameEdit").unwrap().props.retain(|p| p == "text");
        let config = HostConfig {
            schema: 1,
            application: crate::manifest::Application { main_form: "MainForm".into(), ..Default::default() },
            forms: vec![f],
            menus: vec![],
            trays: vec![],
            timers: vec![],
            actions: vec![],
            documents: vec![],
            permissions: Default::default(),
            limits: Default::default(),
        };
        mirror.retain_existing(&config);
        assert_eq!(mirror.snapshot(), json!({"MainForm": {"NameEdit": {"text": "kept"}}}));
    }
}
