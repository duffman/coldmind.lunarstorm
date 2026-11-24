//! Owner-thread native QuickJS debugger and restricted pause control pump.
use super::{NativeValue, RawContext, callbacks::contain, ffi};
use crate::{debug::*, engine::*};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap, VecDeque},
    ffi::{CString, c_void},
    num::NonZeroU64,
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
#[path = "inspection.rs"]
mod inspection;
#[path = "rejections.rs"]
mod rejections;
const PAGE_LIMIT: usize = 1024;
type FrameVariable = (JsString, Rc<NativeValue>, i32, i32);
struct Signal {
    wake: AtomicBool,
    live: AtomicBool,
}
impl DebugSignal for Signal {
    fn wake(&self) -> Result<(), DebugError> {
        if !self.live.load(Ordering::Acquire) {
            return Err(DebugError::Closed);
        }
        self.wake.store(true, Ordering::Release);
        Ok(())
    }
}
#[derive(Clone)]
struct Marker {
    function: usize,
    marker: u32,
    line: u32,
    column: u32,
    skip_step: Option<bool>,
}
struct Source {
    registration: ScriptRegistration,
    markers: Vec<Marker>,
    _root: Rc<NativeValue>,
}
#[derive(Clone)]
struct Requested {
    selector: ScriptSelector,
    spec: BreakpointSpec,
    state: BreakpointState,
    markers: Vec<(usize, u32)>,
    hits: u64,
}
#[derive(Clone)]
struct Temporary {
    location: DebugLocation,
    markers: Vec<(usize, u32)>,
}
struct Step {
    action: DebugContinue,
    activation: u64,
    depth: i32,
    continuation: bool,
}
#[derive(Default)]
struct State {
    next_key: u64,
    staged: Option<ScriptRegistration>,
    sources: Vec<Source>,
    functions: HashMap<usize, ScriptRegistration>,
    breakpoints: Vec<Requested>,
    objects: Vec<Rc<NativeValue>>,
    cursors: HashMap<u64, inspection::CursorRecord>,
    next_cursor: u64,
    edit_targets: HashMap<inspection::EditKey, (u64, Rc<inspection::EditTarget>)>,
    next_target: u64,
    entry: bool,
    pause: bool,
    evaluating: bool,
    exception_pause: bool,
    step: Option<Step>,
    step_policy: StepPolicy,
    history: Option<AsyncStackStore>,
    history_dropped: u64,
    rejections: Option<rejections::Store>,
    rejection_dropped: u64,
    rejection_origin_limit: usize,
    rejection_origins: VecDeque<(AsyncStackId, Option<DebugLocation>)>,
    rejection_filter: bool,
    pause_uncaught: bool,
    rejection_pause: Option<u64>,
    temporary: Option<Temporary>,
}
#[cfg(test)]
#[derive(Default)]
pub(super) struct LookupObservation {
    pub native_entries: std::sync::atomic::AtomicUsize,
    pub compilations: std::sync::atomic::AtomicUsize,
}
pub(super) struct QuickJsDebug {
    #[cfg(test)]
    pub(super) lookup_observation: Arc<LookupObservation>,
    descriptor: DebugDescriptor,
    context: ContextId,
    raw: Weak<RawContext>,
    port: RefCell<Option<Rc<dyn DebugPort>>>,
    session: Cell<Option<DebugSessionId>>,
    signal: RefCell<Option<Arc<Signal>>>,
    state: RefCell<State>,
    in_pause: Cell<bool>,
    closed: Cell<bool>,
    evaluation: RefCell<Option<EvaluationControl>>,
}
struct EvaluationGuard<'a> {
    debug: &'a QuickJsDebug,
    raw: Rc<RawContext>,
    previous_fault: Option<FailureReport>,
}
impl Drop for EvaluationGuard<'_> {
    fn drop(&mut self) {
        let _ = self.debug.evaluation.borrow_mut().take();
        self.debug.state.borrow_mut().evaluating = false;
        let _ = self.raw.take_fault();
        if let Some(fault) = self.previous_fault.take() {
            self.raw.record_fault(fault);
        }
    }
}
enum Flow {
    Stay,
    Pause,
    Exit(i32),
}
impl QuickJsDebug {
    pub(super) fn new(context: ContextId, raw: Weak<RawContext>) -> Rc<Self> {
        Rc::new(Self {
            #[cfg(test)]
            lookup_observation: Arc::default(),
            descriptor: DebugDescriptor {
                api_version: 1,
                capabilities: DebugCapabilities::baseline()
                    .with(DebugCapability::LazyInspection)
                    .with(DebugCapability::RichInspection)
                    .with(DebugCapability::CancelEvaluation)
                    .with(DebugCapability::VariableMutation)
                    .with(DebugCapability::SafeLookup)
                    .with(DebugCapability::ColumnBreakpoints)
                    .with(DebugCapability::BreakpointPolicies)
                    .with(DebugCapability::ContinueTo)
                    .with(DebugCapability::Logpoints)
                    .with(DebugCapability::SkipSources)
                    .with(DebugCapability::AsyncStacks)
                    .with(DebugCapability::UnhandledRejections)
                    .with(DebugCapability::ExceptionDetails),
                native_requirements: vec![
                    "Pinned QuickJS native markers, verified Rust compiler and owner-thread debugger hooks".into(),
                ],
                interrupt_mechanism: "Revocable atomic wake observed at native statement markers; owner-thread restricted control pump".into(),
            },
            context,
            raw,
            port: RefCell::new(None),
            session: Cell::new(None),
            signal: RefCell::new(None),
            state: RefCell::new(State::default()),
            in_pause: Cell::new(false),
            closed: Cell::new(false),
            evaluation: RefCell::new(None),
        })
    }
    pub(super) fn is_development(&self) -> bool {
        !self.closed.get() && self.port.borrow().is_some()
    }
    pub(super) fn stage_host_script(
        &self,
        source: &ScriptSource,
        kind: ScriptKind,
    ) -> Result<(), FailureReport> {
        let registration = self.register_host_source(source, kind).map_err(failure)?;
        self.state.borrow_mut().staged = registration;
        Ok(())
    }
    pub(super) fn register_host_source(
        &self,
        source: &ScriptSource,
        kind: ScriptKind,
    ) -> Result<Option<ScriptRegistration>, DebugError> {
        if !self.is_development() {
            return Ok(None);
        }
        let key = {
            let mut state = self.state.borrow_mut();
            state.next_key = state.next_key.checked_add(1).ok_or(DebugError::Capacity)?;
            NativeScriptKey::new(
                self.context,
                NonZeroU64::new(state.next_key).ok_or(DebugError::Capacity)?,
            )
        };
        admit(self.port()?.register_script(NativeScriptMetadata {
            key,
            source: source.clone(),
            kind,
        }))
    }
    fn ptr(&self) -> Result<*mut ffi::Context, DebugError> {
        if self.closed.get() {
            return Err(DebugError::Closed);
        }
        self.raw
            .upgrade()
            .ok_or(DebugError::Closed)?
            .ptr()
            .map_err(|_| DebugError::Closed)
    }
    fn port(&self) -> Result<Rc<dyn DebugPort>, DebugError> {
        if self.closed.get() {
            return Err(DebugError::Closed);
        }
        self.port.borrow().clone().ok_or(DebugError::NotAttached)
    }
    pub(super) fn register_source(
        &self,
        text: &[u16],
        origin: &str,
        kind: ScriptKind,
    ) -> Result<Option<ScriptRegistration>, DebugError> {
        if !self.is_development() {
            return Ok(None);
        }
        if let Some(registration) = self.state.borrow_mut().staged.take() {
            if registration.digest != SourceDigest::of_utf16(&JsString::from_units(text.to_vec())) {
                return Err(DebugError::InvalidSource);
            }
            return Ok(Some(registration));
        }
        let key = {
            let mut s = self.state.borrow_mut();
            s.next_key = s.next_key.checked_add(1).ok_or(DebugError::Capacity)?;
            NativeScriptKey::new(
                self.context,
                NonZeroU64::new(s.next_key).ok_or(DebugError::Capacity)?,
            )
        };
        let kind = if self.state.borrow().evaluating {
            ScriptKind::DebuggerEval
        } else {
            kind
        };
        let number = self.state.borrow().next_key;
        let origin = match kind {
            ScriptKind::Eval => format!("quickjs-eval://{number}"),
            ScriptKind::Function => format!("quickjs-function://{number}"),
            ScriptKind::DebuggerEval => format!("quickjs-debugger-eval://{number}"),
            _ => origin.into(),
        };
        admit(self.port()?.register_script(NativeScriptMetadata {
            key,
            source: ScriptSource::new(JsString::from_units(text.to_vec()), origin),
            kind,
        }))
    }
    pub(super) fn loaded(
        &self,
        root: ffi::Value,
        registration: Option<ScriptRegistration>,
    ) -> Result<(), DebugError> {
        let Some(registration) = registration else {
            return Ok(());
        };
        let ptr = self.ptr()?;
        let mut output = MarkerOutput {
            markers: Vec::new(),
            failed: false,
        };
        // SAFETY: the synchronous native visitor borrows this live output only for this call.
        if unsafe {
            ffi::jbq_debug_markers(
                ptr,
                root,
                collect_marker,
                (&mut output as *mut MarkerOutput).cast(),
            )
        } < 0
        {
            return Err(DebugError::Unsupported);
        }
        if output.failed {
            return Err(DebugError::Capacity);
        }
        let markers = output.markers;
        let raw = self.raw.upgrade().ok_or(DebugError::Closed)?;
        // SAFETY: root belongs to the live owner-thread context; duplicate it for retained marker ownership.
        let root = Rc::new(NativeValue::new(unsafe { ffi::jbq_dup(ptr, root) }, raw));
        {
            let mut s = self.state.borrow_mut();
            for marker in &markers {
                s.functions.insert(marker.function, registration);
            }
            s.sources.push(Source {
                registration,
                markers,
                _root: root,
            });
        }
        self.invalidate_temporary(ptr)?;
        self.reverify(ptr)
    }
    pub(super) fn close(&self) {
        if self.closed.get() {
            return;
        }
        let _ = self.end_session();
        if let Ok(ptr) = self.ptr() {
            // SAFETY: ptr is live and owner-thread confined; remove the hook before releasing debugger state.
            unsafe {
                ffi::jbq_debug_rejection_install(ptr, None, std::ptr::null_mut());
                ffi::jbq_debug_async_install(ptr, None, std::ptr::null_mut());
                ffi::jbq_debug_install(ptr, None, std::ptr::null_mut());
            }
        }
        *self.state.borrow_mut() = State::default();
        self.port.borrow_mut().take();
        self.closed.set(true);
    }
    fn frame(
        &self,
        ptr: *mut ffi::Context,
        level: i32,
    ) -> Result<(ffi::DebugFrame, JsString), DebugError> {
        self.frame_bounded(ptr, level, 65_536)
    }
    fn frame_bounded(
        &self,
        ptr: *mut ffi::Context,
        level: i32,
        name_units: usize,
    ) -> Result<(ffi::DebugFrame, JsString), DebugError> {
        let mut f = ffi::DebugFrame::default();
        // SAFETY: constructing immediate undefined values requires no context or borrowed storage.
        let (mut name, mut origin) = unsafe { (ffi::jbq_undefined(), ffi::jbq_undefined()) };
        // SAFETY: ptr is live on its owner thread and all frame/value outputs are valid writable locals.
        if unsafe { ffi::jbq_debug_frame_at(ptr, level, &mut f, &mut name, &mut origin) } < 0 {
            return Err(DebugError::InvalidReference);
        }
        // Anonymous/native frames may have no name atom; this is absence of
        // metadata, distinct from failure to copy an existing string.
        // SAFETY: frame_at returned this owned value in the same live context.
        let text = if unsafe { ffi::jbq_kind(ptr, name) } == 0 {
            Ok(JsString::default())
        } else {
            // A frame label is a preview: an oversized name truncates instead of failing the hook.
            string(ptr, name, Some(name_units))
        };
        // SAFETY: frame_at returned owned name/origin values belonging to this live context.
        unsafe {
            ffi::jbq_free(ptr, name);
            ffi::jbq_free(ptr, origin)
        }
        Ok((f, text?))
    }
    fn location(&self, f: &ffi::DebugFrame) -> Option<DebugLocation> {
        let s = self.state.borrow();
        let r = s.functions.get(&f.function)?;
        Some(DebugLocation {
            script: r.id,
            revision: r.revision,
            line: f.line.max(1) as u32 - 1,
            column: Some(f.column.max(1) as u32 - 1),
        })
    }
    fn application_stopping(&self) -> bool {
        self.raw
            .upgrade()
            .is_some_and(|raw| raw.interrupt.load(Ordering::Acquire))
            || self
                .port
                .borrow()
                .as_ref()
                .is_some_and(|port| port.is_stopping())
    }
    fn on_hook(&self, ptr: *mut ffi::Context, event: i32) -> Result<i32, DebugError> {
        if self.session.get().is_none() || self.state.borrow().evaluating {
            return Ok(0);
        }
        if matches!(event, 4..=6) {
            return self.step_transition(ptr, event);
        }
        self.invalidate_temporary(ptr)?;
        let wake = self
            .signal
            .borrow()
            .as_ref()
            .is_some_and(|s| s.wake.swap(false, Ordering::AcqRel));
        if wake {
            let port = self.port()?;
            while let Some(d) = port.next_directive(false) {
                match self.dispatch(ptr, d, None)? {
                    Flow::Stay => {}
                    Flow::Pause => self.state.borrow_mut().pause = true,
                    Flow::Exit(c) => return Ok(c),
                }
            }
        }
        if event == 0 {
            let state = self.state.borrow();
            if !state.entry && !state.pause && state.step.is_none() {
                return Ok(0);
            }
        }
        let (frame, _) = self.frame(ptr, 0)?;
        // SAFETY: the hook runs on the context owner thread with its native stack still suspended.
        let depth = unsafe { ffi::jbq_debug_frame_count(ptr) };
        let entry = std::mem::take(&mut self.state.borrow_mut().entry);
        if entry {
            let code = self.pause_at(ptr, PauseReason::Entry, &frame, Vec::new())?;
            if code != 0 || event == 0 || self.session.get().is_none() {
                return Ok(code);
            }
            // Entry is a configuration gate, not consumption of the application's
            // breakpoint/debugger/exception event at this same native location.
        }
        // Native events can coincide with step/user pause. Process every logical
        // breakpoint before choosing the stop reason; a logpoint never consumes it.
        let breakpoint_ids = if matches!(event, 0..=2) {
            self.hit_policies(ptr, &frame)?
        } else {
            Vec::new()
        };
        if self.application_stopping() {
            self.end_session()?;
            return Ok(1);
        }
        let skipped = if self.state.borrow().step.is_some() {
            self.skip_step_source(&frame)?
        } else {
            false
        };
        let reason = {
            let mut state = self.state.borrow_mut();
            let temporary = state.temporary.as_ref().is_some_and(|temporary| {
                temporary.markers.contains(&(frame.function, frame.marker))
            });
            if std::mem::take(&mut state.pause) {
                Some(PauseReason::Requested)
            } else if event == 2 {
                Some(PauseReason::DebuggerStatement)
            } else if event == 3 {
                Some(PauseReason::Exception)
            } else if !breakpoint_ids.is_empty() || temporary {
                Some(PauseReason::Breakpoint)
            } else if !skipped
                && state.step.as_ref().is_some_and(|step| match step.action {
                    DebugContinue::StepInto => {
                        !step.continuation || frame.activation == step.activation
                    }
                    DebugContinue::StepOver => {
                        frame.activation == step.activation
                            || (!step.continuation && depth < step.depth)
                    }
                    DebugContinue::StepOut => !step.continuation && depth < step.depth,
                    DebugContinue::Resume => false,
                })
            {
                Some(PauseReason::Step)
            } else {
                None
            }
        };
        let Some(reason) = reason else {
            return Ok(0);
        };
        self.pause_at(ptr, reason, &frame, breakpoint_ids)
    }

