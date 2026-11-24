//! HMR: file changes become Vite-protocol frames (`update`, `full-reload`, `error`) on the
//! WebSocket. `oj_graph` decides the boundaries; this module only stamps, invalidates and sends.
//! App channels ride the same socket: a page's `app:<name>` custom event goes to the host and to
//! every other page.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket};
use serde_json::{json, Value};

use crate::module_urls::{ext, url_of, COMPILABLE};
use crate::{AppMessage, State};

/// Event prefix of app channel messages.
const APP_EVENT_PREFIX: &str = "app:";

/// One frame for the pages; `from` is the client that sent it, which does not get it back.
#[derive(Debug, Clone)]
pub(crate) struct Frame {
    from: Option<u64>,
    text: String,
}

/// Invalidates changed modules and emits one reload or update frame per batch.
///
/// Source edits also invalidate loaded Tailwind stylesheets because utility
/// candidates are discovered outside the CSS import graph. A graph failure or
/// missing acceptance boundary takes precedence over incremental updates.
pub fn on_change(state: &State, files: &[PathBuf]) {
    if files.is_empty() {
        return;
    }
    if let Some(validate) = &state.validation {
        let validated = match (validate.0)(files) {
            Err(message) => {
                state.cache.lock().unwrap().clear();
                state
                    .validation_failed
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                send_error(state, &message);
                return;
            }
            Ok(validated) => validated,
        };
        if !validated
            && state
                .validation_failed
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        if validated
            && state
                .validation_failed
                .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            state.cache.lock().unwrap().clear();
            state.resolver.clear_cache();
            send(state, full_reload_frame("host diagnostics resolved", None));
            return;
        }
    }
    // Tailwind scans source files for utility candidates. A source edit must
    // also update loaded compiled stylesheets, even when their CSS is unchanged.
    let mut changed = files.to_vec();
    if files
        .iter()
        .any(|file| COMPILABLE.contains(&ext(&file.to_string_lossy()).as_str()))
    {
        let cache = state.cache.lock().unwrap();
        for (url, _) in cache.iter().filter(|(_, entry)| entry.scans_sources) {
            if let Some(file) = crate::module_urls::file_of(&state.root, &state.allow, url) {
                if !changed.contains(&file) {
                    changed.push(file);
                }
            }
        }
    }
    let files = &changed;
    state.resolver.clear_cache();
    let timestamp = now_ms();
    let mut updates = Vec::new();
    let mut reload: Option<(String, Option<String>)> = None;
    for file in files {
        let url = url_of(&state.root, file);
        state.cache.lock().unwrap().remove(&url);
        state.cjs_files.lock().unwrap().remove(file);
        if ext(&url) == "html" {
            reload.get_or_insert((format!("{url} changed"), Some(url.clone())));
            continue;
        }
        let mut graph = state.graph.lock().unwrap();
        if !graph.contains(Path::new(&url)) {
            continue;
        }
        match graph.update_targets(Path::new(&url)) {
            Ok(targets) if !targets.is_empty() => {
                let dirty = graph.stamp_update(Path::new(&url), timestamp);
                let mut cache = state.cache.lock().unwrap();
                for path in &dirty {
                    cache.remove(&path.to_string_lossy().into_owned());
                }
                for target in &targets {
                    updates.push(update_entry(target, timestamp, None));
                }
            }
            Ok(_) => {
                reload.get_or_insert((format!("{url} has no HMR boundary"), None));
            }
            Err(reason) => {
                reload.get_or_insert((reason, None));
            }
        }
    }
    if let Some((reason, page)) = reload {
        send(state, full_reload_frame(&reason, page.as_deref()));
    } else if !updates.is_empty() {
        send(
            state,
            json!({ "type": "update", "updates": updates }).to_string(),
        );
    }
}

