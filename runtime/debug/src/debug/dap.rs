//! Bounded Debug Adapter Protocol framing and typed, asynchronous translation.
use super::*;
use crate::engine::{JsString, SourceDigest};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::value::RawValue;
use std::{collections::HashMap, fmt, mem::size_of};

#[path = "target_dap.rs"]
mod target_sessions;

/// Framing, schema, bound, or typed debugger error at the IDE boundary.
#[derive(Debug)]
pub enum DapError {
    /// Invalid Content-Length framing or protocol request shape.
    Protocol(String),
    /// A configured input/output, reference, or checked allocation limit was exceeded.
    Capacity,
    /// A typed debugger command/control failed.
    Debug(DebugError),
}
impl fmt::Display for DapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(message) => f.write_str(message),
            Self::Capacity => f.write_str("DAP capacity exceeded"),
            Self::Debug(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for DapError {}
impl From<DebugError> for DapError {
    fn from(error: DebugError) -> Self {
        Self::Debug(error)
    }
}
fn protocol(message: &str) -> DapError {
    DapError::Protocol(message.to_owned())
}
/// Incremental Content-Length framer with independent checked body/output bounds.
///
/// Framing is byte-based; a UTF-8 character may cross input chunks. Header storage
/// is bounded to 8 KiB. A malformed/oversized header closes input parsing before
/// body allocation. JSON command schemas are validated by [`DapAdapter`].
pub struct DapFramer {
    buffer: Vec<u8>,
    expected: Option<usize>,
    max_input: usize,
    max_output: usize,
    failed: bool,
}
impl DapFramer {
    /// Creates a framer with positive body limits, without opening a transport.
    pub fn new(max_input: usize, max_output: usize) -> Result<Self, DapError> {
        if max_input == 0 || max_output == 0 {
            return Err(DapError::Capacity);
        }
        Ok(Self {
            buffer: Vec::new(),
            expected: None,
            max_input,
            max_output,
            failed: false,
        })
    }
    /// Accepts stream fragments and returns complete raw JSON bodies in wire order.
    pub fn push(&mut self, input: &[u8]) -> Result<Vec<Vec<u8>>, DapError> {
        if self.failed {
            return Err(protocol("DAP input is closed"));
        }
        let result = self.push_inner(input);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn push_inner(&mut self, mut input: &[u8]) -> Result<Vec<Vec<u8>>, DapError> {
        let mut frames = Vec::new();
        while !input.is_empty() {
            if let Some(expected) = self.expected {
                let take = (expected - self.buffer.len()).min(input.len());
                self.buffer.extend_from_slice(&input[..take]);
                input = &input[take..];
                if self.buffer.len() == expected {
                    frames.push(std::mem::take(&mut self.buffer));
                    self.expected = None;
                }
            } else {
                if self.buffer.len() == 8192 {
                    return Err(DapError::Capacity);
                }
                self.buffer.push(input[0]);
                input = &input[1..];
                if self.buffer.ends_with(b"\r\n\r\n") {
                    let length = header_length(&self.buffer)?;
                    if length > self.max_input {
                        return Err(DapError::Capacity);
                    }
                    self.buffer.clear();
                    self.buffer
                        .try_reserve_exact(length)
                        .map_err(|_| DapError::Capacity)?;
                    if length == 0 {
                        frames.push(Vec::new());
                    } else {
                        self.expected = Some(length);
                    }
                }
            }
        }
        Ok(frames)
    }
    /// Encodes one already serialized body using its exact UTF-8 byte length.
    ///
    /// Exceeding the output allowance fails; no successful truncated frame is emitted.
    pub fn encode(&self, body: &[u8]) -> Result<Vec<u8>, DapError> {
        if body.len() > self.max_output {
            return Err(DapError::Capacity);
        }
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let total = header
            .len()
            .checked_add(body.len())
            .ok_or(DapError::Capacity)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(total)
            .map_err(|_| DapError::Capacity)?;
        output.extend(header.as_bytes());
        output.extend(body);
        Ok(output)
    }
}
fn header_length(header: &[u8]) -> Result<usize, DapError> {
    let header = std::str::from_utf8(header)
        .ok()
        .filter(|header| header.is_ascii())
        .ok_or_else(|| protocol("non-ASCII DAP header"))?;
    let mut length = None;
    for line in header[..header.len() - 4].split("\r\n") {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| protocol("invalid DAP header field"))?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(protocol("duplicate Content-Length"));
            }
            let value = value.trim();
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(protocol("invalid Content-Length"));
            }
            length = Some(value.parse().map_err(|_| DapError::Capacity)?);
        }
    }
    length.ok_or_else(|| protocol("missing Content-Length"))
}