    // Return/await hooks only update bounded scalar step state. In particular,
    // they never pump controls or touch an application's pending exception.
    fn step_transition(&self, ptr: *mut ffi::Context, event: i32) -> Result<i32, DebugError> {
        let mut state = self.state.borrow_mut();
        let Some(step) = state.step.as_mut() else {
            return Ok(0);
        };
        let (mut activation, mut kind, mut parent) = (0, 0, 0);
        // SAFETY: invoked before the native frame is unlinked; metadata query is allocation-free.
        let status = unsafe {
            ffi::jbq_debug_step_info(
                ptr,
                step.activation,
                &mut activation,
                &mut kind,
                &mut parent,
            )
        };
        if activation != step.activation {
            return Ok(0);
        }
        if event == 5 {
            step.continuation = true;
            return Ok(0);
        }
        // Ordinary generator yields use the live .next() caller and physical
        // depth policy. Async yields/returns must follow the queued next promise.
        if !step.continuation {
            return Ok(0);
        }
        if status >= 0 && parent != 0 {
            step.activation = parent;
            step.action = DebugContinue::StepOver;
        } else {
            state.step = None;
            drop(state);
            self.port()?.publish(DebugUpdate::Diagnostic {
                report: FailureReport::new(
                    FailureClass::Script,
                    FailureCode::ScriptException,
                    "step continuation ended without a unique bounded live caller",
                ),
                dropped: 0,
            })?;
        }
        Ok(0)
    }

    fn capture_history(&self, ptr: *mut ffi::Context, kind: i32, parent: u64) -> u64 {
        if self.session.get().is_none() || self.in_pause.get() || self.closed.get() {
            return 0;
        }
        let mut state = self.state.borrow_mut();
        if state.evaluating {
            return 0;
        }
        let Some(kind) = AsyncScheduleKind::from_native(kind) else {
            return 0;
        };
        let mut locations = Vec::new();
        if locations.try_reserve_exact(32).is_err() {
            state.history_dropped = state.history_dropped.saturating_add(1);
            return 0;
        }
        let mut truncated = false;
        for level in 0..64 {
            let mut frame = ffi::DebugFrame::default();
            // SAFETY: raw scalar metadata is copied from the live owner stack; no JS allocation.
            if unsafe { ffi::jbq_debug_frame_metadata(ptr, level, &mut frame) } < 0 {
                break;
            }
            if let Some(source) = state.functions.get(&frame.function) {
                if locations.len() == 32 {
                    truncated = true;
                    break;
                }
                locations.push(DebugLocation {
                    script: source.id,
                    revision: source.revision,
                    line: frame.line.max(1) as u32 - 1,
                    column: Some(frame.column.max(1) as u32 - 1),
                });
            }
            if level == 63 {
                truncated = true;
            }
        }
        let Some(history) = state.history.as_mut() else {
            return 0;
        };
        let resolved = if parent == 0 {
            None
        } else {
            history.resolve_serial(parent).ok()
        };
        truncated |= parent != 0 && resolved.is_none();
        let rejection_location = locations.first().cloned();
        match history.capture_truncated(
            resolved,
            JsString::from_utf8(kind.label()),
            locations,
            truncated,
        ) {
            Ok(id) => {
                if matches!(kind, AsyncScheduleKind::Rejection) && state.rejection_origin_limit != 0
                {
                    if state.rejection_origins.len() == state.rejection_origin_limit {
                        state.rejection_origins.pop_front();
                    }
                    state.rejection_origins.push_back((id, rejection_location));
                }
                id.serial()
            }
            Err(_) => {
                state.history_dropped = state.history_dropped.saturating_add(1);
                0
            }
        }
    }
    fn async_trace(&self, ptr: *mut ffi::Context) -> Result<DebugReply, DebugError> {
        let state = self.state.borrow();
        let history = state.history.as_ref().ok_or(DebugError::Unsupported)?;
        let limits = self.port()?.limits().clone();
        // SAFETY: scalar owner-local scheduling metadata, independent of JS values.
        let head = state
            .rejection_pause
            .and_then(|serial| state.rejections.as_ref()?.by_serial(serial)?.history)
            .map(AsyncStackId::serial)
            .unwrap_or_else(|| {
                // SAFETY: owner-only integer slot; a historical pause implies no live frame.
                unsafe { ffi::jbq_debug_async_stack(ptr) }
            });
        let mut trace = AsyncStackTrace {
            retained_records: history.count(),
            retained_bytes: history.retained_bytes(),
            truncated: state.history_dropped != 0,
            ..Default::default()
        };
        let mut current = if head == 0 {
            None
        } else {
            match history.resolve_serial(head) {
                Ok(id) => Some(id),
                Err(_) => {
                    trace.truncated = true;
                    None
                }
            }
        };
        let limit = limits.async_history_depth.min(history.count());
        let capacity = limit.min(limits.reply_bytes / std::mem::size_of::<AsyncStackTraceEntry>());
        trace
            .entries
            .try_reserve_exact(capacity)
            .map_err(|_| DebugError::Capacity)?;
        let mut bytes = std::mem::size_of::<DebugReply>()
            .checked_add(trace.entries.capacity() * std::mem::size_of::<AsyncStackTraceEntry>())
            .ok_or(DebugError::Capacity)?;
        while let Some(id) = current {
            if trace.entries.len() >= capacity {
                return Err(DebugError::Capacity);
            }
            let record = history.read(id)?;
            bytes = bytes
                .checked_add(record.label.capacity_bytes())
                .and_then(|n| {
                    n.checked_add(
                        record.locations.capacity() * std::mem::size_of::<DebugLocation>(),
                    )
                })
                .ok_or(DebugError::Capacity)?;
            if bytes > limits.reply_bytes {
                return Err(DebugError::Capacity);
            }
            current = record.parent;
            trace.truncated |= record.truncated;
            trace.entries.push(AsyncStackTraceEntry {
                id,
                record,
                originals: Vec::new(),
            });
        }
        Ok(DebugReply::AsyncStack(trace))
    }