/// The client's `import.meta.hot.invalidate()`: the module rejected its update (a refresh
/// boundary whose exports stopped being components), so the update moves to its importers.
pub fn on_invalidate(state: &State, data: &Value) {
    let Some(path) = data.get("path").and_then(Value::as_str) else {
        return;
    };
    let first = data.get("firstInvalidatedBy").and_then(Value::as_str);
    let timestamp = now_ms();
    let mut graph = state.graph.lock().unwrap();
    let Some(dirty) = graph.accept_invalidation(Path::new(path), timestamp) else {
        return;
    };
    {
        let mut cache = state.cache.lock().unwrap();
        for p in &dirty {
            cache.remove(&p.to_string_lossy().into_owned());
        }
    }
    let frame = match graph.update_targets_from_importers(Path::new(path)) {
        Ok(targets)
            if !targets.is_empty()
                && !targets
                    .iter()
                    .any(|t| Some(t.boundary.to_string_lossy().as_ref()) == first) =>
        {
            let updates: Vec<Value> = targets
                .iter()
                .map(|t| update_entry(t, timestamp, first))
                .collect();
            json!({ "type": "update", "updates": updates }).to_string()
        }
        Ok(_) => full_reload_frame(&format!("{path} invalidated with no other boundary"), None),
        Err(reason) => full_reload_frame(&reason, None),
    };
    drop(graph);
    send(state, frame);
}

/// An error overlay frame for every connected page, or the next one to connect.
pub fn send_error(state: &State, message: &str) {
    send(state, json!({ "type": "error", "err": { "message": message, "stack": "", "plugin": "devtools" } }).to_string());
}

/// Broadcasts `frame`, or holds it for the next client when none is connected.
pub fn send(state: &State, frame: String) {
    if state.frames.receiver_count() == 0 {
        *state.buffered.lock().unwrap() = Some(frame);
    } else {
        let _ = state.frames.send(Frame {
            from: None,
            text: frame,
        });
    }
}

/// Sends `data` on app channel `channel` to every connected page; nothing is held for later pages.
pub fn publish(state: &State, channel: &str, data: Value) {
    let text =
        json!({ "type": "custom", "event": format!("{APP_EVENT_PREFIX}{channel}"), "data": data })
            .to_string();
    let _ = state.frames.send(Frame { from: None, text });
}

/// A page's app channel message: to the host's [`crate::DevServer::messages`] and the other pages.
fn relay_app(state: &State, from: u64, channel: &str, text: String, data: &Value) {
    let _ = state.messages.send(AppMessage {
        channel: channel.to_string(),
        data: data.clone(),
    });
    let _ = state.frames.send(Frame {
        from: Some(from),
        text,
    });
}

/// One WebSocket client: greeting, buffered frame, then broadcast frames out and custom events in.
pub async fn socket(state: std::sync::Arc<State>, mut ws: WebSocket) {
    let id = state
        .next_client
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut frames = state.frames.subscribe();
    if ws
        .send(Message::Text(
            json!({ "type": "connected" }).to_string().into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    let held = state.buffered.lock().unwrap().take();
    if let Some(frame) = held {
        if ws.send(Message::Text(frame.into())).await.is_err() {
            return;
        }
    }
    loop {
        tokio::select! {
            frame = frames.recv() => match frame {
                Ok(Frame { from, .. }) if from == Some(id) => {}
                Ok(Frame { text, .. }) => if ws.send(Message::Text(text.into())).await.is_err() { return },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            },
            incoming = ws.recv() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(msg) = serde_json::from_str::<Value>(&text) {
                        if msg["type"] == "custom" {
                            match msg["event"].as_str() {
                                Some("vite:invalidate") => on_invalidate(&state, &msg["data"]),
                                Some(event) => {
                                    if let Some(channel) = event.strip_prefix(APP_EVENT_PREFIX).filter(|c| !c.is_empty()) {
                                        relay_app(&state, id, channel, text.to_string(), &msg["data"]);
                                    }
                                }
                                None => {}
                            }
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// Vite's `js-update` entry. A non-JS boundary (CSS) is named by its `?import` module URL.
fn update_entry(
    target: &oj_graph::UpdateTarget,
    timestamp: u64,
    first_invalidated_by: Option<&str>,
) -> Value {
    let module_url = |p: &Path| {
        let p = p.to_string_lossy().into_owned();
        if COMPILABLE.contains(&ext(&p).as_str()) {
            p
        } else {
            format!("{p}?import")
        }
    };
    let mut entry = json!({
        "type": "js-update",
        "path": module_url(&target.boundary),
        "acceptedPath": module_url(&target.accepted),
        "timestamp": timestamp,
    });
    if target.within_circular_import {
        entry["isWithinCircularImport"] = Value::Bool(true);
    }
    if let Some(first) = first_invalidated_by {
        entry["firstInvalidatedBy"] = Value::String(first.to_string());
    }
    entry
}

fn full_reload_frame(reason: &str, page: Option<&str>) -> String {
    json!({ "type": "full-reload", "reason": reason, "path": page.unwrap_or("*") }).to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(1)
}
