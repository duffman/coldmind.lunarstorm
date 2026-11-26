// Copyright (c) 2026 Coldmind AB <https://coldmind.com>
// SPDX-License-Identifier: MIT

//! Dev tracing: `jetbunny run --trace CATEGORIES [--trace-file FILE]` shows what a run does as
//! it happens. Off by default; a trace point whose category is off costs one atomic load.
//!
//! Categories: `loop` (each event-loop turn and each idle wait), `ops` (every native function
//! a program calls), `modules` (admission, evaluation, dynamic imports), `http` (each request
//! served), `errors` (script failures with their JavaScript stack) and `exit` (what is still
//! pending when the run ends). `all` selects every category; any selection includes `exit`.
//! Lines go to stderr; with `--trace-file`, one JSON object per line goes to the file instead.
//! Workers run in the same process and trace with their thread name.
use crate::engine::FailureReport;
use serde_json::{Value, json};
use std::{
    fmt::Write as _,
    io::Write as _,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    time::Instant,
};

/// A trace category, selected with `--trace`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cat {
    /// Each event-loop turn (job, message, completion, I/O) and each idle wait.
    Loop = 1,
    /// Every native function call from JavaScript.
    Ops = 2,
    /// Graph admission, module evaluation and dynamic imports.
    Modules = 4,
    /// Each HTTP request a server answers.
    Http = 8,
    /// Script failures, with message and JavaScript stack.
    Errors = 16,
    /// The end-of-run report.
    Exit = 32,
}
const ALL: u32 = 63;
impl Cat {
    fn name(self) -> &'static str {
        match self {
            Cat::Loop => "loop",
            Cat::Ops => "ops",
            Cat::Modules => "modules",
            Cat::Http => "http",
            Cat::Errors => "errors",
            Cat::Exit => "exit",
        }
    }
}

static MASK: AtomicU32 = AtomicU32::new(0);
static START: OnceLock<Instant> = OnceLock::new();
static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// Turns tracing on for the comma-separated `spec`, writing to `file` when given.
///
/// # Errors
/// An unknown category, or a trace file that cannot be created.
pub fn configure(spec: &str, file: Option<&str>) -> Result<(), String> {
    let mut mask = Cat::Exit as u32;
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        mask |= match part {
            "all" => ALL,
            "loop" => Cat::Loop as u32,
            "ops" => Cat::Ops as u32,
            "modules" => Cat::Modules as u32,
            "http" => Cat::Http as u32,
            "errors" => Cat::Errors as u32,
            "exit" => Cat::Exit as u32,
            other => {
                return Err(format!(
                    "unknown trace category {other}: use all, loop, ops, modules, http, errors or exit"
                ));
            }
        };
    }
    if let Some(path) = file {
        let created = std::fs::File::create(path).map_err(|e| format!("trace file {path}: {e}"))?;
        *FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(created);
    }
    START.get_or_init(Instant::now);
    MASK.store(mask, Ordering::Relaxed);
    Ok(())
}

/// Whether `cat` is traced.
#[inline]
pub fn on(cat: Cat) -> bool {
    MASK.load(Ordering::Relaxed) & cat as u32 != 0
}

/// Milliseconds since `start`, to the microsecond.
pub fn ms(start: Instant) -> f64 {
    (start.elapsed().as_secs_f64() * 1e6).round() / 1e3
}

/// Records `event` with `fields` (a JSON object) when `cat` is traced.
pub fn emit(cat: Cat, event: &str, fields: Value) {
    if !on(cat) {
        return;
    }
    let t = START
        .get()
        .map_or(0.0, |s| (s.elapsed().as_secs_f64() * 1e6).round() / 1e3);
    let current = std::thread::current();
    let thread = current.name().unwrap_or("?");
    let mut file = FILE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(file) = file.as_mut() {
        let mut line = json!({"t_ms": t, "cat": cat.name(), "thread": thread, "event": event});
        if let (Some(line), Value::Object(extra)) = (line.as_object_mut(), fields) {
            line.extend(extra);
        }
        let _ = writeln!(file, "{line}");
        return;
    }
    let mut text = format!("[jb {t:>10.3}ms {:<7}] {event}", cat.name());
    if let Value::Object(map) = fields {
        for (key, value) in map {
            let _ = match value {
                Value::String(s) => write!(text, " {key}={s}"),
                value => write!(text, " {key}={value}"),
            };
        }
    }
    if thread != "main" {
        let _ = write!(text, " thread={thread}");
    }
    eprintln!("{text}");
}

/// A failure report as trace fields: code, message and `function (file:line:column)` frames.
pub fn failure(report: &FailureReport) -> Value {
    let stack: Vec<String> = report
        .stack
        .iter()
        .map(|frame| {
            let function = frame
                .function
                .as_ref()
                .and_then(|f| f.to_utf8().ok())
                .filter(|f| !f.is_empty())
                .unwrap_or_else(|| "<anonymous>".into());
            match &frame.location {
                Some(l) => format!(
                    "{function} ({}:{}:{})",
                    l.origin,
                    l.line.map_or(0, |n| n + 1),
                    l.column.map_or(0, |n| n + 1)
                ),
                None => function,
            }
        })
        .collect();
    json!({
        "code": format!("{:?}", report.code),
        "message": report.message.as_ref().and_then(|m| m.to_utf8().ok()).unwrap_or_default(),
        "stack": stack,
    })
}