#[derive(Deserialize)]
struct Request {
    seq: u64,
    #[serde(rename = "type")]
    kind: String,
    command: String,
    #[serde(default)]
    arguments: Option<Box<RawValue>>,
}
fn args<T: DeserializeOwned>(request: &Request) -> Result<T, DapError> {
    serde_json::from_str(request.arguments.as_deref().map_or("{}", RawValue::get)).map_err(
        |error| DapError::Protocol(format!("invalid {} arguments: {error}", request.command)),
    )
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DisconnectArgs {
    #[serde(default)]
    terminate_debuggee: bool,
    #[serde(default)]
    suspend_debuggee: bool,
    #[serde(default)]
    restart: bool,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Initialize {
    #[serde(default)]
    lines_start_at1: Option<bool>,
    #[serde(default)]
    columns_start_at1: Option<bool>,
    #[serde(default)]
    path_format: Option<String>,
    #[serde(default)]
    supports_invalidated_event: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelArgs {
    request_id: Option<u64>,
}
#[derive(Deserialize)]
struct RejectionsArgs {
    #[serde(default)]
    start: usize,
    count: Option<usize>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadArgs {
    #[serde(default = "one")]
    thread_id: u64,
    #[serde(default)]
    start_frame: usize,
    #[serde(default)]
    levels: Option<usize>,
}
fn one() -> u64 {
    1
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrameArgs {
    frame_id: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VariablesArgs {
    variables_reference: u64,
    #[serde(default)]
    start: usize,
    #[serde(default)]
    count: Option<usize>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetVariableArgs {
    variables_reference: u64,
    name: super::source_map::LosslessString,
    value: super::source_map::LosslessString,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvaluateArgs {
    frame_id: Option<u64>,
    expression: super::source_map::LosslessString,
    #[serde(default)]
    context: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireSource {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    source_reference: Option<u64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceArgs {
    #[serde(default)]
    source: Option<WireSource>,
    #[serde(default)]
    source_reference: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocationsArgs {
    source: WireSource,
    line: u32,
    column: Option<u32>,
    end_line: Option<u32>,
    end_column: Option<u32>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StepPolicyArgs {
    skip_sources: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BreakpointsArgs {
    source: WireSource,
    #[serde(default)]
    breakpoints: Vec<WireBreakpoint>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireBreakpoint {
    line: u32,
    #[serde(default)]
    column: Option<u32>,
    #[serde(default)]
    condition: Option<super::source_map::LosslessString>,
    #[serde(default)]
    hit_condition: Option<String>,
    #[serde(default)]
    log_message: Option<super::source_map::LosslessString>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnableBreakpointArgs {
    source: WireSource,
    breakpoint_id: u64,
    enabled: bool,
}
#[derive(Deserialize)]
struct ContinueToArgs {
    source: WireSource,
    line: u32,
    column: u32,
}
#[derive(Deserialize)]
struct ExceptionArgs {
    #[serde(default)]
    filters: Vec<String>,
}

#[derive(Clone, PartialEq, Eq)]
enum SourceKey {
    Generated(ScriptId, u64),
    Original(String, Option<SourceDigest>),
}
#[derive(Clone)]
struct SourceEntry {
    reference: u64,
    key: SourceKey,
    origin: Option<String>,
}
#[derive(Clone)]
enum SourceQuery {
    Generated(ScriptId, u64),
    Original(ScriptSelector),
}
impl SourceQuery {
    fn request(&self, start: usize, count: usize) -> DebugRequest {
        match self {
            Self::Generated(script, revision) => DebugRequest::Source {
                script: *script,
                revision: *revision,
                page: Page { start, count },
            },
            Self::Original(selector) => DebugRequest::OriginalSource {
                selector: selector.clone(),
                page: Page { start, count },
            },
        }
    }
}
#[derive(Clone)]
struct BreakpointMeta {
    id: u64,
    origin: String,
    line: u32,
    column: Option<u32>,
}
enum PendingKind {
    Simple,
    ExceptionInfo(PauseId),
    AsyncStack(PauseId),
    RejectionFilter(bool),
    Rejections,
    Locations,
    Enable {
        meta: BreakpointMeta,
        enabled: bool,
    },
    Stack {
        pause: PauseId,
        start: usize,
        requested: Option<usize>,
        frames: Vec<DebugFrame>,
    },
    Scopes(FrameRef),
    Variables {
        parent: VariableParent,
        start: usize,
        requested: Option<usize>,
        variables: Vec<DebugVariable>,
    },
    Inspection {
        reference: u64,
        start: usize,
        count: usize,
        unpaged: bool,
        cursor: Option<CursorRef>,
        retried: bool,
        reclaimed: bool,
        staged_targets: Vec<WriteTarget>,
    },
    ReleaseTargets {
        next: ReleaseNext,
        remaining: Vec<WriteTarget>,
    },
    Cleanup {
        reference: Option<u64>,
    },
    ReleaseCursor {
        next: ReleaseNext,
    },
    Evaluation(FrameRef),
    Lookup(FrameRef),
    Write(FrameRef),
    Breakpoints {
        origin: String,
        metadata: Vec<BreakpointMeta>,
    },
    Source {
        query: SourceQuery,
        units: Vec<u16>,
        total: Option<usize>,
    },
    Sources {
        scripts: Vec<ScriptInfo>,
    },
}
enum ReleaseNext {
    Page {
        pending: Box<Pending>,
        page: InspectionPage,
        released: Vec<WriteTarget>,
    },
    Retry {
        pending: Box<Pending>,
        request: InspectionRequest,
        cursor: Option<CursorRef>,
        evicted: Vec<WriteTarget>,
    },
    Discard {
        pending: Box<Pending>,
        error: DapError,
        cursor: Option<CursorRef>,
    },
    Inspect(InspectionRequest, Box<Pending>),
    Fail(Box<Pending>, DapError),
    EvaluationError {
        pending: Box<Pending>,
        error: DebugError,
        cursor: CursorRef,
        remaining: Vec<CursorRef>,
    },
}
impl Pending {
    fn inspection_reference(&self) -> Option<u64> {
        match &self.kind {
            PendingKind::Inspection { reference, .. } => Some(*reference),
            PendingKind::Cleanup { reference } => *reference,
            PendingKind::ReleaseCursor { next } | PendingKind::ReleaseTargets { next, .. } => {
                match next {
                    ReleaseNext::Page { pending, .. }
                    | ReleaseNext::Retry { pending, .. }
                    | ReleaseNext::Discard { pending, .. } => pending.inspection_reference(),
                    ReleaseNext::Inspect(_, pending) | ReleaseNext::Fail(pending, _) => {
                        pending.inspection_reference()
                    }
                    ReleaseNext::EvaluationError { .. } => None,
                }
            }
            _ => None,
        }
    }
}
struct Pending {
    seq: u64,
    command: String,
    order: u64,
    setup: bool,
    kind: PendingKind,
}

/// DAP translator driven on host-provided concurrency, separate from the VM owner.
///
/// Construct with a break-on-start controller before application entry. Feed IDE
/// input with [`Self::receive`] and drain asynchronous output with [`Self::poll`].
/// The entry pause is released only after configurationDone and preceding setup
/// terminal replies. This translator never pumps application work or creates a VM.
/// Full source responses gather bounded common pages under the separate exact
/// encoded-message allowance; lossless UTF-16 strings use JSON escapes as needed.
pub struct DapAdapter {
    handle: VmDebugHandle,
    framer: DapFramer,
    limits: DebugLimits,
    output_seq: u64,
    reference_seq: u64,
    breakpoint_seq: u64,
    order: u64,
    lines_one: bool,
    columns_one: bool,
    invalidated_events: bool,
    path_uri: bool,
    initialized: bool,
    disconnected: bool,
    pending: HashMap<DebugRequestId, Pending>,
    configuration: Option<(u64, u64)>,
    configured: bool,
    exception_policy: ExceptionPolicy,
    exception_break_mode: &'static str,
    entry: Option<PauseId>,
    pause: Option<PauseId>,
    last_state_sequence: u64,
    frames: Vec<(u64, FrameRef)>,
    parents: Vec<(u64, VariableParent)>,
    inspection_checkpoints: HashMap<u64, (usize, CursorRef)>,
    parent_frames: HashMap<u64, FrameRef>,
    write_views: HashMap<u64, Vec<(usize, JsString, WriteTarget)>>,
    write_capacity_views: HashMap<u64, Vec<(usize, JsString)>>,
    view_order: Vec<u64>,
    deferred_submission: Option<(DebugRequest, Pending)>,
    sources: Vec<SourceEntry>,
    breakpoints: Vec<BreakpointMeta>,
    targets: Option<target_sessions::TargetSessions>,
}
impl DapAdapter {
    /// Takes a non-cloneable break-on-start session controller.
    ///
    /// All input/protocol limits derive from the already validated debug limits.
    /// A controller without an entry barrier is rejected rather than racing code.
    pub fn new(handle: VmDebugHandle) -> Result<Self, DapError> {
        Self::new_session(handle, true)
    }
    fn new_session(handle: VmDebugHandle, require_entry: bool) -> Result<Self, DapError> {
        if require_entry && !handle.session.break_on_start {
            return Err(protocol("DAP requires a break-on-start debugger session"));
        }
        if handle.session.break_on_start {
            handle.session.claim_entry_gate()?;
        }
        let limits = handle.limits().clone();
        let input = limits
            .request_bytes
            .checked_mul(6)
            .and_then(|v| v.checked_add(8192))
            .ok_or(DapError::Capacity)?;
        let framer = DapFramer::new(input, limits.dap_output_bytes)?;
        Ok(Self {
            handle,
            framer,
            limits,
            output_seq: 0,
            reference_seq: 0,
            breakpoint_seq: 0,
            order: 0,
            lines_one: true,
            columns_one: true,
            invalidated_events: false,
            path_uri: false,
            initialized: false,
            disconnected: false,
            pending: HashMap::new(),
            configuration: None,
            configured: false,
            exception_policy: ExceptionPolicy::None,
            exception_break_mode: "never",
            entry: None,
            pause: None,
            last_state_sequence: 0,
            frames: Vec::new(),
            parents: Vec::new(),
            inspection_checkpoints: HashMap::new(),
            parent_frames: HashMap::new(),
            write_views: HashMap::new(),
            write_capacity_views: HashMap::new(),
            view_order: vec![],
            deferred_submission: None,
            sources: Vec::new(),
            breakpoints: Vec::new(),
            targets: None,
        })
    }
    /// Whether disconnect or a fatal transport error has detached this observer.
    pub fn is_disconnected(&self) -> bool {
        self.disconnected
    }
    /// Ends protocol input and releases the observer without terminating the application.
    pub fn end_input(&mut self) -> Result<(), DapError> {
        self.detach_target_sessions();
        self.disconnected = true;
        self.handle.detach()?;
        Ok(())
    }
    /// Parses framed IDE input and returns any immediately available framed output.
    ///
    /// Admission receipts do not become premature native-result responses. Schema
    /// errors become DAP failures; malformed framing/JSON requests urgent detach.
    /// In-flight sequences stay reserved while configuration waits for setup replies.
    pub fn receive(&mut self, input: &[u8]) -> Result<Vec<Vec<u8>>, DapError> {
        let result = self.receive_inner(input);
        if result.is_err() {
            self.detach_target_sessions();
            let _ = self.handle.detach();
            self.disconnected = true;
        }
        result
    }
    fn receive_inner(&mut self, input: &[u8]) -> Result<Vec<Vec<u8>>, DapError> {
        let bodies = match self.framer.push(input) {
            Ok(bodies) => bodies,
            Err(error) => {
                let _ = self.handle.detach();
                self.disconnected = true;
                return Err(error);
            }
        };
        let mut output = self.poll()?;
        for body in bodies {
            std::str::from_utf8(&body).map_err(|_| protocol("DAP input is not UTF-8"))?;
            let request: Request = match serde_json::from_slice(&body) {
                Ok(request) => request,
                Err(error) => {
                    let _ = self.handle.detach();
                    self.disconnected = true;
                    return Err(DapError::Protocol(format!("invalid DAP request: {error}")));
                }
            };
            if request.kind != "request" {
                return Err(protocol("DAP message is not a request"));
            }
            self.synchronize_pause();
            let seq = request.seq;
            let command = request.command.clone();
            let handled = self.dispatch(request, &mut output);
            if let Err(error) = handled {
                let response =
                    self.response(seq, &command, false, Some(error.to_string()), obj(vec![]))?;
                output.push(response);
            }
            self.finish_configuration(&mut output)?;
        }
        Ok(output)
    }
    /// Drains correlated replies and asynchronous native state/source notifications.
    ///
    /// Call while IDE input is idle too; an evaluation may complete much later.
    /// Consuming replies releases common transport permits before the next page.
    pub fn poll(&mut self) -> Result<Vec<Vec<u8>>, DapError> {
        let result = self.poll_inner();
        if result.is_err() {
            self.detach_target_sessions();
            let _ = self.handle.detach();
            self.disconnected = true;
        }
        result
    }
    fn poll_inner(&mut self) -> Result<Vec<Vec<u8>>, DapError> {
        self.synchronize_pause();
        let mut output = Vec::new();
        while let Some(update) = self.handle.try_recv() {
            if self.process_update(update, &mut output)? {
                self.finish_configuration(&mut output)?;
            }
        }
        self.retry_deferred(&mut output)?;
        self.finish_configuration(&mut output)?;
        self.poll_target_sessions(&mut output)?;
        Ok(output)
    }
    fn process_update(
        &mut self,
        update: DebugUpdate,
        output: &mut Vec<Vec<u8>>,
    ) -> Result<bool, DapError> {
        let mut processed = true;
        match update {
            DebugUpdate::Reply { id, result } => {
                let Some(pending) = self.pending.remove(&id) else {
                    return Ok(false);
                };
                if let PendingKind::ReleaseTargets { next, remaining } = pending.kind {
                    if !remaining.is_empty()
                        && matches!(result, Ok(DebugReply::WriteTargetsReleased))
                    {
                        self.release_targets(remaining, next)?;
                    } else {
                        self.finish_cursor_release(next, result, output)?;
                    }
                    return Ok(true);
                }
                if let PendingKind::ReleaseCursor { next } = pending.kind {
                    self.finish_cursor_release(next, result, output)?;
                    return Ok(true);
                }
                if let (
                    PendingKind::Inspection {
                        cursor: Some(cursor),
                        ..
                    },
                    Err(error),
                ) = (&pending.kind, &result)
                {
                    let cursor = *cursor;
                    let error = error.clone();
                    self.release_cursor(
                        cursor,
                        ReleaseNext::Fail(Box::new(pending), error.into()),
                    )?;
                    return Ok(true);
                }
                match result {
                    Ok(reply) => {
                        let seq = pending.seq;
                        let command = pending.command.clone();
                        if let Err(error) = self.reply(pending, reply, output) {
                            let response = self.response(
                                seq,
                                &command,
                                false,
                                Some(error.to_string()),
                                obj(vec![]),
                            )?;
                            output.push(response);
                        }
                    }
                    Err(error) => {
                        if matches!(
                            pending.kind,
                            PendingKind::Evaluation(_) | PendingKind::Write(_)
                        ) {
                            self.write_views.clear();
                            self.write_capacity_views.clear();
                        }
                        if matches!(
                            pending.kind,
                            PendingKind::Evaluation(_) | PendingKind::Write(_)
                        ) && matches!(error, DebugError::EngineFailure(_) | DebugError::Capacity)
                        {
                            let mut remaining: Vec<_> = self
                                .inspection_checkpoints
                                .values()
                                .map(|(_, cursor)| *cursor)
                                .collect();
                            if let Some(cursor) = remaining.pop() {
                                let seq = pending.seq;
                                let command = pending.command.clone();
                                let original_error = error.clone();
                                if self
                                    .release_cursor(
                                        cursor,
                                        ReleaseNext::EvaluationError {
                                            pending: Box::new(pending),
                                            error,
                                            cursor,
                                            remaining,
                                        },
                                    )
                                    .is_err()
                                {
                                    output.push(self.response(
                                        seq,
                                        &command,
                                        false,
                                        Some(original_error.to_string()),
                                        obj(vec![]),
                                    )?);
                                }
                                return Ok(true);
                            }
                        }
                        if matches!(
                            pending.kind,
                            PendingKind::Evaluation(_) | PendingKind::Write(_)
                        ) && matches!(
                            error,
                            DebugError::EvaluationCancelled { .. }
                                | DebugError::EvaluationDeadlineExceeded { .. }
                        ) {
                            // Native evaluation invalidated these continuations.
                            // Frame/object references retain their existing lifetime.
                            self.inspection_checkpoints.clear();
                        }
                        let (message, body) = match error {
                            DebugError::EvaluationCancelled { effects_may_remain } => (
                                "cancelled".to_owned(),
                                obj(vec![
                                    (
                                        "error",
                                        obj(vec![
                                            ("id", number(2001)),
                                            (
                                                "format",
                                                text(
                                                    "Evaluation cancelled. Side effects already performed were not rolled back.",
                                                ),
                                            ),
                                            ("showUser", boolean(true)),
                                        ]),
                                    ),
                                    ("effectsMayRemain", boolean(effects_may_remain)),
                                ]),
                            ),
                            DebugError::EvaluationDeadlineExceeded { effects_may_remain } => (
                                "Evaluation deadline exceeded".to_owned(),
                                obj(vec![
                                    (
                                        "error",
                                        obj(vec![
                                            ("id", number(2002)),
                                            (
                                                "format",
                                                text(
                                                    "Evaluation deadline exceeded. Side effects already performed were not rolled back.",
                                                ),
                                            ),
                                            ("showUser", boolean(true)),
                                        ]),
                                    ),
                                    ("effectsMayRemain", boolean(effects_may_remain)),
                                ]),
                            ),
                            other => (other.to_string(), obj(vec![])),
                        };
                        let response = self.response(
                            pending.seq,
                            &pending.command,
                            false,
                            Some(message),
                            body,
                        )?;
                        output.push(response);
                    }
                }
            }
            DebugUpdate::Paused {
                sequence,
                pause,
                reason,
                breakpoint_ids,
                ..
            } => {
                processed = advance_state_sequence(&mut self.last_state_sequence, sequence);
                if processed {
                    self.reset_pause(Some(pause));
                    self.exception_break_mode = if reason == PauseReason::UnhandledRejection {
                        "unhandled"
                    } else if reason != PauseReason::Exception {
                        "never"
                    } else if self.exception_policy == ExceptionPolicy::All {
                        "always"
                    } else {
                        "unhandled"
                    };
                    if reason == PauseReason::Entry {
                        self.entry = Some(pause);
                    } else {
                        let event = self.event(
                            "stopped",
                            obj(vec![
                                (
                                    "reason",
                                    text(match reason {
                                        PauseReason::Breakpoint => "breakpoint",
                                        PauseReason::Step => "step",
                                        PauseReason::Exception
                                        | PauseReason::UnhandledRejection => "exception",
                                        _ => "pause",
                                    }),
                                ),
                                ("threadId", number(1)),
                                ("allThreadsStopped", boolean(true)),
                                (
                                    "hitBreakpointIds",
                                    array(breakpoint_ids.into_iter().map(number).collect()),
                                ),
                            ]),
                        )?;
                        output.push(event);
                    }
                }
            }
            DebugUpdate::Resumed { sequence, .. } => {
                processed = advance_state_sequence(&mut self.last_state_sequence, sequence);
                if processed {
                    self.reset_pause(None);
                    let event = self.event(
                        "continued",
                        obj(vec![
                            ("threadId", number(1)),
                            ("allThreadsContinued", boolean(true)),
                        ]),
                    )?;
                    output.push(event);
                }
            }
            DebugUpdate::ScriptRegistered(info) => {
                let known = self.sources.iter().any(|source| {
                    source.key
                        == SourceKey::Generated(info.registration.id, info.registration.revision)
                });
                let source = self.source_info(&info)?;
                if !known && self.initialized {
                    let event = self.event(
                        "loadedSource",
                        obj(vec![("reason", text("new")), ("source", source)]),
                    )?;
                    output.push(event);
                }
            }
            DebugUpdate::BreakpointsChanged(values) => {
                for value in values {
                    if let Some(meta) = self
                        .breakpoints
                        .iter()
                        .find(|meta| meta.id == value.client_id)
                        .cloned()
                    {
                        let breakpoint = self.breakpoint_value(&meta, &value.state)?;
                        let event = self.event(
                            "breakpoint",
                            obj(vec![
                                ("reason", text("changed")),
                                ("breakpoint", breakpoint),
                            ]),
                        )?;
                        output.push(event);
                    }
                }
            }
            DebugUpdate::PromiseRejection(info) => {
                if info.state == PromiseRejectionState::Pending {
                    return Err(protocol("pending rejection is not a qualified transition"));
                }
                let body = self.rejection_value(info)?;
                output.push(self.event("jetbunny/promiseRejection", body)?);
            }
            DebugUpdate::Output { message } => {
                output.push(self.event(
                    "output",
                    obj(vec![
                        ("category", text("console")),
                        ("output", Value::Text(message)),
                    ]),
                )?);
            }
            DebugUpdate::Diagnostic { report, dropped } => {
                let mut message = report
                    .message
                    .unwrap_or_else(|| JsString::from_utf8("Debugger diagnostic"));
                if dropped > 0 {
                    let suffix = format!(" ({dropped} diagnostics omitted)");
                    let mut units = message.into_units();
                    units.extend(suffix.encode_utf16());
                    message = JsString::from_units(units);
                }
                let event = self.event(
                    "output",
                    obj(vec![
                        ("category", text("console")),
                        ("output", Value::Text(message)),
                    ]),
                )?;
                output.push(event);
            }
            DebugUpdate::SessionClosed { reason, .. } => {
                self.reset_pause(None);
                self.disconnected = true;
                if let Some(reason) = reason {
                    let event = self.event(
                        "output",
                        obj(vec![
                            ("category", text("stderr")),
                            ("output", Value::Text(reason.message.unwrap_or_default())),
                        ]),
                    )?;
                    output.push(event);
                }
                output.push(self.event("terminated", obj(vec![]))?);
            }
        }
        Ok(processed)
    }
    fn synchronize_pause(&mut self) {
        let current = match self.handle.state() {
            DebugExecutionState::Paused(pause) => Some(pause),
            _ => None,
        };
        if current != self.pause {
            self.reset_pause(current);
        }
    }
    fn reset_pause(&mut self, pause: Option<PauseId>) {
        if self.pause != pause {
            self.frames.clear();
            self.parents.clear();
            self.parent_frames.clear();
            self.view_order.clear();
            self.write_views.clear();
            self.write_capacity_views.clear();
            self.inspection_checkpoints.clear();
        }
        self.pause = pause;
    }
    fn current_pause(&self) -> Result<PauseId, DapError> {
        match self.handle.state() {
            DebugExecutionState::Paused(pause) => Ok(pause),
            _ => Err(DebugError::NotPaused.into()),
        }
    }
    fn next_reference(&mut self) -> Result<u64, DapError> {
        self.reference_seq = self
            .reference_seq
            .checked_add(1)
            .filter(|v| *v <= i32::MAX as u64)
            .ok_or(DapError::Capacity)?;
        Ok(self.reference_seq)
    }
    fn frame_reference(&mut self, frame: FrameRef) -> Result<u64, DapError> {
        if let Some((id, _)) = self.frames.iter().find(|(_, value)| *value == frame) {
            return Ok(*id);
        }
        if self
            .frames
            .len()
            .checked_add(self.parents.len())
            .is_none_or(|n| n >= self.limits.inspection_objects)
        {
            return Err(DapError::Capacity);
        }
        let id = self.next_reference()?;
        self.frames.push((id, frame));
        Ok(id)
    }
    // Capacity rows own lossless names even without a native target. Account
    // both row categories and their actual Vec/name capacity under one budget.
    // Evict oldest capacity-only rows first; native editable identities remain
    // owned by their retained views and require the acknowledged release path.
    fn bound_capacity_views(
        &mut self,
        reference: u64,
        current: std::ops::Range<usize>,
        view: &Vec<(usize, JsString, WriteTarget)>,
        capacity_view: &mut Vec<(usize, JsString)>,
    ) -> Result<bool, DapError> {
        let row_limit = self
            .limits
            .inspection_objects
            .checked_mul(self.limits.inspection_page_items)
            .ok_or(DapError::Capacity)?
            .max(self.limits.write_targets);
        let mut evicted = false;
        loop {
            let mut rows = view.len().saturating_add(capacity_view.len());
            let mut bytes = view
                .capacity()
                .saturating_mul(size_of::<(usize, JsString, WriteTarget)>())
                .saturating_add(
                    capacity_view
                        .capacity()
                        .saturating_mul(size_of::<(usize, JsString)>()),
                );
            for (_, name, _) in view {
                bytes = bytes.saturating_add(name.capacity_bytes());
            }
            for (_, name) in capacity_view.iter() {
                bytes = bytes.saturating_add(name.capacity_bytes());
            }
            for (id, other) in &self.write_views {
                if *id != reference {
                    rows = rows.saturating_add(other.len());
                    bytes = bytes.saturating_add(other.capacity().saturating_mul(size_of::<(
                        usize,
                        JsString,
                        WriteTarget,
                    )>(
                    )));
                    for (_, name, _) in other {
                        bytes = bytes.saturating_add(name.capacity_bytes());
                    }
                }
            }
            for (id, other) in &self.write_capacity_views {
                if *id != reference {
                    rows = rows.saturating_add(other.len());
                    bytes = bytes.saturating_add(other.capacity().saturating_mul(size_of::<(
                        usize,
                        JsString,
                    )>(
                    )));
                    for (_, name) in other {
                        bytes = bytes.saturating_add(name.capacity_bytes());
                    }
                }
            }
            if rows <= row_limit && bytes <= self.limits.dap_output_bytes {
                return Ok(evicted);
            }
            let mut removed = false;
            for id in &self.view_order {
                if *id != reference
                    && let Some(other) = self.write_capacity_views.get_mut(id)
                    && !other.is_empty()
                {
                    other.remove(0);
                    other.shrink_to_fit();
                    removed = true;
                    break;
                }
            }
            if !removed
                && let Some(at) = capacity_view
                    .iter()
                    .position(|(at, _)| !current.contains(at))
            {
                capacity_view.remove(at);
                capacity_view.shrink_to_fit();
                removed = true;
            }
            if !removed {
                return Err(DapError::Capacity);
            }
            evicted = true;
        }
    }
    fn parent_reference(&mut self, parent: VariableParent) -> Result<u64, DapError> {
        self.parent_reference_in_frame(parent, None)
    }
    fn parent_reference_in_frame(
        &mut self,
        parent: VariableParent,
        frame: Option<FrameRef>,
    ) -> Result<u64, DapError> {
        if let Some((id, _)) = self.parents.iter().find(|(id, value)| {
            same_parent(*value, parent) && self.parent_frames.get(id).copied() == frame
        }) {
            return Ok(*id);
        }
        if self
            .frames
            .len()
            .checked_add(self.parents.len())
            .is_none_or(|n| n >= self.limits.inspection_objects)
        {
            return Err(DapError::Capacity);
        }
        let id = self.next_reference()?;
        self.parents.push((id, parent));
        if let Some(frame) = frame {
            self.parent_frames.insert(id, frame);
        }
        Ok(id)
    }
    fn frame(&self, id: u64) -> Result<FrameRef, DapError> {
        self.frames
            .iter()
            .find(|(known, frame)| *known == id && Some(frame.pause) == self.pause)
            .map(|(_, frame)| *frame)
            .ok_or_else(|| protocol("stale or unknown frameId"))
    }
    fn page_count(&self) -> Result<usize, DapError> {
        let count = self
            .limits
            .reply_bytes
            .checked_sub(size_of::<DebugReply>())
            .map(|bytes| bytes / 2)
            .unwrap_or(0)
            .min(16_384);
        if count == 0 {
            Err(DapError::Capacity)
        } else {
            Ok(count)
        }
    }
    fn submit(&mut self, request: DebugRequest, pending: Pending) -> Result<(), DapError> {
        let receipt = self
            .handle
            .try_request(request)
            .map_err(|rejected| request_rejection(rejected.reason))?;
        let id = match receipt {
            DebugRequestReceipt::Accepted { id } => id,
            // Ownership and the terminal reservation are already accepted. The
            // fatal transport state will close this ID exactly once; emitting an
            // early retryable DAP failure would duplicate its terminal response.
            DebugRequestReceipt::AcceptedWakeFailed { id, .. } => id,
        };
        self.pending.insert(id, pending);
        Ok(())
    }
    fn submit_cleanup(&mut self, request: DebugRequest, pending: Pending) -> Result<(), DapError> {
        match self.handle.try_request(request) {
            Ok(
                DebugRequestReceipt::Accepted { id }
                | DebugRequestReceipt::AcceptedWakeFailed { id, .. },
            ) => {
                self.pending.insert(id, pending);
            }
            Err(rejected) => {
                if self.deferred_submission.is_some() {
                    return Err(protocol("overlapping cleanup transactions"));
                }
                self.deferred_submission = Some((rejected.value, pending));
            }
        }
        Ok(())
    }
    fn retry_deferred(&mut self, output: &mut Vec<Vec<u8>>) -> Result<(), DapError> {
        let Some((request, pending)) = self.deferred_submission.take() else {
            return Ok(());
        };
        match self.handle.try_request(request) {
            Ok(
                DebugRequestReceipt::Accepted { id }
                | DebugRequestReceipt::AcceptedWakeFailed { id, .. },
            ) => {
                self.pending.insert(id, pending);
            }
            Err(rejected) => match rejected.reason {
                crate::engine::RejectionReason::Debug(
                    DebugError::Capacity | DebugError::InFlight,
                ) => self.deferred_submission = Some((rejected.value, pending)),
                reason => output.push(self.response(
                    pending.seq,
                    &pending.command,
                    false,
                    Some(request_rejection(reason).to_string()),
                    obj(vec![]),
                )?),
            },
        }
        Ok(())
    }
    fn release_targets(
        &mut self,
        targets: Vec<WriteTarget>,
        next: ReleaseNext,
    ) -> Result<(), DapError> {
        let original = match &next {
            ReleaseNext::Page { pending, .. }
            | ReleaseNext::Retry { pending, .. }
            | ReleaseNext::Discard { pending, .. } => pending,
            _ => return Err(protocol("invalid target-release continuation")),
        };
        let count = self
            .limits
            .request_bytes
            .saturating_sub(size_of::<DebugRequest>())
            / size_of::<WriteTarget>();
        let count = count.min(self.limits.write_targets).min(128);
        if count == 0 {
            return Err(DapError::Capacity);
        }
        let mut targets = targets;
        let remaining = targets.split_off(targets.len().min(count));
        targets.shrink_to_fit();
        let pending = Pending {
            seq: original.seq,
            command: original.command.clone(),
            order: original.order,
            setup: original.setup,
            kind: PendingKind::ReleaseTargets { next, remaining },
        };
        self.submit_cleanup(DebugRequest::ReleaseWriteTargets(targets), pending)
    }
    fn release_cursor(&mut self, cursor: CursorRef, next: ReleaseNext) -> Result<(), DapError> {
        let original = match &next {
            ReleaseNext::Page { pending, .. }
            | ReleaseNext::Retry { pending, .. }
            | ReleaseNext::Discard { pending, .. } => pending,
            ReleaseNext::Inspect(_, pending) | ReleaseNext::Fail(pending, _) => pending,
            ReleaseNext::EvaluationError { pending, .. } => pending,
        };
        let pending = Pending {
            seq: original.seq,
            command: original.command.clone(),
            order: original.order,
            setup: original.setup,
            kind: PendingKind::ReleaseCursor { next },
        };
        self.submit(DebugRequest::ReleaseCursor(cursor), pending)
    }
    fn release_cursor_owned(
        &mut self,
        cursor: CursorRef,
        next: ReleaseNext,
    ) -> Result<(), DapError> {
        let original = match &next {
            ReleaseNext::Page { pending, .. }
            | ReleaseNext::Retry { pending, .. }
            | ReleaseNext::Discard { pending, .. } => pending,
            ReleaseNext::Inspect(_, pending) | ReleaseNext::Fail(pending, _) => pending,
            ReleaseNext::EvaluationError { pending, .. } => pending,
        };
        let pending = Pending {
            seq: original.seq,
            command: original.command.clone(),
            order: original.order,
            setup: original.setup,
            kind: PendingKind::ReleaseCursor { next },
        };
        self.submit_cleanup(DebugRequest::ReleaseCursor(cursor), pending)
    }
    fn finish_cursor_release(
        &mut self,
        next: ReleaseNext,
        result: Result<DebugReply, DebugError>,
        output: &mut Vec<Vec<u8>>,
    ) -> Result<(), DapError> {
        let release_error = match result {
            Ok(DebugReply::CursorReleased | DebugReply::WriteTargetsReleased)
            | Err(DebugError::StaleInspection | DebugError::StalePause | DebugError::NotPaused) => {
                None
            }
            Err(error) => Some(DapError::Debug(error)),
            Ok(_) => Some(protocol("invalid cursor release reply")),
        };
        let (pending, error) = match next {
            ReleaseNext::Page {
                pending,
                page,
                released,
            } => {
                if let Some(error) = release_error {
                    (pending, error)
                } else {
                    for view in self.write_views.values_mut() {
                        view.retain(|(_, _, target)| !released.contains(target));
                    }
                    return self.reply(*pending, DebugReply::Inspection(page), output);
                }
            }
            ReleaseNext::Retry {
                pending,
                request,
                cursor,
                evicted,
            } => {
                if let Some(error) = release_error {
                    (pending, error)
                } else {
                    for view in self.write_views.values_mut() {
                        view.retain(|(_, _, target)| !evicted.contains(target));
                    }
                    if self.invalidated_events && !evicted.is_empty() {
                        output.push(self.event(
                            "invalidated",
                            obj(vec![("areas", array(vec![text("variables")]))]),
                        )?);
                    }
                    let seq = pending.seq;
                    let command = pending.command.clone();
                    let result = if let Some(cursor) = cursor {
                        self.release_cursor_owned(cursor, ReleaseNext::Inspect(request, pending))
                    } else {
                        self.submit_cleanup(DebugRequest::Inspect(request), *pending)
                    };
                    if let Err(error) = result {
                        output.push(self.response(
                            seq,
                            &command,
                            false,
                            Some(error.to_string()),
                            obj(vec![]),
                        )?);
                    }
                    return Ok(());
                }
            }
            ReleaseNext::Inspect(request, pending) => {
                if let Some(error) = release_error {
                    (pending, error)
                } else {
                    let seq = pending.seq;
                    let command = pending.command.clone();
                    if let Err(error) =
                        self.submit_cleanup(DebugRequest::Inspect(request), *pending)
                    {
                        output.push(self.response(
                            seq,
                            &command,
                            false,
                            Some(error.to_string()),
                            obj(vec![]),
                        )?);
                    }
                    return Ok(());
                }
            }
            ReleaseNext::Discard {
                pending,
                error,
                cursor,
            } => {
                if let Some(release_error) = release_error {
                    (pending, release_error)
                } else if let Some(cursor) = cursor {
                    self.inspection_checkpoints
                        .retain(|_, (_, known)| *known != cursor);
                    self.release_cursor_owned(cursor, ReleaseNext::Fail(pending, error))?;
                    return Ok(());
                } else {
                    (pending, error)
                }
            }
            ReleaseNext::Fail(pending, error) => (pending, release_error.unwrap_or(error)),
            ReleaseNext::EvaluationError {
                pending,
                error,
                cursor,
                mut remaining,
            } => {
                if let Some(release_error) = release_error {
                    // Keep unreleased records: an early evaluator failure may
                    // leave their native roots live. Never silently orphan them.
                    (pending, release_error)
                } else {
                    self.inspection_checkpoints
                        .retain(|_, (_, issued)| *issued != cursor);
                    if let Some(cursor) = remaining.pop() {
                        let seq = pending.seq;
                        let command = pending.command.clone();
                        let original_error = error.clone();
                        if self
                            .release_cursor(
                                cursor,
                                ReleaseNext::EvaluationError {
                                    pending,
                                    error,
                                    cursor,
                                    remaining,
                                },
                            )
                            .is_err()
                        {
                            output.push(self.response(
                                seq,
                                &command,
                                false,
                                Some(original_error.to_string()),
                                obj(vec![]),
                            )?);
                        }
                        return Ok(());
                    }
                    (pending, error.into())
                }
            }
        };
        output.push(self.response(
            pending.seq,
            &pending.command,
            false,
            Some(error.to_string()),
            obj(vec![]),
        )?);
        Ok(())
    }
    fn dispatch(&mut self, request: Request, output: &mut Vec<Vec<u8>>) -> Result<(), DapError> {
        if request.seq > i32::MAX as u64 {
            return Err(protocol(
                "DAP request sequence exceeds signed integer range",
            ));
        }
        if self.disconnected {
            return Err(DebugError::Closed.into());
        }
        if !self.initialized && request.command != "initialize" && request.command != "disconnect" {
            return Err(protocol("initialize must precede debugger commands"));
        }
        if self
            .configuration
            .is_some_and(|(seq, _)| seq == request.seq)
            || self
                .deferred_submission
                .as_ref()
                .is_some_and(|(_, p)| p.seq == request.seq)
            || self
                .pending
                .values()
                .any(|pending| pending.seq == request.seq)
        {
            return Err(protocol("duplicate in-flight DAP sequence"));
        }
        self.order = self.order.checked_add(1).ok_or(DapError::Capacity)?;
        let order = self.order;
        let pending = |kind, setup| Pending {
            seq: request.seq,
            command: request.command.clone(),
            order,
            setup,
            kind,
        };
        match request.command.as_str() {
            "jetbunny/targets"
            | "jetbunny/attachTarget"
            | "jetbunny/targetRequest"
            | "jetbunny/stopTarget" => self.target_request(&request, output)?,
            "initialize" => {
                let arguments: Initialize = args(&request)?;
                if self.initialized {
                    return Err(protocol("DAP already initialized"));
                }
                self.lines_one = arguments.lines_start_at1.unwrap_or(true);
                self.columns_one = arguments.columns_start_at1.unwrap_or(true);
                self.path_uri = match arguments.path_format.as_deref() {
                    None | Some("path") => false,
                    Some("uri") => true,
                    _ => return Err(protocol("unsupported DAP pathFormat")),
                };
                self.invalidated_events = arguments.supports_invalidated_event;
                self.initialized = true;
                let safe_lookup = self
                    .handle
                    .descriptor()
                    .capabilities
                    .contains(DebugCapability::SafeLookup);
                let capabilities = obj(vec![
                    ("supportsConfigurationDoneRequest", boolean(true)),
                    ("supportsConditionalBreakpoints", boolean(true)),
                    (
                        "supportsBreakpointLocationsRequest",
                        boolean(
                            self.handle
                                .descriptor()
                                .capabilities
                                .contains(DebugCapability::ColumnBreakpoints),
                        ),
                    ),
                    (
                        "supportsCancelRequest",
                        boolean(
                            self.handle
                                .descriptor()
                                .capabilities
                                .contains(DebugCapability::CancelEvaluation),
                        ),
                    ),
                    (
                        "supportsExceptionInfoRequest",
                        boolean(
                            self.handle
                                .descriptor()
                                .capabilities
                                .contains(DebugCapability::ExceptionDetails),
                        ),
                    ),
                    ("supportsEvaluateForHovers", boolean(safe_lookup)),
                    ("supportsStepBack", boolean(false)),
                    (
                        "supportsSetVariable",
                        boolean(
                            self.handle
                                .descriptor()
                                .capabilities
                                .contains(DebugCapability::VariableMutation),
                        ),
                    ),
                    (
                        "supportsHitConditionalBreakpoints",
                        boolean(
                            self.handle
                                .descriptor()
                                .capabilities
                                .contains(DebugCapability::BreakpointPolicies),
                        ),
                    ),
                    (
                        "supportsLogPoints",
                        boolean(
                            self.handle
                                .descriptor()
                                .capabilities
                                .contains(DebugCapability::Logpoints),
                        ),
                    ),
                    (
                        "jetbunny",
                        obj(vec![
                            ("supportsWorkerSessions", boolean(self.targets.is_some())),
                            (
                                "supportsAsyncStack",
                                boolean(
                                    self.handle
                                        .descriptor()
                                        .capabilities
                                        .contains(DebugCapability::AsyncStacks),
                                ),
                            ),
                            (
                                "supportsStepPolicy",
                                boolean(
                                    self.handle
                                        .descriptor()
                                        .capabilities
                                        .contains(DebugCapability::SkipSources),
                                ),
                            ),
                            (
                                "supportsSetBreakpointEnabled",
                                boolean(
                                    self.handle
                                        .descriptor()
                                        .capabilities
                                        .contains(DebugCapability::BreakpointPolicies),
                                ),
                            ),
                            (
                                "supportsContinueTo",
                                boolean(
                                    self.handle
                                        .descriptor()
                                        .capabilities
                                        .contains(DebugCapability::ContinueTo),
                                ),
                            ),
                        ]),
                    ),
                    ("supportsTerminateRequest", boolean(false)),
                    (
                        "exceptionBreakpointFilters",
                        array({
                            let mut filters = vec![
                                obj(vec![
                                    ("filter", text("all")),
                                    ("label", text("All Exceptions")),
                                    ("default", boolean(false)),
                                ]),
                                obj(vec![
                                    ("filter", text("uncaught")),
                                    ("label", text("Uncaught Exceptions")),
                                    ("default", boolean(false)),
                                ]),
                            ];
                            if self
                                .handle
                                .descriptor()
                                .capabilities
                                .contains(DebugCapability::UnhandledRejections)
                            {
                                filters.push(obj(vec![
                                    ("filter", text("unhandledRejection")),
                                    ("label", text("Unhandled Promise Rejections")),
                                    ("default", boolean(false)),
                                ]));
                            }
                            filters
                        }),
                    ),
                ]);
                let response =
                    self.response(request.seq, &request.command, true, None, capabilities)?;
                output.push(response);
                output.push(self.event("initialized", obj(vec![]))?);
            }
            "attach" => {
                let response =
                    self.response(request.seq, &request.command, true, None, obj(vec![]))?;
                output.push(response);
            }
            "configurationDone" => {
                if self.configuration.is_some() || self.configured {
                    return Err(protocol("configurationDone already received"));
                }
                self.configuration = Some((request.seq, order));
            }
            "threads" => {
                let response = self.response(
                    request.seq,
                    &request.command,
                    true,
                    None,
                    obj(vec![(
                        "threads",
                        array(vec![obj(vec![
                            ("id", number(1)),
                            ("name", text("JavaScript owner")),
                        ])]),
                    )]),
                )?;
                output.push(response);
            }
            "pause" => {
                let arguments: ThreadArgs = args(&request)?;
                check_thread(arguments.thread_id)?;
                check_control(self.handle.request_pause()?)?;
                let response =
                    self.response(request.seq, &request.command, true, None, obj(vec![]))?;
                output.push(response);
            }
            "continue" | "next" | "stepIn" | "stepOut" => {
                if !self.configured {
                    return Err(protocol("configurationDone barrier is still active"));
                }
                let arguments: ThreadArgs = args(&request)?;
                check_thread(arguments.thread_id)?;
                let action = match request.command.as_str() {
                    "next" => DebugContinue::StepOver,
                    "stepIn" => DebugContinue::StepInto,
                    "stepOut" => DebugContinue::StepOut,
                    _ => DebugContinue::Resume,
                };
                let pause = self.current_pause()?;
                let receipt = self.handle.continue_execution(pause, action)?;
                check_control(receipt)?;
                let response = self.response(
                    request.seq,
                    &request.command,
                    true,
                    None,
                    obj(vec![("allThreadsContinued", boolean(true))]),
                )?;
                output.push(response);
            }
            "cancel" => {
                let arguments: CancelArgs = args(&request)?;
                let id = arguments.request_id.and_then(|sequence| {
                    self.pending.iter().find_map(|(id, pending)| {
                        (pending.seq == sequence
                            && matches!(
                                pending.kind,
                                PendingKind::Evaluation(_) | PendingKind::Write(_)
                            ))
                        .then_some(*id)
                    })
                });
                if let Some(id) = id {
                    match self.handle.cancel_evaluation(id) {
                        Ok(receipt) => check_control(receipt)?,
                        // DAP cancellation is best effort. Unknown, queued and
                        // completed requests never target another evaluation.
                        Err(DebugError::InvalidRequest | DebugError::Closed) => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                output.push(self.response(
                    request.seq,
                    &request.command,
                    true,
                    None,
                    obj(vec![]),
                )?);
            }
            "disconnect" => {
                let arguments: DisconnectArgs = args(&request)?;
                if arguments.terminate_debuggee || arguments.suspend_debuggee || arguments.restart {
                    return Err(protocol(
                        "disconnect supports detach and resume only; termination, suspension and restart are unsupported",
                    ));
                }
                let detached = self.handle.detach();
                self.detach_target_sessions();
                check_control(detached?)?;
                let response =
                    self.response(request.seq, &request.command, true, None, obj(vec![]))?;
                output.push(response);
                self.disconnected = true;
            }
            "jetbunny/promiseRejections" => {
                let arguments: RejectionsArgs = args(&request)?;
                self.submit(
                    DebugRequest::PromiseRejections {
                        page: Page {
                            start: arguments.start,
                            count: arguments.count.unwrap_or(64),
                        },
                    },
                    pending(PendingKind::Rejections, false),
                )?;
            }
            "jetbunny/asyncStack" => {
                let arguments: ThreadArgs = args(&request)?;
                check_thread(arguments.thread_id)?;
                let pause = self.current_pause()?;
                self.submit(
                    DebugRequest::AsyncStack { pause },
                    pending(PendingKind::AsyncStack(pause), false),
                )?;
            }
            "exceptionInfo" => {
                let arguments: ThreadArgs = args(&request)?;
                check_thread(arguments.thread_id)?;
                let pause = self.current_pause()?;
                self.submit(
                    DebugRequest::ExceptionInfo { pause },
                    pending(PendingKind::ExceptionInfo(pause), false),
                )?;
            }
            "stackTrace" => {
                let arguments: ThreadArgs = args(&request)?;
                check_thread(arguments.thread_id)?;
                let pause = self.current_pause()?;
                let requested = arguments.levels.filter(|levels| *levels != 0);
                let count = requested.unwrap_or(64).min(64);
                arguments
                    .start_frame
                    .checked_add(count)
                    .ok_or(DapError::Capacity)?;
                self.submit(
                    DebugRequest::Stack {
                        pause,
                        page: Page {
                            start: arguments.start_frame,
                            count,
                        },
                    },
                    pending(
                        PendingKind::Stack {
                            pause,
                            start: arguments.start_frame,
                            requested,
                            frames: Vec::new(),
                        },
                        false,
                    ),
                )?;
            }
            "scopes" => {
                let arguments: FrameArgs = args(&request)?;
                let frame = self.frame(arguments.frame_id)?;
                self.submit(
                    DebugRequest::Scopes {
                        frame: self.frame(arguments.frame_id)?,
                    },
                    pending(PendingKind::Scopes(frame), false),
                )?;
            }
            "variables" => {
                if self.deferred_submission.is_some()
                    || self
                        .pending
                        .values()
                        .any(|p| p.inspection_reference().is_some())
                {
                    return Err(DebugError::InFlight.into());
                }
                let arguments: VariablesArgs = args(&request)?;
                let parent = self
                    .parents
                    .iter()
                    .find(|(id, _)| *id == arguments.variables_reference)
                    .map(|(_, parent)| *parent)
                    .ok_or_else(|| protocol("stale variablesReference"))?;
                let requested = arguments.count.filter(|count| *count != 0);
                if self
                    .handle
                    .descriptor()
                    .capabilities
                    .contains(DebugCapability::LazyInspection)
                {
                    let count = requested.unwrap_or(64.min(self.limits.inspection_page_items));
                    if count > self.limits.inspection_page_items {
                        return Err(DapError::Capacity);
                    }
                    arguments
                        .start
                        .checked_add(count)
                        .ok_or(DapError::Capacity)?;
                    if self.pending.values().any(|pending| {
                        pending.inspection_reference() == Some(arguments.variables_reference)
                    }) {
                        return Err(DebugError::InFlight.into());
                    }
                    let checkpoint = self
                        .inspection_checkpoints
                        .remove(&arguments.variables_reference);
                    let cursor = checkpoint
                        .filter(|(start, _)| *start == arguments.start)
                        .map(|(_, cursor)| cursor);
                    let inspection = InspectionRequest {
                        parent,
                        page: Page {
                            start: if cursor.is_some() { 0 } else { arguments.start },
                            count,
                        },
                        cursor,
                    };
                    let next = pending(
                        PendingKind::Inspection {
                            reference: arguments.variables_reference,
                            start: arguments.start,
                            count,
                            unpaged: requested.is_none(),
                            cursor,
                            retried: false,
                            reclaimed: false,
                            staged_targets: vec![],
                        },
                        false,
                    );
                    let result =
                        if let Some((_, abandoned)) = checkpoint.filter(|_| cursor.is_none()) {
                            self.release_cursor(
                                abandoned,
                                ReleaseNext::Inspect(inspection, Box::new(next)),
                            )
                        } else {
                            self.submit(DebugRequest::Inspect(inspection), next)
                        };
                    if result.is_err()
                        && let Some(checkpoint) = checkpoint
                    {
                        self.inspection_checkpoints
                            .insert(arguments.variables_reference, checkpoint);
                    }
                    result?;
                    return Ok(());
                }
                let count = requested.unwrap_or(64).min(64);
                arguments
                    .start
                    .checked_add(count)
                    .ok_or(DapError::Capacity)?;
                self.submit(
                    DebugRequest::Variables {
                        parent,
                        page: Page {
                            start: arguments.start,
                            count,
                        },
                    },
                    pending(
                        PendingKind::Variables {
                            parent,
                            start: arguments.start,
                            requested,
                            variables: Vec::new(),
                        },
                        false,
                    ),
                )?;
            }
            "setVariable" => {
                if self.deferred_submission.is_some()
                    || self
                        .pending
                        .values()
                        .any(|p| p.inspection_reference().is_some())
                {
                    return Err(DebugError::InFlight.into());
                }
                if !self
                    .handle
                    .descriptor()
                    .capabilities
                    .contains(DebugCapability::VariableMutation)
                {
                    return Err(DebugError::Unsupported.into());
                }
                let arguments: SetVariableArgs = args(&request)?;
                if self
                    .write_capacity_views
                    .get(&arguments.variables_reference)
                    .is_some_and(|view| {
                        view.iter()
                            .any(|(_, name)| name.as_units() == arguments.name.0.as_units())
                    })
                {
                    return Err(DapError::Capacity);
                }
                let target = self
                    .write_views
                    .get(&arguments.variables_reference)
                    .and_then(|view| {
                        view.iter()
                            .find(|(_, name, _)| name.as_units() == arguments.name.0.as_units())
                    })
                    .map(|(_, _, target)| *target)
                    .ok_or_else(|| {
                        protocol("member is read-only, unavailable, or not in the issued view")
                    })?;
                let frame = *self
                    .parent_frames
                    .get(&arguments.variables_reference)
                    .ok_or_else(|| protocol("selected frame unavailable for variables view"))?;
                self.submit(
                    DebugRequest::Write {
                        target,
                        frame,
                        expression: arguments.value.0,
                    },
                    pending(PendingKind::Write(frame), false),
                )?;
            }
            "evaluate" => {
                if self.deferred_submission.is_some()
                    || self
                        .pending
                        .values()
                        .any(|p| p.inspection_reference().is_some())
                {
                    return Err(DebugError::InFlight.into());
                }
                let arguments: EvaluateArgs = args(&request)?;
                let frame_id = arguments
                    .frame_id
                    .ok_or_else(|| protocol("selected frameId is required"))?;
                let frame = self.frame(frame_id)?;
                if matches!(arguments.context.as_deref(), Some("hover" | "watch")) {
                    // Automatic reevaluation never grants side-effect consent.
                    LookupPath::parse(&arguments.expression.0).map_err(|error| {
                        if error == DebugError::InvalidRequest {
                            protocol("Automatic hover/watch supports only binding names and literal property paths; evaluate other expressions explicitly in the REPL with side effects")
                        } else {
                            DapError::from(error)
                        }
                    })?;
                    self.submit(
                        DebugRequest::LookupPath {
                            frame,
                            path: arguments.expression.0,
                        },
                        pending(PendingKind::Lookup(frame), false),
                    )?;
                } else {
                    self.submit(
                        DebugRequest::Evaluate {
                            frame,
                            expression: arguments.expression.0,
                            allow_side_effects: true,
                        },
                        pending(PendingKind::Evaluation(frame), false),
                    )?;
                }
            }
            "setExceptionBreakpoints" => {
                let arguments: ExceptionArgs = args(&request)?;
                if arguments.filters.iter().any(|filter| {
                    filter != "all" && filter != "uncaught" && filter != "unhandledRejection"
                }) {
                    return Err(protocol("unsupported exception filter"));
                }
                let policy = if arguments.filters.iter().any(|filter| filter == "all") {
                    ExceptionPolicy::All
                } else if arguments.filters.iter().any(|filter| filter == "uncaught") {
                    ExceptionPolicy::Uncaught
                } else {
                    ExceptionPolicy::None
                };
                let supported = self
                    .handle
                    .descriptor()
                    .capabilities
                    .contains(DebugCapability::UnhandledRejections);
                let unhandled = arguments
                    .filters
                    .iter()
                    .any(|filter| filter == "unhandledRejection");
                if unhandled && !supported {
                    return Err(DebugError::Unsupported.into());
                }
                self.submit(
                    DebugRequest::ExceptionPolicy(policy),
                    pending(
                        if supported {
                            PendingKind::RejectionFilter(unhandled)
                        } else {
                            PendingKind::Simple
                        },
                        true,
                    ),
                )?;
            }
            "breakpointLocations" => {
                let arguments: LocationsArgs = args(&request)?;
                let line = from_client(arguments.line, self.lines_one)?;
                let start = SourcePosition {
                    line,
                    column: arguments
                        .column
                        .map(|v| from_client(v, self.columns_one))
                        .transpose()?
                        .unwrap_or(0),
                };
                let end = SourcePosition {
                    line: arguments
                        .end_line
                        .map(|v| from_client(v, self.lines_one))
                        .transpose()?
                        .unwrap_or(line),
                    column: arguments
                        .end_column
                        .map(|v| from_client(v, self.columns_one))
                        .transpose()?
                        .unwrap_or(u32::MAX),
                };
                let origin = self.source_origin(&arguments.source)?;
                self.submit(
                    DebugRequest::BreakpointLocations {
                        selector: ScriptSelector {
                            origin,
                            expected_digest: None,
                        },
                        start,
                        end,
                    },
                    pending(PendingKind::Locations, false),
                )?;
            }
            "setBreakpoints" => {
                let arguments: BreakpointsArgs = args(&request)?;
                let origin = self.source_origin(&arguments.source)?;
                let mut breakpoints = Vec::new();
                let mut metadata = Vec::new();
                for breakpoint in arguments.breakpoints {
                    let line = from_client(breakpoint.line, self.lines_one)?;
                    let column = breakpoint
                        .column
                        .map(|column| from_client(column, self.columns_one))
                        .transpose()?;
                    self.breakpoint_seq = self
                        .breakpoint_seq
                        .checked_add(1)
                        .filter(|id| *id <= i32::MAX as u64)
                        .ok_or(DapError::Capacity)?;
                    let id = self.breakpoint_seq;
                    metadata.push(BreakpointMeta {
                        id,
                        origin: origin.clone(),
                        line,
                        column,
                    });
                    breakpoints.push(BreakpointSpec {
                        client_id: id,
                        line,
                        column,
                        condition: breakpoint.condition.map(|condition| condition.0),
                        enabled: true,
                        hit_condition: breakpoint
                            .hit_condition
                            .as_deref()
                            .map(HitCondition::parse)
                            .transpose()?,
                        log_message: breakpoint.log_message.map(|message| message.0),
                    });
                }
                self.submit(
                    DebugRequest::SetBreakpoints {
                        selector: ScriptSelector {
                            origin: origin.clone(),
                            expected_digest: None,
                        },
                        breakpoints,
                    },
                    pending(PendingKind::Breakpoints { origin, metadata }, true),
                )?;
            }
            "jetbunny/setBreakpointEnabled" => {
                let arguments: EnableBreakpointArgs = args(&request)?;
                let origin = self.source_origin(&arguments.source)?;
                let meta = self
                    .breakpoints
                    .iter()
                    .find(|meta| meta.id == arguments.breakpoint_id && meta.origin == origin)
                    .cloned()
                    .ok_or_else(|| protocol("unknown source/breakpoint identity"))?;
                self.submit(
                    DebugRequest::SetBreakpointEnabled {
                        selector: ScriptSelector {
                            origin,
                            expected_digest: None,
                        },
                        client_id: meta.id,
                        enabled: arguments.enabled,
                    },
                    pending(
                        PendingKind::Enable {
                            meta,
                            enabled: arguments.enabled,
                        },
                        false,
                    ),
                )?;
            }
            "jetbunny/setStepPolicy" => {
                let arguments: StepPolicyArgs = args(&request)?;
                self.submit(
                    DebugRequest::StepPolicy(StepPolicy {
                        skip_sources: arguments.skip_sources,
                    }),
                    pending(PendingKind::Simple, true),
                )?;
            }
            "jetbunny/continueTo" => {
                if !self.configured {
                    return Err(protocol("configurationDone barrier is still active"));
                }
                let arguments: ContinueToArgs = args(&request)?;
                let reference = arguments.source.source_reference.ok_or_else(|| {
                    protocol("jetbunny/continueTo requires a generated sourceReference")
                })?;
                let entry = self
                    .sources
                    .iter()
                    .find(|entry| entry.reference == reference)
                    .ok_or_else(|| protocol("unknown sourceReference"))?;
                let SourceKey::Generated(script, revision) = entry.key else {
                    return Err(protocol(
                        "jetbunny/continueTo requires a generated sourceReference",
                    ));
                };
                self.submit(
                    DebugRequest::ContinueTo {
                        pause: self.current_pause()?,
                        location: DebugLocation {
                            script,
                            revision,
                            line: from_client(arguments.line, self.lines_one)?,
                            column: Some(from_client(arguments.column, self.columns_one)?),
                        },
                    },
                    pending(PendingKind::Simple, false),
                )?;
            }
            "loadedSources" => {
                self.submit(
                    DebugRequest::Sources {
                        page: Page {
                            start: 0,
                            count: 32,
                        },
                    },
                    pending(
                        PendingKind::Sources {
                            scripts: Vec::new(),
                        },
                        false,
                    ),
                )?;
            }
            "source" => {
                let arguments: SourceArgs = args(&request)?;
                let reference = if arguments.source_reference != 0 {
                    arguments.source_reference
                } else {
                    arguments
                        .source
                        .as_ref()
                        .and_then(|source| source.source_reference)
                        .unwrap_or(0)
                };
                let query = if reference != 0 {
                    match &self
                        .sources
                        .iter()
                        .find(|source| source.reference == reference)
                        .ok_or_else(|| protocol("unknown sourceReference"))?
                        .key
                    {
                        SourceKey::Generated(id, revision) => {
                            SourceQuery::Generated(*id, *revision)
                        }
                        SourceKey::Original(origin, digest) => {
                            SourceQuery::Original(ScriptSelector {
                                origin: origin.clone(),
                                expected_digest: *digest,
                            })
                        }
                    }
                } else {
                    let source = arguments
                        .source
                        .ok_or_else(|| protocol("sourceReference or source path is required"))?;
                    SourceQuery::Original(ScriptSelector {
                        origin: self.source_origin(&source)?,
                        expected_digest: None,
                    })
                };
                self.submit(
                    query.request(0, self.page_count()?),
                    pending(
                        PendingKind::Source {
                            query,
                            units: Vec::new(),
                            total: None,
                        },
                        false,
                    ),
                )?;
            }
            _ => return Err(DebugError::Unsupported.into()),
        }
        Ok(())
    }
    fn finish_configuration(&mut self, output: &mut Vec<Vec<u8>>) -> Result<(), DapError> {
        if let Some((seq, cutoff)) = self.configuration
            && !self
                .pending
                .values()
                .any(|pending| pending.setup && pending.order <= cutoff)
        {
            self.handle.session.release_entry_gate()?;
            self.configuration = None;
            self.configured = true;
            output.push(self.response(seq, "configurationDone", true, None, obj(vec![]))?);
        }
        if self.configured
            && let Some(pause) = self.entry.take()
            && self.handle.state() == DebugExecutionState::Paused(pause)
        {
            check_control(
                self.handle
                    .continue_execution(pause, DebugContinue::Resume)?,
            )?;
        }
        Ok(())
    }
    fn reply(
        &mut self,
        pending: Pending,
        reply: DebugReply,
        output: &mut Vec<Vec<u8>>,
    ) -> Result<(), DapError> {
        if !matches!(reply, DebugReply::Inspection(_)) {
            return self.reply_inner(pending, reply, output);
        }
        let page = if let DebugReply::Inspection(page) = &reply {
            Some(page)
        } else {
            None
        };
        let mut targets: Vec<_> = page
            .map(|page| page.items.iter().filter_map(|entry| entry.write).collect())
            .unwrap_or_default();
        if let PendingKind::Inspection { staged_targets, .. } = &pending.kind {
            for target in staged_targets {
                if !targets.contains(target) {
                    targets.push(*target);
                }
            }
        }
        let cursor = page.and_then(|page| page.next);
        let cleanup = Pending {
            seq: pending.seq,
            command: pending.command.clone(),
            order: pending.order,
            setup: pending.setup,
            kind: PendingKind::Cleanup {
                reference: pending.inspection_reference(),
            },
        };
        let inspection = page.is_some();
        match self.reply_inner(pending, reply, output) {
            Ok(()) => Ok(()),
            Err(error) if inspection => {
                targets.retain(|target| {
                    !self
                        .write_views
                        .values()
                        .any(|view| view.iter().any(|(_, _, t)| t == target))
                });
                if !targets.is_empty() {
                    self.release_targets(
                        targets,
                        ReleaseNext::Discard {
                            pending: Box::new(cleanup),
                            error,
                            cursor,
                        },
                    )
                } else if let Some(cursor) = cursor {
                    self.inspection_checkpoints
                        .retain(|_, (_, known)| *known != cursor);
                    self.release_cursor_owned(cursor, ReleaseNext::Fail(Box::new(cleanup), error))
                } else {
                    output.push(self.response(
                        cleanup.seq,
                        &cleanup.command,
                        false,
                        Some(error.to_string()),
                        obj(vec![]),
                    )?);
                    Ok(())
                }
            }
            Err(error) => Err(error),
        }
    }
    fn reply_inner(
        &mut self,
        mut pending: Pending,
        reply: DebugReply,
        output: &mut Vec<Vec<u8>>,
    ) -> Result<(), DapError> {
        let mut staged_view = None;
        let mut staged_checkpoint = None;
        let mut staged_pause = None;
        let body = match (&mut pending.kind, reply) {
            (PendingKind::RejectionFilter(enabled), DebugReply::ExceptionPolicy(policy)) => {
                self.exception_policy = policy;
                let enabled = *enabled;
                pending.kind = PendingKind::Simple;
                self.submit(DebugRequest::UnhandledRejections(enabled), pending)?;
                return Ok(());
            }
            (PendingKind::Simple, DebugReply::UnhandledRejections) => obj(vec![]),
            (
                PendingKind::Rejections,
                DebugReply::PromiseRejections {
                    records,
                    total,
                    dropped,
                    retained_bytes,
                },
            ) => {
                let records = records
                    .into_iter()
                    .map(|info| self.rejection_value(info))
                    .collect::<Result<Vec<_>, _>>()?;
                obj(vec![
                    ("records", array(records)),
                    ("total", number(total as u64)),
                    ("dropped", number(dropped)),
                    ("retainedBytes", number(retained_bytes as u64)),
                ])
            }
            (PendingKind::Simple, DebugReply::ExceptionPolicy(policy)) => {
                self.exception_policy = policy;
                obj(vec![])
            }
            (PendingKind::Simple, DebugReply::ContinueTo | DebugReply::StepPolicy) => obj(vec![]),
            (PendingKind::Enable { meta, enabled }, DebugReply::Breakpoints(values)) => {
                if values.len() != 1 || values[0].client_id != meta.id {
                    return Err(protocol("unexpected enablement reply"));
                }
                obj(vec![
                    ("breakpoint", self.breakpoint_value(meta, &values[0].state)?),
                    ("enabled", boolean(*enabled)),
                ])
            }
            (PendingKind::AsyncStack(pause), DebugReply::AsyncStack(trace)) => {
                if self.handle.state() != DebugExecutionState::Paused(*pause) {
                    return Err(DebugError::StalePause.into());
                }
                staged_pause = Some(*pause);
                let mut entries = Vec::new();
                for entry in trace.entries {
                    let mut frames = Vec::new();
                    for (index, location) in entry.record.locations.iter().enumerate() {
                        let original = entry.originals.get(index).and_then(Option::as_ref);
                        let (source, line, column) = if let Some(original) = original {
                            (
                                self.source_original(original)?,
                                original.line,
                                original.column,
                            )
                        } else {
                            (
                                self.source_generated(location.script, location.revision, None)?,
                                location.line,
                                location.column.unwrap_or(0),
                            )
                        };
                        frames.push(obj(vec![
                            ("source", source),
                            ("line", number(to_client(line, self.lines_one)?)),
                            ("column", number(to_client(column, self.columns_one)?)),
                            ("readOnly", boolean(true)),
                            ("presentationHint", text("label")),
                        ]));
                    }
                    // String IDs inhabit a different namespace from live DAP frame handles.
                    let mut fields = vec![
                        ("id", text(&format!("history:{}", entry.id.serial()))),
                        ("label", Value::Text(entry.record.label)),
                        ("truncated", boolean(entry.record.truncated)),
                        ("readOnly", boolean(true)),
                        ("frames", array(frames)),
                    ];
                    if let Some(parent) = entry.record.parent {
                        fields.push(("parent", text(&format!("history:{}", parent.serial()))));
                    }
                    entries.push(obj(fields));
                }
                obj(vec![
                    ("entries", array(entries)),
                    ("truncated", boolean(trace.truncated)),
                    ("retainedRecords", number(trace.retained_records as u64)),
                    ("retainedBytes", number(trace.retained_bytes as u64)),
                ])
            }
            (PendingKind::ExceptionInfo(pause), DebugReply::ExceptionInfo(info)) => {
                if self.handle.state() != DebugExecutionState::Paused(*pause) {
                    return Err(DebugError::StalePause.into());
                }
                staged_pause = Some(*pause);
                let mut trace = Vec::new();
                for frame in info.stack {
                    let mut suffix = String::new();
                    if let Some(original) = &frame.original {
                        suffix = format!(
                            " ({}:{}",
                            original.origin,
                            to_client(original.line, self.lines_one)?
                        );
                        suffix.push_str(&format!(
                            ":{}",
                            to_client(original.column, self.columns_one)?
                        ));
                        suffix.push(')');
                    } else if let Some(location) = &frame.generated {
                        let origin = self
                            .sources
                            .iter()
                            .find(|source| {
                                source.key
                                    == SourceKey::Generated(location.script, location.revision)
                            })
                            .and_then(|source| source.origin.as_deref())
                            .unwrap_or("<source unavailable>");
                        suffix =
                            format!(" ({}:{}", origin, to_client(location.line, self.lines_one)?);
                        if let Some(column) = location.column {
                            suffix.push_str(&format!(":{}", to_client(column, self.columns_one)?));
                        }
                        suffix.push(')');
                    }
                    let suffix: Vec<u16> = suffix.encode_utf16().collect();
                    let added = 4usize
                        .checked_add(frame.name.as_units().len())
                        .and_then(|n| n.checked_add(suffix.len()))
                        .ok_or(DapError::Capacity)?;
                    let end = trace.len().checked_add(added).ok_or(DapError::Capacity)?;
                    if end > self.limits.dap_output_bytes / 2 {
                        return Err(DapError::Capacity);
                    }
                    trace
                        .try_reserve_exact(added)
                        .map_err(|_| DapError::Capacity)?;
                    trace.extend("at ".encode_utf16());
                    trace.extend_from_slice(frame.name.as_units());
                    trace.extend_from_slice(&suffix);
                    trace.push(10);
                }
                obj(vec![
                    ("exceptionId", text("JavaScript throw")),
                    ("description", Value::Text(info.description.clone())),
                    ("breakMode", text(self.exception_break_mode)),
                    (
                        "details",
                        obj(vec![
                            ("message", Value::Text(info.description)),
                            ("stackTrace", Value::Text(JsString::from_units(trace))),
                        ]),
                    ),
                ])
            }
            (PendingKind::Locations, DebugReply::Locations(locations)) => {
                let mut positions = Vec::new();
                for location in locations {
                    let mut fields =
                        vec![("line", number(to_client(location.line, self.lines_one)?))];
                    if let Some(column) = location.column {
                        fields.push(("column", number(to_client(column, self.columns_one)?)));
                    }
                    positions.push(obj(fields));
                }
                obj(vec![("breakpoints", array(positions))])
            }
            (PendingKind::Locations, DebugReply::OriginalLocations(locations)) => {
                let positions = locations
                    .into_iter()
                    .map(|location| {
                        Ok(obj(vec![
                            ("line", number(to_client(location.line, self.lines_one)?)),
                            (
                                "column",
                                number(to_client(location.column, self.columns_one)?),
                            ),
                        ]))
                    })
                    .collect::<Result<Vec<_>, DapError>>()?;
                obj(vec![("breakpoints", array(positions))])
            }
            (
                PendingKind::Stack {
                    pause,
                    start,
                    requested,
                    frames,
                },
                DebugReply::Stack {
                    frames: page,
                    total,
                },
            ) => {
                let available = total
                    .checked_sub(*start)
                    .ok_or_else(|| protocol("invalid frame total"))?;
                let wanted = requested.map_or(available, |count| count.min(available));
                let next = frames
                    .len()
                    .checked_add(page.len())
                    .ok_or(DapError::Capacity)?;
                if next > wanted || (page.is_empty() && next < wanted) {
                    return Err(protocol("invalid frame page"));
                }
                frames.extend(page);
                let mut retained = frames
                    .capacity()
                    .checked_mul(size_of::<DebugFrame>())
                    .ok_or(DapError::Capacity)?;
                for frame in frames.iter() {
                    retained = retained
                        .checked_add(frame.name.capacity_bytes())
                        .and_then(|bytes| {
                            bytes.checked_add(
                                frame
                                    .original
                                    .as_ref()
                                    .map_or(0, |location| location.origin.capacity()),
                            )
                        })
                        .ok_or(DapError::Capacity)?;
                }
                if retained > self.limits.dap_output_bytes {
                    return Err(DapError::Capacity);
                }
                if next < wanted {
                    let request = DebugRequest::Stack {
                        pause: *pause,
                        page: Page {
                            start: start.checked_add(next).ok_or(DapError::Capacity)?,
                            count: (wanted - next).min(64),
                        },
                    };
                    self.submit(request, pending)?;
                    return Ok(());
                }
                let frames = std::mem::take(frames);
                let mut result = Vec::new();
                for frame in frames {
                    let id = self.frame_reference(frame.id)?;
                    let mut item = vec![("id", number(id)), ("name", Value::Text(frame.name))];
                    if let Some(original) = frame.original {
                        let source = self.source_original(&original)?;
                        item.extend([
                            ("source", source),
                            ("line", number(to_client(original.line, self.lines_one)?)),
                            (
                                "column",
                                number(to_client(original.column, self.columns_one)?),
                            ),
                        ]);
                    } else if let Some(generated) = frame.generated {
                        let source =
                            self.source_generated(generated.script, generated.revision, None)?;
                        item.extend([
                            ("source", source),
                            ("line", number(to_client(generated.line, self.lines_one)?)),
                            (
                                "column",
                                number(
                                    generated
                                        .column
                                        .map(|column| to_client(column, self.columns_one))
                                        .transpose()?
                                        .unwrap_or(0),
                                ),
                            ),
                        ]);
                    } else {
                        item.extend([("line", number(0)), ("column", number(0))]);
                    }
                    result.push(obj(item));
                }
                obj(vec![
                    ("stackFrames", array(result)),
                    ("totalFrames", number(total as u64)),
                ])
            }
            (PendingKind::Scopes(frame), DebugReply::Scopes(scopes)) => {
                let mut result = Vec::new();
                for scope in scopes {
                    let reference = self
                        .parent_reference_in_frame(VariableParent::Scope(scope.id), Some(*frame))?;
                    self.parent_frames.insert(reference, *frame);
                    result.push(obj(vec![
                        ("name", Value::Text(scope.name)),
                        ("variablesReference", number(reference)),
                        ("expensive", boolean(scope.expensive)),
                    ]));
                }
                obj(vec![("scopes", array(result))])
            }
            (
                PendingKind::Variables {
                    parent,
                    start,
                    requested,
                    variables,
                },
                DebugReply::Variables {
                    variables: page,
                    total,
                },
            ) => {
                let available = total
                    .checked_sub(*start)
                    .ok_or_else(|| protocol("invalid variable total"))?;
                let wanted = requested.map_or(available, |count| count.min(available));
                let next = variables
                    .len()
                    .checked_add(page.len())
                    .ok_or(DapError::Capacity)?;
                if next > wanted || (page.is_empty() && next < wanted) {
                    return Err(protocol("invalid variable page"));
                }
                variables.extend(page);
                let mut retained = variables
                    .capacity()
                    .checked_mul(size_of::<DebugVariable>())
                    .ok_or(DapError::Capacity)?;
                for variable in variables.iter() {
                    retained = retained
                        .checked_add(variable.name.capacity_bytes())
                        .and_then(|bytes| {
                            bytes.checked_add(variable.value.preview.capacity_bytes())
                        })
                        .and_then(|bytes| {
                            bytes.checked_add(
                                variable
                                    .value
                                    .unavailable
                                    .as_ref()
                                    .map_or(0, String::capacity),
                            )
                        })
                        .ok_or(DapError::Capacity)?;
                }
                if retained > self.limits.dap_output_bytes {
                    return Err(DapError::Capacity);
                }
                if next < wanted {
                    let request = DebugRequest::Variables {
                        parent: *parent,
                        page: Page {
                            start: start.checked_add(next).ok_or(DapError::Capacity)?,
                            count: (wanted - next).min(64),
                        },
                    };
                    self.submit(request, pending)?;
                    return Ok(());
                }
                let variables = std::mem::take(variables);
                let mut result = Vec::new();
                for variable in variables {
                    let reference = variable
                        .value
                        .object
                        .map(|object| self.parent_reference(VariableParent::Object(object)))
                        .transpose()?
                        .unwrap_or(0);
                    result.push(obj(vec![
                        ("name", Value::Text(variable.name)),
                        ("value", Value::Text(variable.value.preview)),
                        ("variablesReference", number(reference)),
                    ]));
                }
                obj(vec![("variables", array(result))])
            }
            (
                PendingKind::Inspection {
                    reference,
                    start,
                    count,
                    unpaged,
                    retried,
                    reclaimed,
                    staged_targets,
                    ..
                },
                DebugReply::Inspection(page),
            ) => {
                let original_parent = self
                    .parents
                    .iter()
                    .find(|(id, _)| id == reference)
                    .map(|(_, parent)| *parent)
                    .ok_or(DebugError::StalePause)?;
                let original_pause = match original_parent {
                    VariableParent::Scope(scope) => scope.pause,
                    VariableParent::Object(object) => object.pause,
                };
                if self.handle.state() != DebugExecutionState::Paused(original_pause) {
                    return Err(DebugError::StalePause.into());
                }
                staged_pause = Some(original_pause);
                if page.items.len() > *count
                    || (!page.complete && (*unpaged || page.items.len() < *count))
                {
                    return Err(DapError::Capacity);
                }
                if !*reclaimed {
                    // Serialize issuance/release transactions; staged page refs
                    // protect aliases until the old/new view difference is known.
                    let page_targets: Vec<_> = page.items.iter().filter_map(|e| e.write).collect();
                    let capacity = page.items.iter().any(|e| e.write_unavailable.is_some());
                    if capacity && !*retried && page.items.len() <= self.limits.write_targets {
                        let missing = page
                            .items
                            .iter()
                            .filter(|e| e.write_unavailable.is_some())
                            .count();
                        let mut release = Vec::new();
                        for parent in &self.view_order {
                            if let Some(view) = self.write_views.get(parent) {
                                for (_, _, target) in view {
                                    if !page_targets.contains(target) && !release.contains(target) {
                                        release.push(*target);
                                    }
                                    if release.len() >= missing {
                                        break;
                                    }
                                }
                            }
                            if release.len() >= missing {
                                break;
                            }
                        }
                        if !release.is_empty() {
                            *retried = true;
                            *staged_targets = page_targets.clone();
                            let parent = self
                                .parents
                                .iter()
                                .find(|(id, _)| id == reference)
                                .map(|(_, parent)| *parent)
                                .ok_or_else(|| protocol("stale variables view"))?;
                            let request = InspectionRequest {
                                parent,
                                page: Page {
                                    start: *start,
                                    count: *count,
                                },
                                cursor: None,
                            };
                            let cursor = page.next;
                            let evicted = release.clone();
                            self.release_targets(
                                release,
                                ReleaseNext::Retry {
                                    pending: Box::new(pending),
                                    request,
                                    cursor,
                                    evicted,
                                },
                            )?;
                            return Ok(());
                        }
                    }
                    let mut release = Vec::new();
                    if let Some(old) = self.write_views.get(reference) {
                        let end = start.saturating_add(page.items.len());
                        for (at, _, target) in old {
                            if *at >= *start
                                && *at < end
                                && !old.iter().any(|(other, _, t)| {
                                    t == target && (*other < *start || *other >= end)
                                })
                                && !page_targets.contains(target)
                                && !self.write_views.iter().any(|(id, view)| {
                                    id != reference && view.iter().any(|(_, _, t)| t == target)
                                })
                                && !release.contains(target)
                            {
                                release.push(*target);
                            }
                        }
                    }
                    if !release.is_empty() {
                        *reclaimed = true;
                        self.release_targets(
                            release.clone(),
                            ReleaseNext::Page {
                                pending: Box::new(pending),
                                page,
                                released: release.clone(),
                            },
                        )?;
                        return Ok(());
                    }
                }
                if let Some(cursor) = page.next {
                    let next = start
                        .checked_add(page.items.len())
                        .ok_or(DapError::Capacity)?;
                    staged_checkpoint = Some((*reference, next, cursor));
                }
                let selected_frame = self.parent_frames.get(reference).copied();
                let end = start.saturating_add(page.items.len());
                let mut view = self.write_views.get(reference).cloned().unwrap_or_default();
                view.retain(|(at, _, _)| *at < *start || *at >= end);
                let mut capacity_view = self
                    .write_capacity_views
                    .get(reference)
                    .cloned()
                    .unwrap_or_default();
                capacity_view.retain(|(at, _)| *at < *start || *at >= end);
                let mut result = Vec::new();
                for (offset, entry) in page.items.into_iter().enumerate() {
                    if let Some(target) = entry.write {
                        view.push((*start + offset, entry.variable.name.clone(), target));
                    }
                    if entry.write_unavailable.is_some() {
                        capacity_view.push((*start + offset, entry.variable.name.clone()));
                    }
                    let variable = entry.variable;
                    let reference = variable
                        .value
                        .object
                        .map(|object| {
                            self.parent_reference_in_frame(
                                VariableParent::Object(object),
                                selected_frame,
                            )
                        })
                        .transpose()?
                        .unwrap_or(0);
                    if let Some(frame) = selected_frame
                        && reference != 0
                    {
                        self.parent_frames.insert(reference, frame);
                    }
                    result.push(obj(vec![
                        ("name", Value::Text(variable.name)),
                        ("value", Value::Text(variable.value.preview)),
                        ("type", Value::Text(entry.type_name)),
                        ("variablesReference", number(reference)),
                    ]));
                }
                if self.bound_capacity_views(*reference, *start..end, &view, &mut capacity_view)?
                    && self.invalidated_events
                {
                    output.push(self.event(
                        "invalidated",
                        obj(vec![("areas", array(vec![text("variables")]))]),
                    )?);
                }
                staged_view = Some((*reference, view, capacity_view));
                obj(vec![("variables", array(result))])
            }
            (PendingKind::Lookup(frame), DebugReply::Lookup(value)) => {
                if self.handle.state() != DebugExecutionState::Paused(frame.pause) {
                    return Err(DebugError::StalePause.into());
                }
                staged_pause = Some(frame.pause);
                if let Some(reason) = value.unavailable {
                    return Err(protocol(&reason));
                }
                let reference = value
                    .object
                    .map(|object| {
                        self.parent_reference_in_frame(VariableParent::Object(object), Some(*frame))
                    })
                    .transpose()?
                    .unwrap_or(0);
                obj(vec![
                    ("result", Value::Text(value.preview)),
                    ("variablesReference", number(reference)),
                ])
            }
            (PendingKind::Evaluation(frame), DebugReply::Evaluation { value, exception }) => {
                self.write_views.clear();
                self.write_capacity_views.clear();
                self.inspection_checkpoints.clear();
                if exception {
                    let response = self.response(
                        pending.seq,
                        &pending.command,
                        false,
                        None,
                        obj(vec![("result", Value::Text(value.preview))]),
                    )?;
                    output.push(response);
                    return Ok(());
                }
                let reference = value
                    .object
                    .map(|object| {
                        self.parent_reference_in_frame(VariableParent::Object(object), Some(*frame))
                    })
                    .transpose()?
                    .unwrap_or(0);
                if reference != 0 {
                    self.parent_frames.insert(reference, *frame);
                }
                obj(vec![
                    ("result", Value::Text(value.preview)),
                    ("variablesReference", number(reference)),
                ])
            }
            (PendingKind::Write(frame), DebugReply::Write(value)) => {
                self.inspection_checkpoints.clear();
                self.write_views.clear();
                self.write_capacity_views.clear();
                let reference = value
                    .object
                    .map(|object| {
                        self.parent_reference_in_frame(VariableParent::Object(object), Some(*frame))
                    })
                    .transpose()?
                    .unwrap_or(0);
                if reference != 0 {
                    self.parent_frames.insert(reference, *frame);
                }
                if self.invalidated_events {
                    output.push(self.event(
                        "invalidated",
                        obj(vec![("areas", array(vec![text("variables")]))]),
                    )?);
                }
                obj(vec![
                    ("value", Value::Text(value.preview)),
                    ("variablesReference", number(reference)),
                ])
            }
            (PendingKind::Breakpoints { origin, metadata }, DebugReply::Breakpoints(values)) => {
                let mut result = Vec::new();
                for value in values {
                    let meta = metadata
                        .iter()
                        .find(|meta| meta.id == value.client_id)
                        .ok_or_else(|| protocol("unknown native breakpoint identity"))?;
                    result.push(self.breakpoint_value(meta, &value.state)?);
                }
                self.breakpoints.retain(|old| old.origin != *origin);
                self.breakpoints.extend(metadata.iter().cloned());
                obj(vec![("breakpoints", array(result))])
            }
            (
                PendingKind::Sources { scripts },
                DebugReply::Sources {
                    scripts: page,
                    total,
                },
            ) => {
                let next = scripts
                    .len()
                    .checked_add(page.len())
                    .ok_or(DapError::Capacity)?;
                if next > self.limits.scripts || next > total {
                    return Err(DapError::Capacity);
                }
                if page.is_empty() && next < total {
                    return Err(protocol("empty nonterminal source page"));
                }
                scripts.extend(page);
                if next < total {
                    self.submit(
                        DebugRequest::Sources {
                            page: Page {
                                start: next,
                                count: 32,
                            },
                        },
                        pending,
                    )?;
                    return Ok(());
                }
                let scripts = std::mem::take(scripts);
                let mut result = Vec::new();
                for script in scripts {
                    result.push(self.source_info(&script)?);
                }
                obj(vec![("sources", array(result))])
            }
            (
                PendingKind::Source {
                    query,
                    units,
                    total,
                },
                DebugReply::Source { text, total_units },
            ) => {
                if total_units > self.limits.source_bytes / 2
                    || total.is_some_and(|old| old != total_units)
                {
                    return Err(DapError::Capacity);
                }
                if total.is_none() {
                    units
                        .try_reserve_exact(total_units)
                        .map_err(|_| DapError::Capacity)?;
                    *total = Some(total_units);
                }
                let next = units
                    .len()
                    .checked_add(text.as_units().len())
                    .ok_or(DapError::Capacity)?;
                if next > total_units || (text.as_units().is_empty() && next < total_units) {
                    return Err(protocol("invalid source page"));
                }
                units.extend_from_slice(text.as_units());
                if next < total_units {
                    let request = query.request(next, self.page_count()?);
                    self.submit(request, pending)?;
                    return Ok(());
                }
                obj(vec![
                    (
                        "content",
                        Value::Text(JsString::from_units(std::mem::take(units))),
                    ),
                    (
                        "mimeType",
                        Value::Text(JsString::from_utf8("text/javascript")),
                    ),
                ])
            }
            _ => {
                let response = self.response(
                    pending.seq,
                    &pending.command,
                    false,
                    Some("unexpected typed debugger reply".to_owned()),
                    obj(vec![]),
                )?;
                output.push(response);
                return Ok(());
            }
        };
        match self.response(pending.seq, &pending.command, true, None, body) {
            Ok(frame) => {
                if staged_pause
                    .is_some_and(|pause| self.handle.state() != DebugExecutionState::Paused(pause))
                {
                    // Discard any parent references materialized during serialization
                    // if the owner resumed while this response was being encoded.
                    self.synchronize_pause();
                    return Err(DebugError::StalePause.into());
                }
                if let Some((reference, view, capacity_view)) = staged_view {
                    self.write_views.insert(reference, view);
                    self.write_capacity_views.insert(reference, capacity_view);
                    self.view_order.retain(|id| *id != reference);
                    self.view_order.push(reference);
                }
                if let Some((reference, next, cursor)) = staged_checkpoint {
                    self.inspection_checkpoints
                        .insert(reference, (next, cursor));
                }
                output.push(frame);
            }
            Err(error) => {
                if staged_view.is_some() {
                    return Err(error);
                }
                let response =
                    self.output_failure_response(pending.seq, &pending.command, error)?;
                output.push(response);
            }
        }
        Ok(())
    }
    fn output_failure_response(
        &mut self,
        seq: u64,
        command: &str,
        error: DapError,
    ) -> Result<Vec<u8>, DapError> {
        match error {
            DapError::Capacity => self.response(
                seq,
                command,
                false,
                Some("DAP output message exceeds configured allowance".to_owned()),
                obj(vec![]),
            ),
            error => Err(error),
        }
    }
    fn source_origin(&self, source: &WireSource) -> Result<String, DapError> {
        if let Some(path) = &source.path {
            return canonical_uri(path);
        }
        let reference = source
            .source_reference
            .ok_or_else(|| protocol("source path is required"))?;
        self.sources
            .iter()
            .find(|entry| entry.reference == reference)
            .and_then(|entry| entry.origin.clone())
            .ok_or_else(|| protocol("source has no canonical path"))
    }
    fn rejection_value(&mut self, info: PromiseRejectionInfo) -> Result<Value, DapError> {
        let mut fields = vec![
            ("id", text(&info.id.wire_id())),
            (
                "state",
                text(match info.state {
                    PromiseRejectionState::Pending => "pending",
                    PromiseRejectionState::Unhandled => "unhandled",
                    PromiseRejectionState::HandledLater => "handledLater",
                }),
            ),
            ("description", Value::Text(info.description)),
            ("readOnly", boolean(true)),
        ];
        if let Some(original) = info.original {
            fields.extend([
                ("source", self.source_original(&original)?),
                ("line", number(to_client(original.line, self.lines_one)?)),
                (
                    "column",
                    number(to_client(original.column, self.columns_one)?),
                ),
            ]);
        } else if let Some(location) = info.location {
            fields.extend([
                (
                    "source",
                    self.source_generated(location.script, location.revision, None)?,
                ),
                ("line", number(to_client(location.line, self.lines_one)?)),
                (
                    "column",
                    number(
                        location
                            .column
                            .map(|c| to_client(c, self.columns_one))
                            .transpose()?
                            .unwrap_or(0),
                    ),
                ),
            ]);
        }
        Ok(obj(fields))
    }
    fn source_info(&mut self, info: &ScriptInfo) -> Result<Value, DapError> {
        self.source_generated(
            info.registration.id,
            info.registration.revision,
            Some(info.origin.clone()),
        )
    }
    fn source_generated(
        &mut self,
        id: ScriptId,
        revision: u64,
        origin: Option<String>,
    ) -> Result<Value, DapError> {
        let key = SourceKey::Generated(id, revision);
        let index = if let Some(index) = self.sources.iter().position(|entry| entry.key == key) {
            if origin.is_some() {
                self.sources[index].origin = origin;
            }
            index
        } else {
            let reference = self.next_reference()?;
            self.sources.push(SourceEntry {
                reference,
                key,
                origin,
            });
            self.sources.len() - 1
        };
        Ok(self.source_value(&self.sources[index]))
    }
    fn source_original(&mut self, location: &OriginalLocation) -> Result<Value, DapError> {
        if location.digest.is_none() {
            return Ok(self.path_source(&location.origin, 0));
        }
        let key = SourceKey::Original(location.origin.clone(), location.digest);
        let index = if let Some(index) = self.sources.iter().position(|entry| entry.key == key) {
            index
        } else {
            let reference = self.next_reference()?;
            self.sources.push(SourceEntry {
                reference,
                key,
                origin: Some(location.origin.clone()),
            });
            self.sources.len() - 1
        };
        Ok(self.source_value(&self.sources[index]))
    }
    fn source_value(&self, source: &SourceEntry) -> Value {
        source.origin.as_ref().map_or_else(
            || {
                obj(vec![
                    ("name", text("Generated source")),
                    ("sourceReference", number(source.reference)),
                ])
            },
            |origin| self.path_source(origin, source.reference),
        )
    }
    fn path_source(&self, origin: &str, reference: u64) -> Value {
        let path = if self.path_uri {
            origin.to_owned()
        } else {
            url::Url::parse(origin)
                .ok()
                .filter(|url| url.scheme() == "file")
                .and_then(|url| url.to_file_path().ok())
                .and_then(|path| path.to_str().map(str::to_owned))
                .unwrap_or_else(|| origin.to_owned())
        };
        obj(vec![
            ("name", text(origin.rsplit('/').next().unwrap_or(origin))),
            ("path", text(&path)),
            ("sourceReference", number(reference)),
        ])
    }
    fn breakpoint_value(
        &mut self,
        meta: &BreakpointMeta,
        state: &BreakpointState,
    ) -> Result<Value, DapError> {
        let actual = match state {
            BreakpointState::Verified(locations) => locations.first(),
            _ => None,
        };
        // A mapped request can bind generated code elsewhere. Report its actual
        // source identity along with actual coordinates instead of labeling them
        // with the requested original URI.
        let source = match actual {
            Some(location) => self.source_generated(location.script, location.revision, None)?,
            None => self.path_source(&meta.origin, 0),
        };
        let line = actual.map_or(meta.line, |location| location.line);
        let column = actual.map_or(meta.column, |location| location.column);
        let mut values = vec![
            ("id", number(meta.id)),
            (
                "verified",
                boolean(
                    matches!(state,BreakpointState::Verified(locations) if !locations.is_empty()),
                ),
            ),
            ("source", source),
            ("line", number(to_client(line, self.lines_one)?)),
        ];
        if let Some(column) = column {
            values.push(("column", number(to_client(column, self.columns_one)?)));
        }
        match state {
            BreakpointState::Pending => {
                values.push(("message", text("Source not loaded; breakpoint pending")))
            }
            BreakpointState::Rejected(reason) => values.push(("message", text(reason))),
            _ => {}
        }
        Ok(obj(values))
    }
    fn response(
        &mut self,
        seq: u64,
        command: &str,
        success: bool,
        message: Option<String>,
        body: Value,
    ) -> Result<Vec<u8>, DapError> {
        let output_seq = self.next_output()?;
        let mut fields = vec![
            ("seq", number(output_seq)),
            ("type", text("response")),
            ("request_seq", number(seq)),
            ("command", text(command)),
            ("success", boolean(success)),
            ("body", body),
        ];
        if let Some(message) = message {
            fields.push(("message", text(&message)));
        }
        frame_value(&obj(fields), self.limits.dap_output_bytes)
    }
    fn event(&mut self, event: &str, body: Value) -> Result<Vec<u8>, DapError> {
        let seq = self.next_output()?;
        frame_value(
            &obj(vec![
                ("seq", number(seq)),
                ("type", text("event")),
                ("event", text(event)),
                ("body", body),
            ]),
            self.limits.dap_output_bytes,
        )
    }
    fn next_output(&mut self) -> Result<u64, DapError> {
        self.output_seq = self
            .output_seq
            .checked_add(1)
            .filter(|seq| *seq <= i32::MAX as u64)
            .ok_or(DapError::Capacity)?;
        Ok(self.output_seq)
    }
}
fn advance_state_sequence(last: &mut u64, sequence: u64) -> bool {
    if sequence <= *last {
        return false;
    }
    *last = sequence;
    true
}
fn request_rejection(reason: crate::engine::RejectionReason) -> DapError {
    match reason {
        crate::engine::RejectionReason::Debug(error) => DapError::Debug(error),
        _ => protocol("debug request rejected"),
    }
}
fn check_thread(id: u64) -> Result<(), DapError> {
    if id == 1 {
        Ok(())
    } else {
        Err(protocol("unknown debugger thread"))
    }
}
fn check_control(receipt: DebugControlReceipt) -> Result<(), DapError> {
    match receipt {
        DebugControlReceipt::AcceptedWakeFailed { error } => Err(error.into()),
        _ => Ok(()),
    }
}
fn canonical_uri(path: &str) -> Result<String, DapError> {
    // A native Windows drive prefix is also syntactically a URI scheme. Resolve
    // absolute host paths first so C:\\... does not become an opaque c: URI.
    if let Ok(url) = url::Url::from_file_path(path) {
        return Ok(url.to_string());
    }
    url::Url::parse(path)
        .map(|url| url.to_string())
        .map_err(|_| protocol("source path must be a canonical URI or absolute host path"))
}
fn from_client(value: u32, one: bool) -> Result<u32, DapError> {
    if one {
        value
            .checked_sub(1)
            .ok_or_else(|| protocol("source coordinates must be positive"))
    } else {
        Ok(value)
    }
}
fn to_client(value: u32, one: bool) -> Result<u64, DapError> {
    let value = if one {
        value.checked_add(1).ok_or(DapError::Capacity)?
    } else {
        value
    };
    Ok(u64::from(value))
}
fn same_parent(a: VariableParent, b: VariableParent) -> bool {
    match (a, b) {
        (VariableParent::Scope(a), VariableParent::Scope(b)) => a == b,
        (VariableParent::Object(a), VariableParent::Object(b)) => a == b,
        _ => false,
    }
}

// A tiny output value model keeps every native string in UTF-16. serde_json's
// owned Value requires valid UTF-8 and would reject or lose lone surrogates.
enum Value {
    Text(JsString),
    Number(u64),
    Bool(bool),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
    Raw(Box<RawValue>),
}
fn text(value: &str) -> Value {
    Value::Text(JsString::from_utf8(value))
}
fn number(value: u64) -> Value {
    Value::Number(value)
}
fn boolean(value: bool) -> Value {
    Value::Bool(value)
}
fn array(value: Vec<Value>) -> Value {
    Value::Array(value)
}
fn obj(value: Vec<(&str, Value)>) -> Value {
    Value::Object(
        value
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}
fn write_string<E>(
    value: &JsString,
    write: &mut impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), E> {
    write(b"\"")?;
    let mut units = value.as_units().iter().copied().peekable();
    while let Some(unit) = units.next() {
        match unit {
            0x22 => write(b"\\\"")?,
            0x5c => write(b"\\\\")?,
            0x08 => write(b"\\b")?,
            0x0c => write(b"\\f")?,
            0x0a => write(b"\\n")?,
            0x0d => write(b"\\r")?,
            0x09 => write(b"\\t")?,
            0..=0x1f => write(format!("\\u{unit:04x}").as_bytes())?,
            0xd800..=0xdbff
                if units
                    .peek()
                    .is_some_and(|next| (0xdc00..=0xdfff).contains(next)) =>
            {
                let low = units.next().unwrap();
                let scalar =
                    0x10000 + ((u32::from(unit) - 0xd800) << 10) + (u32::from(low) - 0xdc00);
                let mut encoded = [0; 4];
                write(
                    char::from_u32(scalar)
                        .unwrap()
                        .encode_utf8(&mut encoded)
                        .as_bytes(),
                )?;
            }
            0xd800..=0xdfff => write(format!("\\u{unit:04x}").as_bytes())?,
            _ => {
                let mut encoded = [0; 4];
                write(
                    char::from_u32(u32::from(unit))
                        .unwrap()
                        .encode_utf8(&mut encoded)
                        .as_bytes(),
                )?;
            }
        }
    }
    write(b"\"")
}
fn write_value<E>(value: &Value, write: &mut impl FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
    match value {
        Value::Text(value) => write_string(value, write),
        Value::Number(value) => write(value.to_string().as_bytes()),
        Value::Bool(value) => write(if *value { b"true" } else { b"false" }),
        Value::Raw(value) => write(value.get().as_bytes()),
        Value::Array(values) => {
            write(b"[")?;
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    write(b",")?;
                }
                write_value(value, write)?;
            }
            write(b"]")
        }
        Value::Object(values) => {
            write(b"{")?;
            for (index, (key, value)) in values.iter().enumerate() {
                if index != 0 {
                    write(b",")?;
                }
                write_string(&JsString::from_utf8(key), write)?;
                write(b":")?;
                write_value(value, write)?;
            }
            write(b"}")
        }
    }
}
fn frame_value(value: &Value, limit: usize) -> Result<Vec<u8>, DapError> {
    let mut length = 0usize;
    write_value(value, &mut |bytes| {
        length = length
            .checked_add(bytes.len())
            .filter(|length| *length <= limit)
            .ok_or(DapError::Capacity)?;
        Ok::<(), DapError>(())
    })?;
    let header = format!("Content-Length: {length}\r\n\r\n");
    let mut result = Vec::new();
    result
        .try_reserve_exact(header.len().checked_add(length).ok_or(DapError::Capacity)?)
        .map_err(|_| DapError::Capacity)?;
    result.extend(header.as_bytes());
    // The checked counting pass reserved the exact output capacity. Encoding
    // itself only reports sink failures, so this pass has an infallible sink.
    let encoded = write_value(value, &mut |bytes| {
        result.extend(bytes);
        Ok::<(), std::convert::Infallible>(())
    });
    match encoded {
        Ok(()) => {}
        Err(never) => match never {},
    }
    Ok(result)
}

#[cfg(test)]
#[path = "../../tests/unit/dap.rs"]
mod tests;