    fn observe_rejection(
        &self,
        ptr: *mut ffi::Context,
        promise: ffi::Value,
        reason: ffi::Value,
        handled: i32,
        origin: u64,
    ) {
        if self.session.get().is_none() || self.in_pause.get() || self.closed.get() {
            return;
        }
        // SAFETY: this callback borrows an actual local Promise; identity access never executes JS.
        let key = unsafe { ffi::jbq_debug_promise_identity(ptr, promise) };
        if key == 0 {
            return;
        }
        let mut state = self.state.borrow_mut();
        if state.evaluating {
            return;
        }
        if handled != 0 {
            if let Some(store) = state.rejections.as_mut() {
                store.handle(key);
            }
            return;
        }
        let cached = state
            .rejection_origins
            .iter()
            .find(|(id, _)| id.serial() == origin)
            .cloned();
        let history = state
            .history
            .as_ref()
            .and_then(|h| h.resolve_serial(origin).ok())
            .or_else(|| cached.as_ref().map(|(id, _)| *id));
        let location = history
            .and_then(|id| {
                state
                    .history
                    .as_ref()?
                    .read(id)
                    .ok()?
                    .locations
                    .first()
                    .cloned()
            })
            .or_else(|| cached.and_then(|(_, location)| location));
        if origin != 0 && history.is_none() {
            state.history_dropped = state.history_dropped.saturating_add(1);
        }
        let Some(raw) = self.raw.upgrade() else {
            return;
        };
        // SAFETY: retaining borrowed local values adds roots without JS execution; table owns both.
        let (promise, reason) = unsafe {
            (
                Rc::new(NativeValue::new(ffi::jbq_dup(ptr, promise), raw.clone())),
                Rc::new(NativeValue::new(ffi::jbq_dup(ptr, reason), raw)),
            )
        };
        if state.rejections.as_mut().is_some_and(|store| {
            store
                .observe(key, promise, reason, location, history)
                .is_err()
        }) {
            state.rejection_dropped = state.rejection_dropped.saturating_add(1);
        }
    }
    fn rejection_info(
        &self,
        ptr: *mut ffi::Context,
        serial: u64,
    ) -> Result<PromiseRejectionInfo, DebugError> {
        let state = self.state.borrow();
        let record = state
            .rejections
            .as_ref()
            .and_then(|s| s.by_serial(serial))
            .ok_or(DebugError::InvalidReference)?;
        // SAFETY: retained Promise root must still have its original identity on this owner.
        if unsafe { ffi::jbq_debug_promise_identity(ptr, record.promise.value) } != record.key {
            return Err(DebugError::InvalidReference);
        }
        let mut description = std::mem::MaybeUninit::<ffi::DebugValueInfo>::uninit();
        // SAFETY: retained local reason, fixed output, no getters/formatting/JS execution.
        let error_description = if unsafe {
            ffi::jbq_debug_describe_exception(ptr, record.reason.value, description.as_mut_ptr())
        } == 1
        {
            // SAFETY: success initializes the entire fixed native description.
            Some(unsafe { description.assume_init() })
        } else {
            None
        };
        let description = error_description
            .or_else(|| self.raw_info(ptr, record.reason.value))
            .map(|info| {
                JsString::from_units(info.text[..info.length.min(info.text.len())].to_vec())
            })
            .unwrap_or_else(|| JsString::from_utf8("<reason unavailable>"));
        Ok(PromiseRejectionInfo {
            id: PromiseRejectionId::new(self.context, serial)?,
            state: match record.status {
                rejections::Status::Pending => PromiseRejectionState::Pending,
                rejections::Status::Unhandled => PromiseRejectionState::Unhandled,
                rejections::Status::HandledLater => PromiseRejectionState::HandledLater,
            },
            description,
            location: record.location.clone(),
            original: None,
            history: record.history,
        })
    }
    fn pause_at(
        &self,
        ptr: *mut ffi::Context,
        reason: PauseReason,
        frame: &ffi::DebugFrame,
        breakpoint_ids: Vec<u64>,
    ) -> Result<i32, DebugError> {
        self.replace_temporary(ptr, None)?;
        self.state.borrow_mut().step = None;
        self.state.borrow_mut().exception_pause = reason == PauseReason::Exception;
        let port = self.port()?;
        let pause = port
            .identities()
            .begin_pause(self.session.get().ok_or(DebugError::NotAttached)?)?;
        port.publish(DebugUpdate::Paused {
            sequence: port.identities().next_update_sequence()?,
            pause,
            reason,
            location: self.location(frame),
            original: None,
            breakpoint_ids,
        })?;
        self.wait_pause(ptr, pause)
    }
    fn wait_pause(&self, ptr: *mut ffi::Context, pause: PauseId) -> Result<i32, DebugError> {
        let port = self.port()?;
        loop {
            let Some(d) = port.next_directive(true) else {
                self.end_session()?;
                return Ok(0);
            };
            if let Flow::Exit(c) = self.dispatch(ptr, d, Some(pause))? {
                return Ok(c);
            }
        }
    }
    fn pause_rejection(&self, ptr: *mut ffi::Context, serial: u64) -> Result<(), DebugError> {
        self.replace_temporary(ptr, None)?;
        let location = self.rejection_info(ptr, serial)?.location;
        {
            let mut state = self.state.borrow_mut();
            state.step = None;
            state.exception_pause = false;
            state.rejection_pause = Some(serial);
        }
        let port = self.port()?;
        let pause = port
            .identities()
            .begin_pause(self.session.get().ok_or(DebugError::NotAttached)?)?;
        port.publish(DebugUpdate::Paused {
            sequence: port.identities().next_update_sequence()?,
            pause,
            reason: PauseReason::UnhandledRejection,
            location,
            original: None,
            breakpoint_ids: Vec::new(),
        })?;
        let previous = self.in_pause.replace(true);
        let result = self.wait_pause(ptr, pause);
        self.in_pause.set(previous);
        result.map(|_| ())
    }
    fn skip_step_source(&self, frame: &ffi::DebugFrame) -> Result<bool, DebugError> {
        let (index, marker, location, policy) = {
            let state = self.state.borrow();
            if state.step_policy.skip_sources.is_empty() {
                return Ok(false);
            }
            let Some(registration) = state.functions.get(&frame.function) else {
                return Ok(false);
            };
            let Some(index) = state
                .sources
                .iter()
                .position(|source| source.registration == *registration)
            else {
                return Ok(false);
            };
            let Some(marker) = state.sources[index].markers.iter().position(|marker| {
                marker.function == frame.function && marker.marker == frame.marker
            }) else {
                return Ok(false);
            };
            let metadata = &state.sources[index].markers[marker];
            if let Some(skipped) = metadata.skip_step {
                return Ok(skipped);
            }
            (
                index,
                marker,
                DebugLocation {
                    script: registration.id,
                    revision: registration.revision,
                    line: metadata.line,
                    column: Some(metadata.column),
                },
                state.step_policy.clone(),
            )
        };
        let skipped = match self.port()?.skip_source(&location, &policy) {
            Ok(skipped) => skipped,
            Err(error) => {
                self.port()?.publish(DebugUpdate::Diagnostic {
                    report: FailureReport::new(
                        FailureClass::Script,
                        FailureCode::ScriptException,
                        &format!("source skip rule unavailable: {error}; step remains visible"),
                    ),
                    dropped: 0,
                })?;
                false
            }
        };
        self.state.borrow_mut().sources[index].markers[marker].skip_step = Some(skipped);
        Ok(skipped)
    }
    fn policy_diagnostic(&self, id: u64, error: &DebugError) -> Result<(), DebugError> {
        self.port()?.publish(DebugUpdate::Diagnostic {
            report:FailureReport::new(FailureClass::Script,FailureCode::ScriptException,
                &format!("breakpoint {id} condition/logpoint failed within its evaluator budget: {error}; this policy did not stop execution")), dropped:0,
        })
    }
    fn policy_evaluate(
        &self,
        ptr: *mut ffi::Context,
        expression: &JsString,
        started: Instant,
    ) -> Result<Rc<NativeValue>, DebugError> {
        let control = EvaluationControl::new_at(
            EvaluationOptions {
                deadline_ms: self.port()?.limits().condition_deadline_ms,
                ..EvaluationOptions::default()
            },
            started,
        )?;
        let (value, status) = self.evaluate_controlled(ptr, 0, expression, control)?;
        match status {
            EvaluationStatus::Returned => value.ok_or(DebugError::InvalidRequest),
            EvaluationStatus::Threw => Err(DebugError::InvalidRequest),
            EvaluationStatus::Cancelled => Err(DebugError::EvaluationCancelled {
                effects_may_remain: true,
            }),
            EvaluationStatus::DeadlineExceeded => Err(DebugError::EvaluationDeadlineExceeded {
                effects_may_remain: true,
            }),
        }
    }
    fn hit_policies(
        &self,
        ptr: *mut ffi::Context,
        frame: &ffi::DebugFrame,
    ) -> Result<Vec<u64>, DebugError> {
        // Snapshot and increment before evaluation. No RefCell guard survives a
        // compiler/native evaluator or host callback; condition effects may reenter.
        let policies: Vec<_> = {
            let mut state = self.state.borrow_mut();
            state
                .breakpoints
                .iter_mut()
                .filter(|bp| {
                    bp.spec.enabled && bp.markers.contains(&(frame.function, frame.marker))
                })
                .map(|bp| {
                    let count = bp.hits.checked_add(1);
                    if let Some(count) = count {
                        bp.hits = count;
                    }
                    (bp.spec.clone(), count)
                })
                .collect()
        };
        let started = Instant::now();
        let mut matched = Vec::new();
        matched
            .try_reserve_exact(policies.len())
            .map_err(|_| DebugError::Capacity)?;
        for (spec, count) in policies {
            let result = (|| {
                let count = count.ok_or(DebugError::Capacity)?;
                if let Some(expression) = &spec.condition {
                    let value = self.policy_evaluate(ptr, expression, started)?;
                    // SAFETY: selected-frame evaluation returned a live retained value;
                    // native boolean conversion cannot invoke application hooks.
                    if unsafe { ffi::jbq_to_bool(ptr, value.value) } == 0 {
                        return Ok(false);
                    }
                }
                if spec.hit_condition.is_some_and(|hit| !hit.matches(count)) {
                    return Ok(false);
                }
                if let Some(message) = spec
                    .log_message
                    .as_ref()
                    .filter(|message| !message.as_units().is_empty())
                {
                    self.logpoint(ptr, message, started)?;
                    Ok(false)
                } else {
                    Ok(true)
                }
            })();
            match result {
                Ok(true) => matched.push(spec.client_id),
                Ok(false) => {}
                Err(error) => self.policy_diagnostic(spec.client_id, &error)?,
            }
            if self.application_stopping() {
                break;
            }
        }
        Ok(matched)
    }
    fn logpoint(
        &self,
        ptr: *mut ffi::Context,
        message: &JsString,
        started: Instant,
    ) -> Result<(), DebugError> {
        use crate::debug::logpoint::{LogPart, LogParts};
        let port = self.port()?;
        let maximum = port
            .limits()
            .reply_bytes
            .saturating_sub(std::mem::size_of::<DebugUpdate>())
            / 2;
        let mut units = Vec::new();
        for part in LogParts::new(message.as_units()) {
            if started.elapsed().as_millis() >= u128::from(port.limits().condition_deadline_ms) {
                return Err(DebugError::EvaluationDeadlineExceeded {
                    effects_may_remain: true,
                });
            }
            let remaining = maximum.saturating_sub(units.len()).saturating_sub(1);
            let text = match part? {
                LogPart::Text(text) => std::borrow::Cow::Borrowed(text),
                LogPart::Brace(brace) => std::borrow::Cow::Owned(vec![brace]),
                LogPart::Expression(expression) => {
                    let value = self.policy_evaluate(
                        ptr,
                        &JsString::from_units(expression.to_vec()),
                        started,
                    )?;
                    std::borrow::Cow::Owned(
                        self.log_value(ptr, value.value, remaining)?.into_units(),
                    )
                }
            };
            if text.len() > remaining {
                return Err(DebugError::Capacity);
            }
            units
                .try_reserve_exact(text.len())
                .map_err(|_| DebugError::Capacity)?;
            units.extend_from_slice(&text);
        }
        if started.elapsed().as_millis() >= u128::from(port.limits().condition_deadline_ms) {
            return Err(DebugError::EvaluationDeadlineExceeded {
                effects_may_remain: true,
            });
        }
        if units.len() >= maximum {
            return Err(DebugError::Capacity);
        }
        units
            .try_reserve_exact(1)
            .map_err(|_| DebugError::Capacity)?;
        units.push(10);
        port.publish(DebugUpdate::Output {
            message: JsString::from_units(units),
        })
    }
    fn log_value(
        &self,
        ptr: *mut ffi::Context,
        value: ffi::Value,
        remaining: usize,
    ) -> Result<JsString, DebugError> {
        // SAFETY: value is retained in this live context, and kind queries are raw.
        match unsafe { ffi::jbq_kind(ptr, value) } {
            0 => Ok(JsString::from_utf8("undefined")),
            1 => Ok(JsString::from_utf8("null")),
            // SAFETY: the matched primitive cannot call application conversions.
            2 => Ok(JsString::from_utf8(
                if unsafe { ffi::jbq_to_bool(ptr, value) } != 0 {
                    "true"
                } else {
                    "false"
                },
            )),
            3 => {
                let mut number = 0.;
                // SAFETY: matched primitive number, with valid writable output.
                unsafe { ffi::jbq_to_number(ptr, value, &mut number) };
                Ok(JsString::from_utf8(&js_number(number)))
            }
            4 => {
                let mut length = 0;
                // SAFETY: matched primitive string; the zero-capacity query writes no text.
                if unsafe {
                    ffi::jbq_debug_copy_string(value, std::ptr::null_mut(), 0, &mut length)
                } < 0
                {
                    return Err(DebugError::InvalidRequest);
                }
                if length > remaining {
                    return Err(DebugError::Capacity);
                }
                string(ptr, value, Some(remaining))
            }
            5 | 6 => self
                .raw_info(ptr, value)
                .map(|info| JsString::from_units(info.text[..info.length].to_vec()))
                .ok_or(DebugError::Unsupported),
            _ => Ok(JsString::from_utf8("Object")),
        }
    }
    fn dispatch(
        &self,
        ptr: *mut ffi::Context,
        d: DebugDirective,
        pause: Option<PauseId>,
    ) -> Result<Flow, DebugError> {
        let port = self.port()?;
        match d {
            DebugDirective::Request(command) => {
                let continue_to = if let DebugRequest::ContinueTo { pause, .. } = &command.request {
                    Some(*pause)
                } else {
                    None
                };
                let result = self.request(ptr, command.id, command.request, pause);
                let resume_to =
                    continue_to.filter(|_| matches!(result, Ok(DebugReply::ContinueTo)));
                port.publish(DebugUpdate::Reply {
                    id: command.id,
                    result,
                })?;
                if self.application_stopping() {
                    self.end_session()?;
                    Ok(Flow::Exit(1))
                } else if let Some(pause) = resume_to {
                    self.resume_at(ptr, pause, DebugContinue::Resume, true)
                } else {
                    Ok(Flow::Stay)
                }
            }
            DebugDirective::Pause => Ok(if pause.is_some() {
                Flow::Stay
            } else {
                Flow::Pause
            }),
            DebugDirective::Continue {
                pause: target,
                action,
            } => {
                if pause != Some(target) {
                    return Ok(Flow::Stay);
                }
                self.resume_at(ptr, target, action, false)
            }
            DebugDirective::Detach => {
                self.end_session()?;
                Ok(Flow::Exit(0))
            }
            DebugDirective::Stop => {
                self.end_session()?;
                Ok(Flow::Exit(1))
            }
        }
    }
    fn resume_at(
        &self,
        ptr: *mut ffi::Context,
        target: PauseId,
        action: DebugContinue,
        keep_temporary: bool,
    ) -> Result<Flow, DebugError> {
        if !keep_temporary {
            self.replace_temporary(ptr, None)?;
        }
        let port = self.port()?;
        if self.state.borrow().rejection_pause.is_some() && action != DebugContinue::Resume {
            return Err(DebugError::Unsupported);
        }
        let step = if action == DebugContinue::Resume {
            None
        } else {
            let (f, _) = self.frame(ptr, 0)?;
            let (mut activation, mut kind, mut parent) = (0, 0, 0);
            // SAFETY: scalar metadata from the live owner-thread pause; no JS execution.
            unsafe { ffi::jbq_debug_step_info(ptr, 0, &mut activation, &mut kind, &mut parent) };
            if activation == 0 {
                return Err(DebugError::Unsupported);
            }
            Some(Step {
                continuation: kind & 2 != 0 && action != DebugContinue::StepInto,
                action,
                activation: f.activation,
                // SAFETY: the selected pause holds the live context stack on its owner thread.
                depth: unsafe { ffi::jbq_debug_frame_count(ptr) },
            })
        };
        port.identities().end_pause(target)?;
        {
            let mut s = self.state.borrow_mut();
            s.exception_pause = false;
            s.rejection_pause = None;
            s.objects.clear();
            s.cursors.clear();
            s.edit_targets.clear();
            s.step = step;
        }
        port.publish(DebugUpdate::Resumed {
            sequence: port.identities().next_update_sequence()?,
            previous: target,
        })?;
        Ok(Flow::Exit(0))
    }
    fn end_session(&self) -> Result<(), DebugError> {
        let Some(session) = self.session.take() else {
            return Ok(());
        };
        let ptr = self.ptr()?;
        self.replace_temporary(ptr, None)?;
        // SAFETY: ptr is the live owner-thread context; clearing debugger policy retains no borrowed data.
        unsafe {
            ffi::jbq_debug_flags(ptr, 0);
            ffi::jbq_debug_async_set(ptr, 0);
            ffi::jbq_debug_exception_policy(ptr, 0)
        }
        let mut s = self.state.borrow_mut();
        for bp in &s.breakpoints {
            for &(function, marker) in &bp.markers {
                // SAFETY: retained source roots keep these marker/function identities valid in this context.
                unsafe { ffi::jbq_debug_breakpoint(ptr, function, marker, 0) };
            }
        }
        s.breakpoints.clear();
        s.exception_pause = false;
        s.objects.clear();
        s.cursors.clear();
        s.edit_targets.clear();
        s.entry = false;
        s.pause = false;
        s.step = None;
        s.step_policy = StepPolicy::default();
        if let Some(history) = s.history.as_mut() {
            history.clear();
        }
        s.history_dropped = 0;
        if let Some(rejections) = s.rejections.as_mut() {
            rejections.clear();
        }
        s.rejection_dropped = 0;
        s.rejection_origins.clear();
        s.rejection_filter = false;
        s.pause_uncaught = false;
        s.rejection_pause = None;
        drop(s);
        let port = self.port()?;
        if let DebugExecutionState::Paused(p) = port.identities().state() {
            port.identities().end_pause(p)?;
        }
        let _ = port.identities().close_session(session);
        let result = port.publish(DebugUpdate::SessionClosed {
            session,
            reason: None,
        });
        // SessionClosed closes transport admission and drains entered signal
        // calls. Revoking earlier lets a concurrent detach/drop see an open
        // transport with a dead signal and incorrectly escalate to VM stop.
        if let Some(signal) = self.signal.borrow_mut().take() {
            signal.live.store(false, Ordering::Release)
        }
        result
    }
    fn set_breakpoints(
        &self,
        ptr: *mut ffi::Context,
        selector: ScriptSelector,
        specs: Vec<BreakpointSpec>,
    ) -> Result<DebugReply, DebugError> {
        let old = self.state.borrow().breakpoints.clone();
        let mut next: Vec<_> = old
            .iter()
            .filter(|b| {
                b.selector.origin != selector.origin
                    || b.selector.expected_digest != selector.expected_digest
            })
            .cloned()
            .collect();
        let mut infos = Vec::new();
        for spec in specs {
            let mut b = Requested {
                selector: selector.clone(),
                spec,
                state: BreakpointState::Pending,
                markers: Vec::new(),
                hits: 0,
            };
            self.install(ptr, &mut b)?;
            infos.push(BreakpointInfo {
                client_id: b.spec.client_id,
                state: b.state.clone(),
            });
            next.push(b);
        }
        self.update_marker_union(ptr, &old, &next)?;
        self.state.borrow_mut().breakpoints = next;
        Ok(DebugReply::Breakpoints(infos))
    }
    fn update_marker_union(
        &self,
        ptr: *mut ffi::Context,
        old: &[Requested],
        next: &[Requested],
    ) -> Result<(), DebugError> {
        let temporary = self.state.borrow().temporary.clone();
        let union = |bps: &[Requested]| {
            bps.iter()
                .filter(|bp| bp.spec.enabled)
                .flat_map(|bp| bp.markers.iter().copied())
                .chain(
                    temporary
                        .iter()
                        .flat_map(|temporary| temporary.markers.iter().copied()),
                )
                .collect::<BTreeSet<_>>()
        };
        self.apply_marker_union(ptr, union(old), union(next))
    }
    fn apply_marker_union(
        &self,
        ptr: *mut ffi::Context,
        before: BTreeSet<(usize, u32)>,
        after: BTreeSet<(usize, u32)>,
    ) -> Result<(), DebugError> {
        let changes = after
            .difference(&before)
            .map(|&marker| (marker, 1))
            .chain(before.difference(&after).map(|&marker| (marker, 0)));
        let mut applied = Vec::new();
        applied
            .try_reserve(before.len().saturating_add(after.len()))
            .map_err(|_| DebugError::Capacity)?;
        for ((function, marker), enabled) in changes {
            // SAFETY: all marker identities come from retained native function roots in this context.
            if unsafe { ffi::jbq_debug_breakpoint(ptr, function, marker, enabled) } < 0 {
                for ((function, marker), enabled) in applied.into_iter().rev() {
                    // SAFETY: restoring previously validated markers in the same live context.
                    unsafe { ffi::jbq_debug_breakpoint(ptr, function, marker, 1 - enabled) };
                }
                return Err(DebugError::Unsupported);
            }
            applied.push(((function, marker), enabled));
        }
        Ok(())
    }
    fn replace_temporary(
        &self,
        ptr: *mut ffi::Context,
        next: Option<Temporary>,
    ) -> Result<(), DebugError> {
        let (before, after) = {
            let state = self.state.borrow();
            let users: BTreeSet<_> = state
                .breakpoints
                .iter()
                .filter(|bp| bp.spec.enabled)
                .flat_map(|bp| bp.markers.iter().copied())
                .collect();
            let mut before = users.clone();
            let mut after = users;
            if let Some(old) = &state.temporary {
                before.extend(old.markers.iter().copied());
            }
            if let Some(next) = &next {
                after.extend(next.markers.iter().copied());
            }
            (before, after)
        };
        self.apply_marker_union(ptr, before, after)?;
        self.state.borrow_mut().temporary = next;
        Ok(())
    }
    fn invalidate_temporary(&self, ptr: *mut ffi::Context) -> Result<(), DebugError> {
        let location = self
            .state
            .borrow()
            .temporary
            .as_ref()
            .map(|temporary| temporary.location.clone());
        if let Some(location) = location
            && !self.port()?.is_current_script(&location)?
        {
            self.replace_temporary(ptr, None)?;
        }
        Ok(())
    }
    fn arm_continue_to(
        &self,
        ptr: *mut ffi::Context,
        location: DebugLocation,
    ) -> Result<DebugReply, DebugError> {
        if location.column.is_none() || !self.port()?.is_current_script(&location)? {
            return Err(DebugError::InvalidSource);
        }
        let mut markers = Vec::new();
        let mut work = 0usize;
        let port = self.port()?;
        for source in &self.state.borrow().sources {
            for marker in &source.markers {
                work = work.checked_add(1).ok_or(DebugError::Capacity)?;
                if work > port.limits().location_scan_slots {
                    return Err(DebugError::Capacity);
                }
                if source.registration.id == location.script
                    && source.registration.revision == location.revision
                    && marker.line == location.line
                    && Some(marker.column) == location.column
                {
                    if markers.len() >= port.limits().breakpoints {
                        return Err(DebugError::Capacity);
                    }
                    if !markers.contains(&(marker.function, marker.marker)) {
                        markers
                            .try_reserve_exact(1)
                            .map_err(|_| DebugError::Capacity)?;
                        markers.push((marker.function, marker.marker));
                    }
                }
            }
        }
        if markers.is_empty() {
            return Err(DebugError::InvalidSource);
        }
        self.replace_temporary(ptr, Some(Temporary { location, markers }))?;
        Ok(DebugReply::ContinueTo)
    }
    fn set_enabled(
        &self,
        ptr: *mut ffi::Context,
        selector: ScriptSelector,
        client_id: u64,
        enabled: bool,
    ) -> Result<DebugReply, DebugError> {
        let old = self.state.borrow().breakpoints.clone();
        let mut next = old.clone();
        let bp = next
            .iter_mut()
            .find(|bp| {
                bp.selector.origin == selector.origin
                    && bp.selector.expected_digest == selector.expected_digest
                    && bp.spec.client_id == client_id
            })
            .ok_or(DebugError::InvalidRequest)?;
        bp.spec.enabled = enabled;
        let info = BreakpointInfo {
            client_id,
            state: bp.state.clone(),
        };
        self.update_marker_union(ptr, &old, &next)?;
        self.state.borrow_mut().breakpoints = next;
        Ok(DebugReply::Breakpoints(vec![info]))
    }
    fn install(&self, _ptr: *mut ffi::Context, b: &mut Requested) -> Result<(), DebugError> {
        let candidates =
            self.port()?
                .resolve_breakpoint(&b.selector, b.spec.line, b.spec.column)?;
        let mut locations = Vec::new();
        b.markers.clear();
        let s = self.state.borrow();
        for c in &candidates {
            for source in s
                .sources
                .iter()
                .filter(|s| s.registration.id == c.script && s.registration.revision == c.revision)
            {
                let nearest = source
                    .markers
                    .iter()
                    .filter(|m| (m.line, m.column) >= (c.line, c.column.unwrap_or(0)))
                    .map(|m| (m.line, m.column))
                    .min();
                for m in source.markers.iter().filter(|m| {
                    nearest.is_some_and(|position| {
                        if c.column.is_some() {
                            (m.line, m.column) == position
                        } else {
                            m.line == position.0
                        }
                    })
                }) {
                    if !b.markers.contains(&(m.function, m.marker)) {
                        b.markers.push((m.function, m.marker));
                    }
                    if !locations.iter().any(|location: &DebugLocation| {
                        location.script == c.script
                            && location.revision == c.revision
                            && location.line == m.line
                            && location.column == Some(m.column)
                    }) {
                        locations.push(DebugLocation {
                            script: c.script,
                            revision: c.revision,
                            line: m.line,
                            column: Some(m.column),
                        });
                    }
                }
            }
        }
        locations.sort_by_key(|location| (location.line, location.column));
        b.state = if locations.is_empty() {
            if candidates.is_empty() {
                BreakpointState::Pending
            } else {
                BreakpointState::Rejected("no executable marker".into())
            }
        } else {
            BreakpointState::Verified(locations)
        };
        Ok(())
    }
    fn reverify(&self, ptr: *mut ffi::Context) -> Result<(), DebugError> {
        let old = self.state.borrow().breakpoints.clone();
        let mut bps = old.clone();
        let mut changed = Vec::new();
        let result = (|| {
            for b in &mut bps {
                let before = b.markers.len();
                self.install(ptr, b)?;
                if before != b.markers.len() {
                    changed.push(BreakpointInfo {
                        client_id: b.spec.client_id,
                        state: b.state.clone(),
                    })
                }
            }
            Ok(())
        })();
        result?;
        self.update_marker_union(ptr, &old, &bps)?;
        self.state.borrow_mut().breakpoints = bps;
        if !changed.is_empty() {
            self.port()?
                .publish(DebugUpdate::BreakpointsChanged(changed))?
        }
        Ok(())
    }
    fn current(&self, pause: Option<PauseId>, target: PauseId) -> Result<PauseId, DebugError> {
        match pause {
            Some(p) if p == target && self.in_pause.get() => Ok(p),
            Some(_) => Err(DebugError::StalePause),
            None => Err(DebugError::NotPaused),
        }
    }
    fn evaluate_controlled(
        &self,
        ptr: *mut ffi::Context,
        index: i32,
        text: &JsString,
        control: EvaluationControl,
    ) -> Result<(Option<Rc<NativeValue>>, EvaluationStatus), DebugError> {
        let (value, status) = self.evaluate_controlled_raw(ptr, index, text, control.clone())?;
        let status = control.finish(status).ok_or(DebugError::Closed)?;
        if matches!(
            status,
            EvaluationStatus::Cancelled | EvaluationStatus::DeadlineExceeded
        ) {
            return Ok((None, status));
        }
        Ok((value, status))
    }
    fn evaluate_controlled_raw(
        &self,
        ptr: *mut ffi::Context,
        index: i32,
        text: &JsString,
        control: EvaluationControl,
    ) -> Result<(Option<Rc<NativeValue>>, EvaluationStatus), DebugError> {
        if let DebugExecutionState::Paused(pause) = self.port()?.identities().state() {
            self.port()?.identities().invalidate_inspection(pause)?;
            self.state.borrow_mut().cursors.clear();
            self.state.borrow_mut().edit_targets.clear();
        }
        let raw = self.raw.upgrade().ok_or(DebugError::Closed)?;
        let previous_fault = raw.take_fault();
        self.state.borrow_mut().evaluating = true;
        *self.evaluation.borrow_mut() = Some(control.clone());
        let guard = EvaluationGuard {
            debug: self,
            raw: raw.clone(),
            previous_fault,
        };
        let origin = CString::new("quickjs-debugger-eval.js").unwrap();
        let mut value = std::mem::MaybeUninit::uninit();
        // SAFETY: the owner pause and frame lease remain live throughout this
        // synchronous call. Native captures its owned result before restoring
        // the pending application exception and trace, on every exit.
        let native_status = unsafe {
            ffi::jbq_debug_eval_verified(
                ptr,
                index,
                text.as_units().as_ptr(),
                text.as_units().len(),
                origin.as_ptr(),
                value.as_mut_ptr(),
            )
        };
        // SAFETY: the bridge initializes out even when native rejects the call.
        let value = unsafe { value.assume_init() };
        let fault = raw.take_fault();
        drop(guard);
        if self.port()?.is_stopping() || raw.interrupt.load(Ordering::Relaxed) || native_status == 4
        {
            // SAFETY: native transferred this owned result to the owner.
            unsafe { ffi::jbq_free(ptr, value) };
            return Err(DebugError::Closed);
        }
        let native_status = match native_status {
            0 => EvaluationStatus::Returned,
            1 => EvaluationStatus::Threw,
            2 => EvaluationStatus::Cancelled,
            3 => EvaluationStatus::DeadlineExceeded,
            status => {
                // SAFETY: native transferred this owned result to the owner.
                unsafe { ffi::jbq_free(ptr, value) };
                return Err(if status == 5 {
                    DebugError::Capacity
                } else {
                    DebugError::InvalidRequest
                });
            }
        };
        if matches!(
            native_status,
            EvaluationStatus::Cancelled | EvaluationStatus::DeadlineExceeded
        ) {
            // SAFETY: native transferred this owned value, discarded on termination.
            unsafe { ffi::jbq_free(ptr, value) };
            return Ok((None, native_status));
        }
        let value = owned(value, &self.raw)?;
        if let Some(fault) = fault {
            return Err(DebugError::EngineFailure(fault));
        }
        Ok((Some(value), native_status))
    }
    fn exception_info(
        &self,
        ptr: *mut ffi::Context,
        target: PauseId,
        pause: Option<PauseId>,
    ) -> Result<DebugReply, DebugError> {
        let pause = self.current(pause, target)?;
        let rejection_pause = self.state.borrow().rejection_pause;
        if let Some(serial) = rejection_pause {
            let (reason, location) = {
                let state = self.state.borrow();
                let record = state
                    .rejections
                    .as_ref()
                    .and_then(|s| s.by_serial(serial))
                    .ok_or(DebugError::InvalidReference)?;
                (record.reason.clone(), record.location.clone())
            };
            let info = self.rejection_info(ptr, serial)?;
            let value = self.preview(ptr, pause, reason);
            let stack = location
                .clone()
                .map(|generated| ExceptionFrame {
                    name: JsString::from_utf8("Promise rejection"),
                    generated: Some(generated),
                    original: None,
                })
                .into_iter()
                .collect();
            return Ok(DebugReply::ExceptionInfo(ExceptionInfo {
                value,
                location,
                caught: false,
                stack,
                description: info.description,
            }));
        }
        if !self.state.borrow().exception_pause {
            return Err(DebugError::InvalidRequest);
        }
        // SAFETY: constructing immediate undefined does not touch context state.
        let mut original = unsafe { ffi::jbq_undefined() };
        let mut caught = 0;
        // SAFETY: only the original exception hook exposes this owned duplicate;
        // no field lookup, callback, pending-exception mutation or conversion occurs.
        if unsafe { ffi::jbq_debug_exception(ptr, &mut original, &mut caught) } < 0 {
            return Err(DebugError::InvalidRequest);
        }
        let value = self.preview(ptr, pause, owned(original, &self.raw)?);
        let mut description = std::mem::MaybeUninit::<ffi::DebugValueInfo>::uninit();
        // SAFETY: hook-only fixed-size own-data description; success initializes
        // the output, and accessor/prototype fields are never evaluated.
        let description = if unsafe {
            ffi::jbq_debug_exception_description(ptr, description.as_mut_ptr())
        } == 1
        {
            // SAFETY: success fully initialized the fixed-size native output.
            let description = unsafe { description.assume_init() };
            JsString::from_units(description.text[..description.length].to_vec())
        } else {
            value.preview.clone()
        };
        let limit = self.port()?.limits().reply_bytes;
        let mut bytes = std::mem::size_of::<DebugReply>()
            .checked_add(value.preview.capacity_bytes())
            .and_then(|n| n.checked_add(value.unavailable.as_ref().map_or(0, String::capacity)))
            .and_then(|n| n.checked_add(description.capacity_bytes()))
            .ok_or(DebugError::Capacity)?;
        if bytes > limit {
            return Err(DebugError::Capacity);
        }
        // SAFETY: the original exception hook holds the owner's live stack.
        let count = unsafe { ffi::jbq_debug_frame_count(ptr) };
        if count < 0 {
            return Err(DebugError::Capacity);
        }
        let count = count as usize;
        let array_bytes = count
            .checked_mul(std::mem::size_of::<ExceptionFrame>())
            .ok_or(DebugError::Capacity)?;
        bytes = bytes.checked_add(array_bytes).ok_or(DebugError::Capacity)?;
        if bytes > limit {
            return Err(DebugError::Capacity);
        }
        let mut stack = Vec::new();
        stack
            .try_reserve_exact(count)
            .map_err(|_| DebugError::Capacity)?;
        // Charge actual vector backing capacity before copying diagnostic labels.
        bytes = bytes - array_bytes + stack.capacity() * std::mem::size_of::<ExceptionFrame>();
        if bytes > limit {
            return Err(DebugError::Capacity);
        }
        for level in 0..count {
            let (frame, name) =
                self.frame_bounded(ptr, level as i32, ((limit - bytes) / 2).min(4096))?;
            bytes = bytes
                .checked_add(name.capacity_bytes())
                .ok_or(DebugError::Capacity)?;
            if bytes > limit {
                return Err(DebugError::Capacity);
            }
            stack.push(ExceptionFrame {
                name,
                generated: self.location(&frame),
                original: None,
            });
        }
        Ok(DebugReply::ExceptionInfo(ExceptionInfo {
            location: stack.first().and_then(|frame| frame.generated.clone()),
            value,
            caught: caught != 0,
            stack,
            description,
        }))
    }
    fn breakpoint_locations(
        &self,
        selector: &ScriptSelector,
        start: SourcePosition,
        end: SourcePosition,
    ) -> Result<DebugReply, DebugError> {
        if start > end {
            return Err(DebugError::InvalidSource);
        }
        let port = self.port()?;
        let Some(registration) = port.select_script(selector)? else {
            return self.original_breakpoint_locations(selector, start, end);
        };
        let limits = port.limits();
        let maximum = limits
            .reply_bytes
            .saturating_sub(std::mem::size_of::<DebugReply>())
            / std::mem::size_of::<DebugLocation>();
        let state = self.state.borrow();
        let mut scanned = 0usize;
        // Keep a sorted unique reply directly, with checked exact reservations.
        // No uncharged staging collection or truncated successful result.
        let mut result: Vec<DebugLocation> = Vec::new();
        for source in &state.sources {
            scanned = scanned.checked_add(1).ok_or(DebugError::Capacity)?;
            if scanned > limits.location_scan_slots {
                return Err(DebugError::Capacity);
            }
            if source.registration != registration {
                continue;
            }
            for marker in &source.markers {
                scanned = scanned.checked_add(1).ok_or(DebugError::Capacity)?;
                if scanned > limits.location_scan_slots {
                    return Err(DebugError::Capacity);
                }
                let position = SourcePosition {
                    line: marker.line,
                    column: marker.column,
                };
                if position < start || position > end {
                    continue;
                }
                let comparisons = result
                    .len()
                    .checked_ilog2()
                    .map_or(0, |bits| bits as usize + 2);
                scanned = scanned
                    .checked_add(comparisons)
                    .ok_or(DebugError::Capacity)?;
                if scanned > limits.location_scan_slots {
                    return Err(DebugError::Capacity);
                }
                let index =
                    match result.binary_search_by_key(&position, |location| SourcePosition {
                        line: location.line,
                        column: location.column.unwrap(),
                    }) {
                        Ok(_) => continue,
                        Err(index) => index,
                    };
                if result.len() >= maximum {
                    return Err(DebugError::Capacity);
                }
                scanned = scanned
                    .checked_add(result.len() - index)
                    .ok_or(DebugError::Capacity)?;
                if scanned > limits.location_scan_slots {
                    return Err(DebugError::Capacity);
                }
                result
                    .try_reserve_exact(1)
                    .map_err(|_| DebugError::Capacity)?;
                result.insert(
                    index,
                    DebugLocation {
                        script: registration.id,
                        revision: registration.revision,
                        line: marker.line,
                        column: Some(marker.column),
                    },
                );
            }
        }
        Ok(DebugReply::Locations(result))
    }
    fn original_breakpoint_locations(
        &self,
        selector: &ScriptSelector,
        start: SourcePosition,
        end: SourcePosition,
    ) -> Result<DebugReply, DebugError> {
        let port = self.port()?;
        let limits = port.limits();
        let state = self.state.borrow();
        let mut scanned = 0usize;
        let mut bytes = std::mem::size_of::<DebugReply>();
        let mut result: Vec<OriginalLocation> = Vec::new();
        for source in &state.sources {
            scanned = scanned.checked_add(1).ok_or(DebugError::Capacity)?;
            if scanned > limits.location_scan_slots {
                return Err(DebugError::Capacity);
            }
            for marker in &source.markers {
                scanned = scanned.checked_add(1).ok_or(DebugError::Capacity)?;
                if scanned > limits.location_scan_slots {
                    return Err(DebugError::Capacity);
                }
                let location = DebugLocation {
                    script: source.registration.id,
                    revision: source.registration.revision,
                    line: marker.line,
                    column: Some(marker.column),
                };
                for original in port.original_locations(selector, &location)? {
                    let position = SourcePosition {
                        line: original.line,
                        column: original.column,
                    };
                    scanned = scanned.checked_add(1).ok_or(DebugError::Capacity)?;
                    if scanned > limits.location_scan_slots {
                        return Err(DebugError::Capacity);
                    }
                    if position < start || position > end {
                        continue;
                    }
                    scanned = scanned
                        .checked_add(result.len().checked_ilog2().map_or(0, |n| n as usize + 2))
                        .ok_or(DebugError::Capacity)?;
                    if scanned > limits.location_scan_slots {
                        return Err(DebugError::Capacity);
                    }
                    let index = match result.binary_search_by_key(&position, |at| SourcePosition {
                        line: at.line,
                        column: at.column,
                    }) {
                        Ok(_) => continue,
                        Err(index) => index,
                    };
                    scanned = scanned
                        .checked_add(result.len() - index)
                        .ok_or(DebugError::Capacity)?;
                    if scanned > limits.location_scan_slots {
                        return Err(DebugError::Capacity);
                    }
                    let next_bytes = bytes
                        .checked_add(original.origin.capacity())
                        .ok_or(DebugError::Capacity)?;
                    if next_bytes
                        .checked_add((result.len() + 1) * std::mem::size_of::<OriginalLocation>())
                        .is_none_or(|n| n > limits.reply_bytes)
                    {
                        return Err(DebugError::Capacity);
                    }
                    result
                        .try_reserve_exact(1)
                        .map_err(|_| DebugError::Capacity)?;
                    if next_bytes
                        .checked_add(result.capacity() * std::mem::size_of::<OriginalLocation>())
                        .is_none_or(|n| n > limits.reply_bytes)
                    {
                        return Err(DebugError::Capacity);
                    }
                    bytes = next_bytes;
                    result.insert(index, original);
                }
            }
        }
        if result.is_empty() {
            Ok(DebugReply::Locations(Vec::new()))
        } else {
            Ok(DebugReply::OriginalLocations(result))
        }
    }
    fn request(
        &self,
        ptr: *mut ffi::Context,
        id: DebugRequestId,
        request: DebugRequest,
        pause: Option<PauseId>,
    ) -> Result<DebugReply, DebugError> {
        match request {
            DebugRequest::UnhandledRejections(enabled) => {
                self.state.borrow_mut().rejection_filter = enabled;
                Ok(DebugReply::UnhandledRejections)
            }
            DebugRequest::PromiseRejections { page } => {
                let (serials, total, dropped, retained_bytes) = {
                    let state = self.state.borrow();
                    let store = state.rejections.as_ref().ok_or(DebugError::Unsupported)?;
                    let count = page.count.min(self.port()?.limits().inspection_page_items);
                    let serials = store
                        .records()
                        .skip(page.start)
                        .take(count)
                        .map(|r| r.serial)
                        .collect::<Vec<_>>();
                    (
                        serials,
                        store.count(),
                        store.dropped().saturating_add(state.rejection_dropped),
                        store.retained_bytes()
                            + state.rejection_origins.capacity()
                                * std::mem::size_of::<(AsyncStackId, Option<DebugLocation>)>(),
                    )
                };
                let mut records = Vec::new();
                records
                    .try_reserve_exact(serials.len())
                    .map_err(|_| DebugError::Capacity)?;
                for serial in serials {
                    records.push(self.rejection_info(ptr, serial)?);
                }
                Ok(DebugReply::PromiseRejections {
                    records,
                    total,
                    dropped,
                    retained_bytes,
                })
            }
            DebugRequest::StepPolicy(policy) => {
                self.state.borrow_mut().step_policy = policy;
                for source in &mut self.state.borrow_mut().sources {
                    for marker in &mut source.markers {
                        marker.skip_step = None;
                    }
                }
                Ok(DebugReply::StepPolicy)
            }
            DebugRequest::SetBreakpointEnabled {
                selector,
                client_id,
                enabled,
            } => self.set_enabled(ptr, selector, client_id, enabled),
            DebugRequest::ContinueTo {
                pause: target,
                location,
            } => {
                self.current(pause, target)?;
                self.arm_continue_to(ptr, location)
            }
            DebugRequest::AsyncStack { pause: target } => {
                self.current(pause, target)?;
                self.async_trace(ptr)
            }
            DebugRequest::ExceptionInfo { pause: target } => {
                self.exception_info(ptr, target, pause)
            }
            DebugRequest::BreakpointLocations {
                selector,
                start,
                end,
            } => self.breakpoint_locations(&selector, start, end),
            DebugRequest::SetBreakpoints {
                selector,
                breakpoints,
            } => self.set_breakpoints(ptr, selector, breakpoints),
            DebugRequest::ExceptionPolicy(policy) => {
                self.state.borrow_mut().pause_uncaught = policy == ExceptionPolicy::Uncaught;
                // SAFETY: ptr is live on its owner thread and the policy is one of the native enum values.
                unsafe {
                    ffi::jbq_debug_exception_policy(
                        ptr,
                        match policy {
                            ExceptionPolicy::None => 0,
                            ExceptionPolicy::Uncaught => 1,
                            ExceptionPolicy::All => 2,
                        },
                    )
                };
                Ok(DebugReply::ExceptionPolicy(policy))
            }
            DebugRequest::Stack {
                pause: target,
                page,
            } => {
                let pause = self.current(pause, target)?;
                // SAFETY: the validated pause keeps the owner-thread native stack available for inspection.
                let total = unsafe { ffi::jbq_debug_frame_count(ptr) }.max(0) as usize;
                let range = page_range(page, total);
                let mut frames = Vec::new();
                frames
                    .try_reserve_exact(range.len())
                    .map_err(|_| DebugError::Capacity)?;
                for index in range {
                    let DebugReference::Frame(id) = self.port()?.identities().register(
                        pause,
                        ReferenceKind::Frame,
                        key(index as u64),
                    )?
                    else {
                        return Err(DebugError::InvalidReference);
                    };
                    let (f, name) = self.frame(ptr, index as i32)?;
                    frames.push(DebugFrame {
                        id,
                        name,
                        generated: self.location(&f),
                        original: None,
                    });
                }
                Ok(DebugReply::Stack { frames, total })
            }
            DebugRequest::Scopes { frame } => {
                let pause = self.current(pause, frame.pause)?;
                let index = slot(
                    self.port()?
                        .identities()
                        .resolve(DebugReference::Frame(frame))?,
                ) as i32;
                let (f, _) = self.frame(ptr, index)?;
                if f.scope_known == 0 {
                    return Err(DebugError::Unsupported);
                }
                let mut output = ScopeOutput {
                    depths: BTreeSet::new(),
                    full: false,
                };
                // SAFETY: selected frame remains paused; visitor records only bounded
                // depth metadata, never allocates names or retains a variable snapshot.
                let result = unsafe {
                    ffi::jbq_debug_variables(
                        ptr,
                        index,
                        collect_scope,
                        (&mut output as *mut ScopeOutput).cast(),
                    )
                };
                if result < 0 {
                    return Err(DebugError::Unsupported);
                }
                if output.full {
                    return Err(DebugError::Capacity);
                }
                let mut depths = output.depths;
                if depths.is_empty() {
                    depths.insert(0);
                }
                let has_lexical = depths.iter().any(|depth| *depth < FUNCTION_SCOPE);
                let mut scopes = Vec::new();
                for depth in depths {
                    let DebugReference::Scope(id) = self.port()?.identities().register(
                        pause,
                        ReferenceKind::Scope,
                        key(((index as u64) << 32) | (depth as u64)),
                    )?
                    else {
                        return Err(DebugError::InvalidReference);
                    };
                    scopes.push(DebugScope {
                        id,
                        name: JsString::from_utf8(
                            if depth == 0 || (depth == FUNCTION_SCOPE && !has_lexical) {
                                "Local"
                            } else if depth == FUNCTION_SCOPE {
                                "Function"
                            } else if depth & CLOSURE_SCOPE != 0 {
                                "Closure"
                            } else {
                                "Block"
                            },
                        ),
                        expensive: false,
                    });
                }
                Ok(DebugReply::Scopes(scopes))
            }
            DebugRequest::ReleaseCursor(cursor) => {
                self.current(pause, cursor.pause)?;
                let port = self.port()?;
                let k = slot(port.identities().resolve(DebugReference::Cursor(cursor))?);
                self.state
                    .borrow_mut()
                    .cursors
                    .remove(&k)
                    .ok_or(DebugError::StaleInspection)?;
                port.identities().release_cursor(cursor)?;
                Ok(DebugReply::CursorReleased)
            }
            DebugRequest::LookupPath { frame, path } => {
                // Reject syntax before identity leases, FFI or compiler access.
                let path = LookupPath::parse(&path)?;
                let pause = self.current(pause, frame.pause)?;
                let index = slot(
                    self.port()?
                        .identities()
                        .resolve(DebugReference::Frame(frame))?,
                ) as i32;
                let (root_this, root) = match &path.root {
                    LookupRoot::This => (1, &[][..]),
                    LookupRoot::Identifier(name) => (0, name.as_units()),
                };
                let keys: Vec<_> = path
                    .steps
                    .iter()
                    .map(|step| match step {
                        LookupStep::Named(name) => ffi::LookupKey {
                            text: name.as_units().as_ptr(),
                            units: name.as_units().len(),
                            index: 0,
                            is_index: 0,
                        },
                        LookupStep::Index(index) => ffi::LookupKey {
                            text: std::ptr::null(),
                            units: 0,
                            index: *index,
                            is_index: 1,
                        },
                    })
                    .collect();
                let mut value = std::mem::MaybeUninit::uninit();
                // SAFETY: verified frame remains paused on its owner; parsed keys
                // and initialized output live throughout the synchronous raw read.
                #[cfg(test)]
                self.lookup_observation
                    .native_entries
                    .fetch_add(1, Ordering::SeqCst);
                // SAFETY: same synchronous parsed keys and checked frame as above.
                let status = unsafe {
                    ffi::jbq_debug_lookup(
                        ptr,
                        index,
                        root_this,
                        root.as_ptr(),
                        root.len(),
                        keys.as_ptr(),
                        keys.len(),
                        value.as_mut_ptr(),
                    )
                };
                // SAFETY: bridge initializes output on every status.
                let value = unsafe { value.assume_init() };
                if status == 0 {
                    Ok(DebugReply::Lookup(self.preview(
                        ptr,
                        pause,
                        owned(value, &self.raw)?,
                    )))
                } else {
                    // SAFETY: bridge transfers an owned undefined on refusal.
                    unsafe { ffi::jbq_free(ptr, value) };
                    match status {
                        -2 => Err(DebugError::Capacity),
                        1 => Ok(DebugReply::Lookup(unavailable(
                            "own binding or property is absent",
                        ))),
                        _ => Ok(DebugReply::Lookup(unavailable(
                            "safe lookup is unavailable",
                        ))),
                    }
                }
            }
            DebugRequest::Inspect(request) => self.inspect(ptr, request, pause),
            DebugRequest::Variables { parent, page } => match parent {
                VariableParent::Scope(scope) => {
                    let pause = self.current(pause, scope.pause)?;
                    let reference = slot(
                        self.port()?
                            .identities()
                            .resolve(DebugReference::Scope(scope))?,
                    );
                    let (index, depth) = ((reference >> 32) as i32, reference as u32);
                    let all: Vec<_> = self
                        .variables(ptr, index)?
                        .into_iter()
                        .filter(|v| variable_scope(v.2, v.3) == depth)
                        .collect();
                    let total = all.len();
                    let mut variables = Vec::new();
                    for i in page_range(page, total) {
                        let (name, value, _, _) = &all[i];
                        variables.push(DebugVariable {
                            name: name.clone(),
                            // SAFETY: the retained variable value is live; this checks only its native tag.
                            value: if unsafe { ffi::jbq_debug_uninitialized(value.value) } != 0 {
                                unavailable("binding is uninitialized")
                            } else {
                                self.preview(ptr, pause, value.clone())
                            },
                        });
                    }
                    Ok(DebugReply::Variables { variables, total })
                }
                VariableParent::Object(object) => {
                    let pause = self.current(pause, object.pause)?;
                    let index = slot(
                        self.port()?
                            .identities()
                            .resolve(DebugReference::Object(object))?,
                    ) as usize;
                    let value = self
                        .state
                        .borrow()
                        .objects
                        .get(index)
                        .cloned()
                        .ok_or(DebugError::InvalidReference)?;
                    self.properties(ptr, pause, &value, page)
                }
            },
            request
            @ (DebugRequest::Evaluate { .. } | DebugRequest::EvaluateWithOptions { .. }) => {
                let (frame, expression, allow_side_effects, explicit) = match request {
                    DebugRequest::Evaluate {
                        frame,
                        expression,
                        allow_side_effects,
                    } => (frame, expression, allow_side_effects, false),
                    DebugRequest::EvaluateWithOptions {
                        frame,
                        expression,
                        options,
                    } => (frame, expression, options.allow_side_effects, true),
                    _ => unreachable!(),
                };
                let pause = self.current(pause, frame.pause)?;
                if !allow_side_effects {
                    return Err(DebugError::Unsupported);
                }
                let port = self.port()?;
                let index = slot(port.identities().resolve(DebugReference::Frame(frame))?) as i32;
                let control = port
                    .evaluation_control(id)
                    .ok_or(DebugError::InvalidRequest)?;
                let (value, status) = self.evaluate_controlled(ptr, index, &expression, control)?;
                if explicit {
                    Ok(DebugReply::EvaluationOutcome(EvaluationOutcome {
                        status,
                        value: value.map(|value| self.preview(ptr, pause, value)),
                        effects_may_remain: true,
                    }))
                } else {
                    match status {
                        EvaluationStatus::Cancelled => Err(DebugError::EvaluationCancelled {
                            effects_may_remain: true,
                        }),
                        EvaluationStatus::DeadlineExceeded => {
                            Err(DebugError::EvaluationDeadlineExceeded {
                                effects_may_remain: true,
                            })
                        }
                        _ => Ok(DebugReply::Evaluation {
                            value: self.preview(
                                ptr,
                                pause,
                                value.ok_or(DebugError::InvalidRequest)?,
                            ),
                            exception: status == EvaluationStatus::Threw,
                        }),
                    }
                }
            }
            DebugRequest::ReleaseWriteTargets(targets) => self.release_write_targets(targets),
            DebugRequest::Write {
                target,
                frame,
                expression,
            } => self.write_target(ptr, id, target, frame, &expression, pause),
            _ => Err(DebugError::Unsupported),
        }
    }
    fn variables(
        &self,
        ptr: *mut ffi::Context,
        index: i32,
    ) -> Result<Vec<FrameVariable>, DebugError> {
        let mut output = VariableOutput {
            raw: &self.raw,
            ptr,
            values: Vec::new(),
            error: None,
        };
        // SAFETY: ptr/frame are live on the owner thread; the synchronous callback exclusively borrows output.
        let result = unsafe {
            ffi::jbq_debug_variables(
                ptr,
                index,
                collect_variable,
                (&mut output as *mut VariableOutput).cast(),
            )
        };
        if let Some(error) = output.error {
            return Err(error);
        }
        if result < 0 {
            return Err(DebugError::Unsupported);
        }
        Ok(output.values)
    }
    fn properties(
        &self,
        ptr: *mut ffi::Context,
        pause: PauseId,
        value: &NativeValue,
        page: Page,
    ) -> Result<DebugReply, DebugError> {
        let mut output = PropertyOutput {
            debug: self,
            pause,
            variables: Vec::new(),
            error: None,
        };
        let mut total = 0;
        // SAFETY: value belongs to this live context; output/total remain writable throughout the synchronous visitor.
        let result = unsafe {
            ffi::jbq_debug_properties(
                ptr,
                value.value,
                page.start,
                page.count.min(PAGE_LIMIT),
                collect_property,
                (&mut output as *mut PropertyOutput).cast(),
                &mut total,
            )
        };
        if result != 0 {
            if result == -2 {
                return Err(DebugError::Capacity);
            }
            // SAFETY: take_exception transfers this context's pending owned value for immediate release.
            unsafe { ffi::jbq_free(ptr, ffi::jbq_take_exception(ptr)) };
            return Err(DebugError::Unsupported);
        }
        if let Some(error) = output.error {
            return Err(error);
        }
        Ok(DebugReply::Variables {
            variables: output.variables,
            total,
        })
    }
    fn raw_info(&self, ptr: *mut ffi::Context, value: ffi::Value) -> Option<ffi::DebugValueInfo> {
        let mut info = std::mem::MaybeUninit::<ffi::DebugValueInfo>::uninit();
        // SAFETY: live owner-thread value and fixed-size initialized native output; no callbacks.
        if unsafe { ffi::jbq_debug_value_info_get(ptr, value, info.as_mut_ptr()) } == 1 {
            // SAFETY: success initializes every byte of the structure.
            Some(unsafe { info.assume_init() })
        } else {
            None
        }
    }
    fn preview(
        &self,
        ptr: *mut ffi::Context,
        pause: PauseId,
        value: Rc<NativeValue>,
    ) -> DebugValue {
        let plain = |text: JsString| DebugValue {
            preview: text,
            object: None,
            accessor: false,
            unavailable: None,
        };
        let text = |s: &str| plain(JsString::from_utf8(s));
        // SAFETY: value is retained in the live owner-thread context; kind inspection does not execute code.
        match unsafe { ffi::jbq_kind(ptr, value.value) } {
            0 => text("undefined"),
            1 => text("null"),
            // SAFETY: the matched native kind is boolean and value belongs to this live context.
            2 => text(if unsafe { ffi::jbq_to_bool(ptr, value.value) } != 0 {
                "true"
            } else {
                "false"
            }),
            3 => {
                let mut n = 0.;
                // SAFETY: the matched value is a number; n is a valid output and no user conversion is needed.
                unsafe { ffi::jbq_to_number(ptr, value.value, &mut n) };
                text(&js_number(n))
            }
            4 => {
                let preview = match string(ptr, value.value, Some(256)) {
                    Ok(preview) => preview,
                    Err(_) => return unavailable("string preview could not be copied"),
                };
                let mut units = vec![34];
                units.extend_from_slice(preview.as_units());
                units.push(34);
                plain(JsString::from_units(units))
            }
            5 | 6 => self.raw_info(ptr, value.value).map_or_else(
                || unavailable("raw primitive preview unavailable"),
                |info| plain(JsString::from_units(info.text[..info.length].to_vec())),
            ),
            _ => {
                // SAFETY: value is a retained object in this context; class inspection invokes no user code.
                let class = unsafe { ffi::jbq_debug_object_class(ptr, value.value) };
                let info = self.raw_info(ptr, value.value);
                let label = if class == 4 {
                    JsString::from_utf8("Proxy")
                } else {
                    info.as_ref().map_or_else(
                        || JsString::from_utf8("Object"),
                        |info| JsString::from_units(info.text[..info.length].to_vec()),
                    )
                };
                if info.as_ref().is_none_or(|info| info.expandable == 0) || matches!(class, 4 | 5) {
                    return DebugValue {
                        unavailable: Some(
                            "opaque object cannot be inspected without executing code".into(),
                        ),
                        ..plain(label)
                    };
                }
                let existing = self
                    .state
                    .borrow()
                    .objects
                    .iter()
                    .position(|v| v.value.words == value.value.words);
                let index = existing.unwrap_or_else(|| self.state.borrow().objects.len());
                let reference = self.port().and_then(|p| {
                    p.identities()
                        .register(pause, ReferenceKind::Object, key(index as u64))
                });
                match reference {
                    Ok(DebugReference::Object(object)) => {
                        if existing.is_none() {
                            self.state.borrow_mut().objects.push(value);
                        }
                        DebugValue {
                            object: Some(object),
                            ..plain(label)
                        }
                    }
                    _ => DebugValue {
                        unavailable: Some("inspection object limit reached".into()),
                        ..plain(label)
                    },
                }
            }
        }
    }
}
impl DebugBackend for QuickJsDebug {
    fn descriptor(&self) -> &DebugDescriptor {
        &self.descriptor
    }
    fn initialize(&self, port: Rc<dyn DebugPort>) -> Result<(), DebugError> {
        let ptr = self.ptr()?;
        if self.port.borrow().is_some() {
            return Err(DebugError::InFlight);
        }
        self.state.borrow_mut().history = Some(AsyncStackStore::with_limits(
            self.context,
            port.limits().async_history_depth,
            port.limits().async_history_records,
            port.limits().async_history_bytes,
        )?);
        self.state.borrow_mut().rejections =
            Some(rejections::Store::new(port.limits().promise_rejections)?);
        {
            let mut state = self.state.borrow_mut();
            state
                .rejection_origins
                .try_reserve_exact(port.limits().promise_rejections)
                .map_err(|_| DebugError::Capacity)?;
            state.rejection_origin_limit = port.limits().promise_rejections;
        }
        // SAFETY: this Rc-backed debugger stays stable until close removes both owner-thread hooks.
        unsafe {
            ffi::jbq_debug_install(ptr, Some(hook), (self as *const Self).cast_mut().cast());
            ffi::jbq_debug_rejection_install(
                ptr,
                Some(rejection_hook),
                (self as *const Self).cast_mut().cast(),
            );
            ffi::jbq_debug_async_install(
                ptr,
                Some(async_hook),
                (self as *const Self).cast_mut().cast(),
            );
        };
        *self.port.borrow_mut() = Some(port);
        Ok(())
    }
    fn checkpoint(&self, drained: bool) -> Result<(), DebugError> {
        if self.session.get().is_none() {
            return Ok(());
        }
        let ptr = self.ptr()?;
        let transitions = self
            .state
            .borrow_mut()
            .rejections
            .as_mut()
            .ok_or(DebugError::Unsupported)?
            .checkpoint(drained)?;
        for (serial, _) in transitions {
            let info = self.rejection_info(ptr, serial)?;
            match self.port()?.publish(DebugUpdate::PromiseRejection(info)) {
                Ok(()) | Err(DebugError::Closed | DebugError::NotAttached) => {}
                Err(_) => {
                    self.port()?.publish(DebugUpdate::Diagnostic {
                        report: FailureReport::new(
                            FailureClass::Script,
                            FailureCode::ScriptException,
                            "rejection notification omitted within debugger update limits",
                        ),
                        dropped: 1,
                    })?;
                }
            }
        }
        loop {
            let serial = {
                let mut state = self.state.borrow_mut();
                if !(state.rejection_filter || state.pause_uncaught) || self.session.get().is_none()
                {
                    break;
                }
                let store = state.rejections.as_mut().ok_or(DebugError::Unsupported)?;
                let serial = store
                    .records()
                    .find(|r| r.status == rejections::Status::Unhandled && !r.pause_reported)
                    .map(|r| r.serial);
                if let Some(serial) = serial {
                    store.mark_pause_reported(serial);
                }
                serial
            };
            let Some(serial) = serial else {
                break;
            };
            self.pause_rejection(ptr, serial)?;
        }
        Ok(())
    }
    fn capture_async(&self, kind: AsyncScheduleKind) -> Option<AsyncStackId> {
        let ptr = self.ptr().ok()?;
        // SAFETY: owner-local metadata capture never executes application code.
        let serial = unsafe { ffi::jbq_debug_async_capture(ptr, kind as i32) };
        self.state
            .borrow()
            .history
            .as_ref()?
            .resolve_serial(serial)
            .ok()
    }
    fn async_parent(&self) -> Option<AsyncStackId> {
        let ptr = self.ptr().ok()?;
        // SAFETY: reads only the live owner's integer scheduling slot.
        let serial = unsafe { ffi::jbq_debug_async_stack(ptr) };
        self.state
            .borrow()
            .history
            .as_ref()?
            .resolve_serial(serial)
            .ok()
    }
    fn set_async_parent(&self, parent: Option<AsyncStackId>) {
        let Ok(ptr) = self.ptr() else {
            return;
        };
        let serial = parent
            .filter(|id| {
                self.state
                    .borrow()
                    .history
                    .as_ref()
                    .is_some_and(|history| history.resolve_serial(id.serial()) == Ok(*id))
            })
            .map_or(0, AsyncStackId::serial);
        // SAFETY: scalar metadata swap on its owner; no JS values or callbacks are involved.
        unsafe { ffi::jbq_debug_async_set(ptr, serial) };
    }
    fn attach(&self, options: DebugAttachOptions) -> Result<Arc<dyn DebugSignal>, DebugError> {
        let ptr = self.ptr()?;
        self.port()?;
        if self.session.get().is_some() {
            return Err(DebugError::InFlight);
        }
        let signal = Arc::new(Signal {
            wake: AtomicBool::new(false),
            live: AtomicBool::new(true),
        });
        *self.signal.borrow_mut() = Some(signal.clone());
        self.session.set(Some(options.session));
        self.state.borrow_mut().entry = options.break_on_start;
        // SAFETY: ptr is a live owner-thread context and 3 enables the supported native debug flags.
        unsafe { ffi::jbq_debug_flags(ptr, 3) };
        Ok(signal)
    }
    fn service(&self) -> Result<DebugServiceReport, DebugError> {
        let ptr = self.ptr()?;
        let port = self.port()?;
        let mut made_progress = false;
        while let Some(d) = port.next_directive(false) {
            made_progress = true;
            match self.dispatch(ptr, d, None)? {
                Flow::Pause => self.state.borrow_mut().pause = true,
                Flow::Exit(1) => {
                    // SAFETY: stop targets the live context on its owner thread and returns an owned value.
                    let value = unsafe { ffi::jbq_debug_stop(ptr) };
                    // SAFETY: value was just returned by debug_stop in this same live context.
                    unsafe { ffi::jbq_free(ptr, value) }
                }
                _ => {}
            }
        }
        Ok(DebugServiceReport {
            made_progress,
            state: port.identities().state(),
        })
    }
    fn detach(&self) -> Result<(), DebugError> {
        self.ptr()?;
        self.end_session()
    }
}
unsafe extern "C" fn hook(opaque: *mut c_void, ptr: *mut ffi::Context, event: i32) -> i32 {
    // SAFETY: initialize installs a stable QuickJsDebug pointer and close removes it before release.
    let debug = unsafe { &*opaque.cast::<QuickJsDebug>() };
    if event == -1 {
        // Owner-thread interrupt poll, including while debugger evaluation runs.
        // Never inspect frames or execute queued/application work here.
        return contain(|| {
            if debug
                .port
                .borrow()
                .as_ref()
                .is_some_and(|p| p.is_stopping())
            {
                return 1;
            }
            match debug
                .evaluation
                .borrow()
                .as_ref()
                .and_then(|c| c.poll_at(std::time::Instant::now()))
            {
                Some(EvaluationStatus::Cancelled) => 2,
                Some(EvaluationStatus::DeadlineExceeded) => 3,
                _ => 0,
            }
        })
        .unwrap_or(1);
    }
    if debug.in_pause.get() {
        return 0;
    }
    let result = contain(|| {
        debug.in_pause.set(true);
        let result = debug.on_hook(ptr, event);
        debug.in_pause.set(false);
        result
    });
    debug.in_pause.set(false);
    match result {
        Ok(Ok(code)) => code,
        other => {
            if let Some(raw) = debug.raw.upgrade() {
                raw.record_fault(FailureReport::fault(
                    FailureCode::EngineFailure,
                    &format!(
                        "QuickJS debugger callback failed: {}",
                        match other {
                            Ok(Err(e)) => e.to_string(),
                            _ => "panic".into(),
                        }
                    ),
                ));
            }
            1
        }
    }
}
unsafe extern "C" fn rejection_hook(
    opaque: *mut c_void,
    ptr: *mut ffi::Context,
    promise: ffi::Value,
    reason: ffi::Value,
    handled: i32,
    origin: u64,
) {
    // SAFETY: initialize installs a stable owner-local Rc pointee; close revokes it first.
    let debug = unsafe { &*opaque.cast::<QuickJsDebug>() };
    let _ = contain(|| debug.observe_rejection(ptr, promise, reason, handled, origin));
}
unsafe extern "C" fn async_hook(
    opaque: *mut c_void,
    ptr: *mut ffi::Context,
    kind: i32,
    parent: u64,
) -> u64 {
    // SAFETY: initialization installs this stable owner-local Rc pointee; close removes it first.
    let debug = unsafe { &*opaque.cast::<QuickJsDebug>() };
    // Metadata capture failures never replace an application's pending exception.
    contain(|| debug.capture_history(ptr, kind, parent)).unwrap_or(0)
}
unsafe extern "C" fn collect_marker(
    opaque: *mut c_void,
    function: usize,
    marker: u32,
    line: i32,
    column: i32,
    _debugger: i32,
) {
    // SAFETY: loaded passes a live, exclusively borrowed MarkerOutput to this synchronous visitor.
    let output = unsafe { &mut *opaque.cast::<MarkerOutput>() };
    if output.failed || line <= 0 || column <= 0 {
        // Missing native position metadata stays unknown, never a guessed (0,0).
        return;
    }
    let result = contain(|| {
        output
            .markers
            .try_reserve(1)
            .map_err(|_| DebugError::Capacity)?;
        output.markers.push(Marker {
            skip_step: None,
            function,
            marker,
            line: line as u32 - 1,
            column: column as u32 - 1,
        });
        Ok::<_, DebugError>(())
    });
    output.failed = !matches!(result, Ok(Ok(())));
}
struct MarkerOutput {
    markers: Vec<Marker>,
    failed: bool,
}
const FUNCTION_SCOPE: u32 = 1 << 30;
const CLOSURE_SCOPE: u32 = 1 << 31;
fn variable_scope(kind: i32, depth: i32) -> u32 {
    if kind == 3 {
        CLOSURE_SCOPE | depth.max(0) as u32
    } else if kind == 0 || kind == 4 {
        depth.max(0) as u32
    } else {
        FUNCTION_SCOPE
    }
}
struct ScopeOutput {
    depths: BTreeSet<u32>,
    full: bool,
}
unsafe extern "C" fn collect_scope(
    opaque: *mut c_void,
    _name: ffi::Value,
    _value: ffi::Value,
    kind: i32,
    depth: i32,
) {
    // SAFETY: Scopes lends this live exclusive output for the synchronous visitor.
    let output = unsafe { &mut *opaque.cast::<ScopeOutput>() };
    if output.full || kind == 4 {
        return;
    }
    let depth = variable_scope(kind, depth);
    if output.depths.len() >= PAGE_LIMIT && !output.depths.contains(&depth) {
        output.full = true;
    } else {
        output.depths.insert(depth);
    }
}
struct VariableOutput<'a> {
    raw: &'a Weak<RawContext>,
    ptr: *mut ffi::Context,
    values: Vec<FrameVariable>,
    error: Option<DebugError>,
}
unsafe extern "C" fn collect_variable(
    opaque: *mut c_void,
    name: ffi::Value,
    value: ffi::Value,
    kind: i32,
    depth: i32,
) {
    // SAFETY: variables lends its live output exclusively for this synchronous native visitor.
    let output = unsafe { &mut *opaque.cast::<VariableOutput<'_>>() };
    if output.error.is_some() {
        return;
    }
    if output.values.len() >= 65536 {
        output.error = Some(DebugError::Capacity);
        return;
    }
    let result = contain(|| {
        let name = string(output.ptr, name, None)?;
        output
            .values
            .try_reserve(1)
            .map_err(|_| DebugError::Capacity)?;
        // SAFETY: the visitor lends a value from output.ptr; duplicate it before the native borrow ends.
        let value = owned(unsafe { ffi::jbq_dup(output.ptr, value) }, output.raw)?;
        output.values.push((name, value, kind, depth));
        Ok::<_, DebugError>(())
    });
    output.error = match result {
        Ok(result) => result.err(),
        Err(_) => Some(DebugError::Unsupported),
    };
}
struct PropertyOutput<'a> {
    debug: &'a QuickJsDebug,
    pause: PauseId,
    variables: Vec<DebugVariable>,
    error: Option<DebugError>,
}
unsafe extern "C" fn collect_property(
    opaque: *mut c_void,
    ptr: *mut ffi::Context,
    name: ffi::Value,
    value: ffi::Value,
    flags: i32,
) {
    // SAFETY: properties passes its exclusively borrowed output for this synchronous callback.
    let output = unsafe { &mut *opaque.cast::<PropertyOutput<'_>>() };
    if output.error.is_some() {
        return;
    }
    let result = contain(|| {
        output
            .variables
            .try_reserve(1)
            .map_err(|_| DebugError::Capacity)?;
        let mut name = string(ptr, name, None)?;
        if flags & 2 != 0 {
            let mut units = JsString::from_utf8("[Symbol(").into_units();
            units.extend_from_slice(name.as_units());
            units.extend(")]".encode_utf16());
            name = JsString::from_units(units);
        }
        let value = if flags & 1 != 0 {
            DebugValue {
                preview: JsString::from_utf8("(accessor)"),
                object: None,
                accessor: true,
                unavailable: Some("getter not invoked".into()),
            }
        } else {
            output.debug.preview(
                ptr,
                output.pause,
                // SAFETY: the native visitor lends a live value in this context; retain it before returning.
                owned(unsafe { ffi::jbq_dup(ptr, value) }, &output.debug.raw)?,
            )
        };
        output.variables.push(DebugVariable { name, value });
        Ok::<_, DebugError>(())
    });
    output.error = match result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(_) => Some(DebugError::Unsupported),
    };
}
fn owned(value: ffi::Value, raw: &Weak<RawContext>) -> Result<Rc<NativeValue>, DebugError> {
    Ok(Rc::new(NativeValue::new(
        value,
        raw.upgrade().ok_or(DebugError::Closed)?,
    )))
}
/// Names are lossless within an explicit capacity; only value previews truncate.
/// ECMAScript Number::toString(10); negative zero keeps its sign, as debuggers show it.
fn js_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n == 0. {
        return if n.is_sign_negative() { "-0" } else { "0" }.into();
    }
    if n.is_infinite() {
        return if n < 0. { "-Infinity" } else { "Infinity" }.into();
    }
    let sign = if n < 0. { "-" } else { "" };
    // Shortest round-trip digits and exponent, as Number::toString requires.
    let exponential = format!("{:e}", n.abs());
    let (mantissa, exponent) = exponential.split_once('e').unwrap_or((&exponential, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let point = exponent.parse::<i32>().unwrap_or(0) + 1;
    let body = if k <= point && point <= 21 {
        format!("{digits}{}", "0".repeat((point - k) as usize))
    } else if 0 < point && point <= 21 {
        let (int, frac) = digits.split_at(point as usize);
        format!("{int}.{frac}")
    } else if -6 < point && point <= 0 {
        format!("0.{}{digits}", "0".repeat(-point as usize))
    } else {
        let (first, rest) = digits.split_at(1);
        let dot = if rest.is_empty() { "" } else { "." };
        let e = if point > 0 { '+' } else { '-' };
        format!("{first}{dot}{rest}e{e}{}", (point - 1).abs())
    };
    format!("{sign}{body}")
}
fn string(
    ptr: *mut ffi::Context,
    value: ffi::Value,
    preview_limit: Option<usize>,
) -> Result<JsString, DebugError> {
    // SAFETY: callers provide a live value and its owner-thread context; kind checks do not execute code.
    if unsafe { ffi::jbq_kind(ptr, value) } != 4 {
        return Err(DebugError::InvalidReference);
    }
    let mut length = 0;
    // SAFETY: primitive string tag verified above; zero-capacity query dereferences no output.
    if unsafe { ffi::jbq_debug_copy_string(value, std::ptr::null_mut(), 0, &mut length) } < 0 {
        return Err(DebugError::InvalidReference);
    }
    if preview_limit.is_none() && length > 65_536 {
        return Err(DebugError::Capacity);
    }
    let limit = preview_limit.map_or(length, |limit| length.min(limit));
    let mut copied = Vec::new();
    copied
        .try_reserve_exact(limit + usize::from(length > limit))
        .map_err(|_| DebugError::Capacity)?;
    copied.resize(limit, 0);
    // SAFETY: primitive string and output is writable for precisely limit units.
    let result =
        unsafe { ffi::jbq_debug_copy_string(value, copied.as_mut_ptr(), limit, &mut length) };
    if result < 0 {
        return Err(if result == -2 {
            DebugError::Capacity
        } else {
            DebugError::InvalidReference
        });
    }
    if preview_limit.is_none() && length > limit {
        return Err(DebugError::Capacity);
    }
    copied.truncate(length.min(limit));
    if length > limit {
        copied.push(0x2026);
    }
    Ok(JsString::from_units(copied))
}
fn unavailable(reason: &str) -> DebugValue {
    DebugValue {
        preview: JsString::from_utf8("<unavailable>"),
        object: None,
        accessor: false,
        unavailable: Some(reason.into()),
    }
}
fn key(index: u64) -> NonZeroU64 {
    NonZeroU64::new(index + 1).expect("bounded index")
}
fn slot(key: NonZeroU64) -> u64 {
    key.get() - 1
}
fn page_range(page: Page, total: usize) -> std::ops::Range<usize> {
    let start = page.start.min(total);
    start..start.saturating_add(page.count.min(PAGE_LIMIT)).min(total)
}

/// Turns a full debugger source registry into an unregistered, still runnable script.
///
/// Defect fixed: in the default `--debug development` mode every compiled script, module,
/// `eval` and `new Function` body is registered, and `SourceRegistry::register` refuses with
/// `Capacity` past `DebugLimits::scripts` (1,024 records) or `source_bytes` (16 MiB). The
/// refusal was thrown as a compile error, so a program loading more code than the debugger
/// budget, or calling `eval`/`new Function` more than 1,024 times, stopped working.
///
/// Decision: the debugger budget never limits the application. A script arriving after the
/// registry is full compiles and runs without a registration; its frames carry no location
/// and breakpoints in it stay unverified, exactly like code compiled outside development
/// mode. Raising the limits was rejected (it only moves the failure point). Evicting old
/// records was rejected: the registry guarantees retained revisions are never silently
/// evicted, which stable breakpoint identities depend on.
///
/// # Arguments
/// * `result` - The registry's answer for one script.
///
/// # Returns
/// `Some(registration)` for an admitted script, `None` when the registry is full.
///
/// # Errors
/// Every registry error other than `Capacity` (closed, invalid source or map, foreign key).
fn admit(
    result: Result<ScriptRegistration, DebugError>,
) -> Result<Option<ScriptRegistration>, DebugError> {
    match result {
        Ok(registration) => Ok(Some(registration)),
        Err(DebugError::Capacity) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn failure(error: DebugError) -> FailureReport {
    match error {
        DebugError::EngineFailure(report) => report,
        DebugError::Capacity => FailureReport::fault(
            FailureCode::Capacity,
            "debugger source registration capacity",
        ),
        DebugError::InvalidSource | DebugError::InvalidMap => FailureReport::fault(
            FailureCode::InvalidArtifact,
            "debugger refused source registration",
        ),
        DebugError::Closed => FailureReport::fault(FailureCode::ClosedContext, "debugger closed"),
        other => FailureReport::fault(
            FailureCode::Unsupported,
            &format!("debugger registration failed: {other}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_fail_at_capacity_while_string_previews_mark_truncation() {
        use super::super::QuickJsNgEngine;
        let engine = QuickJsNgEngine::new().unwrap();
        let id = engine.create_context(ContextLimits::default()).unwrap();
        let state = engine.context(id).unwrap();
        let ptr = state.raw.ptr().unwrap();
        let units = vec![u16::from(b'x'); 65_537];
        // SAFETY: ptr is live on its owner thread and units is borrowed for this
        // synchronous string constructor, whose owned result is released below.
        let value = unsafe { ffi::jbq_string(ptr, units.as_ptr(), units.len()) };
        assert_eq!(string(ptr, value, None), Err(DebugError::Capacity));
        let preview = string(ptr, value, Some(256)).unwrap();
        assert_eq!(preview.as_units().len(), 257);
        assert_eq!(preview.as_units()[256], 0x2026);
        // SAFETY: release the owned constructor result in its original context.
        unsafe { ffi::jbq_free(ptr, value) };
        engine.close_context(id).unwrap();
    }

    #[test]
    fn lexical_scope_depths_do_not_alias_closures_or_locals() {
        assert_eq!(variable_scope(0, 0), 0);
        assert_eq!(variable_scope(1, 4), FUNCTION_SCOPE);
        assert_eq!(variable_scope(2, 4), FUNCTION_SCOPE);
        assert_eq!(variable_scope(0, 1), 1);
        assert_eq!(variable_scope(0, 2), 2);
        assert_eq!(variable_scope(3, 0), CLOSURE_SCOPE);
        assert_ne!(variable_scope(0, 1), variable_scope(3, 1));
    }

    #[test]
    fn marker_callback_records_normalized_positions_and_stops_after_failure() {
        let mut output = MarkerOutput {
            markers: Vec::new(),
            failed: false,
        };
        // SAFETY: this synchronous invocation receives a live exclusive output pointer.
        unsafe { collect_marker((&mut output as *mut MarkerOutput).cast(), 42, 7, 2, 3, 0) };
        assert_eq!(output.markers.len(), 1);
        assert_eq!((output.markers[0].line, output.markers[0].column), (1, 2));
        for (line, column) in [(0, 3), (2, 0), (-1, -1)] {
            // SAFETY: unknown native metadata uses the same live visitor output.
            unsafe {
                collect_marker(
                    (&mut output as *mut MarkerOutput).cast(),
                    42,
                    8,
                    line,
                    column,
                    0,
                )
            };
        }
        assert_eq!(
            output.markers.len(),
            1,
            "unknown positions cannot become executable (0,0)"
        );
        output.failed = true;
        // SAFETY: same live exclusive output as above, with failure already recorded.
        unsafe { collect_marker((&mut output as *mut MarkerOutput).cast(), 42, 8, 4, 5, 0) };
        assert_eq!(output.markers.len(), 1);
    }

    #[test]
    fn native_evaluator_rejection_restores_prior_rust_fault() {
        let engine = super::super::QuickJsNgEngine::new().unwrap();
        let id = engine.create_context(ContextLimits::default()).unwrap();
        let state = engine.context(id).unwrap();
        let coordinator = DebugCoordinator::new(
            state.debug.clone(),
            id,
            DebugLimits::default(),
            Arc::new(|| Ok(())),
            NonZeroU64::new(1).unwrap(),
        )
        .unwrap();
        let _handle = coordinator.attach(DebugSessionOptions::default()).unwrap();
        let original = FailureReport::fault(FailureCode::Capacity, "prior application fault");
        state.raw.record_fault(original.clone());
        // Deliberately exercise native selected-lease rejection while the real
        // evaluator guard isolates its own exception and Rust fault state.
        let (_, status) = state
            .debug
            .evaluate_controlled(
                state.raw.ptr().unwrap(),
                0,
                &JsString::from_utf8("1+1"),
                EvaluationControl::new(EvaluationOptions::default()).unwrap(),
            )
            .unwrap();
        assert_eq!(status, EvaluationStatus::Threw);
        assert_eq!(state.raw.take_fault(), Some(original));
        coordinator.shutdown().unwrap();
        engine.close_context(id).unwrap();
    }
}
