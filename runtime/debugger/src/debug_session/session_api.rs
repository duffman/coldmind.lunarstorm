use super::{HitCondition, OriginalLocation, evaluation::*, inspection::*};
use crate::engine::{ContextId, FailureReport, JsString, ScriptSource, SourceDigest};
use std::{fmt, rc::Rc, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Native debugger capability; advertise only behavior demonstrated by the adapter.
///
/// The first sixteen capabilities form the required development baseline. Optional
/// extensions cannot be supplied by a wire protocol or source-map frontend alone.
#[repr(u8)]
pub enum DebugCapability {
    /// Attach before application entry and retain hooks while development is unattached.
    Attach,
    /// Register and read bounded native source identities/text.
    Sources,
    /// Verify source line breakpoints against actual executable native locations.
    LineBreakpoints,
    /// Evaluate breakpoint conditions using native frame semantics.
    ConditionalBreakpoints,
    /// Reach native safe points while pure JavaScript is executing.
    Pause,
    /// Continue suspended native execution without pumping application jobs.
    Resume,
    /// Step into native application calls.
    StepInto,
    /// Step across calls in the current frame.
    StepOver,
    /// Continue until the selected native frame returns.
    StepOut,
    /// Pause on caught exceptions before application policy is applied.
    PauseOnCaught,
    /// Pause on uncaught exceptions before application policy is applied.
    PauseOnUncaught,
    /// Read native stack frames under a live pause identity.
    StackFrames,
    /// Read native lexical scope metadata for a paused frame.
    Scopes,
    /// Read paged variables/objects without invoking accessors or coercion.
    Variables,
    /// Explicitly evaluate in a selected paused native frame.
    FrameEvaluate,
    /// Register engine-discovered dynamic scripts before their first instruction.
    DynamicSourceGate,
    /// Optional asynchronous stack ancestry.
    AsyncStacks,
    /// Optional executable-location breakpoints at UTF-16 columns.
    ColumnBreakpoints,
    /// Optional logging breakpoints.
    Logpoints,
    /// Optional explicit paused variable mutation.
    VariableMutation,
    /// Optional native guarantee that evaluation cannot mutate application state.
    ReadOnlyEvaluation,
    /// Optional native CPU profiling.
    CpuProfiler,
    /// Optional native heap snapshots.
    HeapSnapshot,
    /// Optional native reverse execution.
    ReverseExecution,
    /// Optional native live code editing.
    LiveEdit,
    /// Optional safe cancellation/unwind of an in-flight debugger evaluation.
    CancelEvaluation,
    /// Optional typed values and identity-based inspection entries.
    RichInspection,
    /// Optional bounded resumable inspection cursors.
    LazyInspection,
    /// Optional non-executing binding and own-property lookup.
    SafeLookup,
    /// Optional safe native exception snapshots.
    ExceptionDetails,
    /// Optional independently controlled worker targets.
    WorkerTargets,
    /// Optional logical hit-count policies and count-preserving enablement.
    BreakpointPolicies,
    /// Optional temporary native continue-to operations.
    ContinueTo,
    /// Optional source-URI skip rules for step operations.
    SkipSources,
    /// Optional checkpoint-qualified Promise rejection tracking and historical pauses.
    UnhandledRejections,
}
/// Compact capability set supporting the required baseline and optional extensions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DebugCapabilities(u64);
impl DebugCapabilities {
    /// Returns a set with no capabilities.
    pub const fn empty() -> Self {
        Self(0)
    }
    /// Returns all sixteen required development-debugging capabilities.
    pub const fn baseline() -> Self {
        Self((1 << 16) - 1)
    }
    /// Returns this set with one capability included.
    pub const fn with(self, capability: DebugCapability) -> Self {
        Self(self.0 | (1u64 << capability as u8))
    }
    /// Returns the union of two capability sets.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    /// Reports whether every capability in `other` is present.
    pub const fn contains_all(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
    /// Reports whether a single capability is present.
    pub const fn contains(self, capability: DebugCapability) -> bool {
        self.contains_all(Self::empty().with(capability))
    }
    /// Reports whether no capabilities are present.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}
/// Required debugging profile; development is the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DebugMode {
    /// Require the complete native debugger baseline before application admission.
    #[default]
    Development,
    /// Explicitly opt out of sessions/queues/source-map processing; require no extras.
    Disabled,
}
/// Debugger profile and additional native requirements checked before execution.
#[derive(Clone, Debug, Default)]
pub struct DebugRequirements {
    /// Required development or explicitly disabled profile.
    pub mode: DebugMode,
    /// Extensions required in addition to the full development baseline.
    pub additional: DebugCapabilities,
}
impl DebugRequirements {
    /// Creates the explicit opt-out profile with an empty additional capability set.
    pub fn disabled() -> Self {
        Self {
            mode: DebugMode::Disabled,
            additional: DebugCapabilities::empty(),
        }
    }
}
/// Immutable native debugger identity and truthful supported capability metadata.
///
/// A retained backend must keep this metadata accessible after context close,
/// while rejecting subsequent native operations with [`DebugError::Closed`].
#[derive(Clone, Debug)]
pub struct DebugDescriptor {
    /// Version of the typed native debugger contract implemented.
    pub api_version: u32,
    /// Native behaviors actually supported by this adapter/build.
    pub capabilities: DebugCapabilities,
    /// Required debug build flags, native hooks, and patch/revision identities.
    pub native_requirements: Vec<String>,
    /// Description of the native running/paused wake mechanism and handoff assumptions.
    pub interrupt_mechanism: String,
}
/// Bounds for host debugger bookkeeping, replies, sources, and protocol output.
///
/// Runtime budgets apply only to debugger operations, never application execution.
/// These bounds do not limit native heap allocation.
/// A coordinator/transport implementing these interfaces must enforce limits
/// before admission or native changes; the value alone creates no enforcement.
#[derive(Clone, Debug)]
pub struct DebugLimits {
    /// Maximum outstanding commands, including terminal replies not yet consumed.
    pub requests: usize,
    /// Maximum non-reply updates; urgent/latest-state/closure records need separate reservation.
    pub updates: usize,
    /// Maximum owned bytes per admitted normal request.
    pub request_bytes: usize,
    /// Maximum reserved common reply bytes per admitted request.
    pub reply_bytes: usize,
    /// Maximum retained inspection objects for one pause.
    pub inspection_objects: usize,
    /// Independent bound for retained exact edit targets; does not consume frame/cursor slots.
    pub write_targets: usize,
    /// Maximum entries emitted by a native inspection page.
    pub inspection_page_items: usize,
    /// Maximum native storage slots examined by one inspection page.
    pub inspection_scan_slots: usize,
    /// Maximum discovery work units: source/marker scans, comparisons and moved entries.
    pub location_scan_slots: usize,
    /// Maximum retained source records per context.
    pub scripts: usize,
    /// Maximum retained source/map/original text capacity per context.
    pub source_bytes: usize,
    /// Maximum breakpoint entries per session.
    pub breakpoints: usize,
    /// Maximum owned breakpoint-condition and log-template text capacity per session.
    pub condition_bytes: usize,
    /// Maximum debugger expression length in UTF-16 code units.
    pub expression_units: usize,
    /// Default explicit/legacy evaluator deadline in milliseconds (nonzero).
    pub evaluation_deadline_ms: u64,
    /// Breakpoint-condition/logpoint evaluator budget in milliseconds (nonzero).
    pub condition_deadline_ms: u64,
    /// Maximum complete encoded DAP output message; must match the configured source budget.
    pub dap_output_bytes: usize,
    /// Maximum immutable scheduling boundaries in one ancestry chain.
    pub async_history_depth: usize,
    /// Maximum retained scheduling records for one context.
    pub async_history_records: usize,
    /// Maximum retained history backing allocation per context.
    pub async_history_bytes: usize,
    /// Maximum rooted Promise/reason pairs retained by rejection tracking.
    pub promise_rejections: usize,
}
impl Default for DebugLimits {
    fn default() -> Self {
        Self {
            requests: 64,
            updates: 128,
            request_bytes: 65_536,
            reply_bytes: 262_144,
            inspection_objects: 1024,
            write_targets: 1024,
            inspection_page_items: 64,
            inspection_scan_slots: 4096,
            location_scan_slots: 65_536,
            scripts: 1024,
            source_bytes: 16 * 1024 * 1024,
            breakpoints: 1024,
            condition_bytes: 1024 * 1024,
            expression_units: 16_384,
            evaluation_deadline_ms: 2_000,
            condition_deadline_ms: 100,
            dap_output_bytes: 64 * 1024 * 1024,
            async_history_depth: 32,
            async_history_records: 1024,
            async_history_bytes: 8 * 1024 * 1024,
            promise_rejections: 1024,
        }
    }
}

/// Opaque session identity bound to one issuer, VM, engine, and context generation.
///
/// Issued by [`super::DebugIdentities`]; ending/replacing a session makes it stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DebugSessionId {
    pub(crate) issuer_epoch: u64,
    pub(crate) vm_epoch: u64,
    pub(crate) engine_epoch: u64,
    pub(crate) context_generation: u64,
    pub(crate) generation: u64,
}
/// Opaque pause generation within a live debugger session.
///
/// Resume, step, detach, or close invalidates this pause and every reference
/// issued for it before another application callback may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PauseId {
    pub(crate) session: DebugSessionId,
    pub(crate) generation: u64,
}
macro_rules! pause_reference {
    ($name:ident, $description:literal) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[doc = $description]
        pub struct $name {
            pub(crate) pause: PauseId,
            pub(crate) generation: u64,
            pub(crate) slot: u64,
        }
    };
}
pause_reference!(
    FrameRef,
    "Opaque native stack-frame reference valid only for its issuing pause; resume, step, detach, or close invalidates it."
);
pause_reference!(
    ScopeRef,
    "Opaque native lexical-scope reference valid only for its issuing pause; native access requires current pause validation."
);
pause_reference!(
    ObjectRef,
    "Opaque retained inspection-object reference valid only for its issuing pause and bounded by the inspection object limit."
);
pause_reference!(
    BindingRef,
    "Opaque binding write identity valid for its issuing inspection generation and pause."
);
pause_reference!(
    PropertyRef,
    "Opaque own-property write identity valid for its issuing inspection generation and pause."
);
pause_reference!(
    CursorRef,
    "Opaque inspection continuation valid for its issuing inspection generation and pause."
);
/// Opaque normal-command identity bound to one debugger session.
///
/// Every admitted command retains one terminal reply reservation until its reply
/// is consumed/disposed; urgent controls do not need a request identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DebugRequestId {
    pub(crate) session: DebugSessionId,
    pub(crate) sequence: u64,
}
/// Opaque context/engine-bound debugger source identity, distinct from native I/O IDs.
///
/// Source revision/digest disambiguation is carried separately; it is not a native
/// pointer and does not itself retain native code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScriptId {
    pub(crate) issuer_epoch: u64,
    pub(crate) engine_epoch: u64,
    pub(crate) context_generation: u64,
    pub(crate) sequence: u64,
}
/// Adapter-issued native script-table key bound to its context.
///
/// Use table metadata, never an encoded native pointer. The adapter is responsible
/// for uniqueness and native lifetime; the key alone retains no script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeScriptKey {
    context: ContextId,
    sequence: u64,
}
impl NativeScriptKey {
    /// Combines a context identity and adapter-supplied nonzero unique sequence.
    ///
    /// This stores metadata only and does not verify a native script or context.
    pub fn new(context: ContextId, sequence: std::num::NonZeroU64) -> Self {
        Self {
            context,
            sequence: sequence.get(),
        }
    }
    /// Returns the context owning the native script-table entry.
    pub fn context(self) -> ContextId {
        self.context
    }
    /// Returns the adapter-issued nonzero table sequence.
    pub fn sequence(self) -> u64 {
        self.sequence
    }
}
/// Native origin/category of a source unit registered before execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptKind {
    /// Host-entered standalone source.
    Script,
    /// Native module source.
    Module,
    /// Application `eval` source discovered by the native engine.
    Eval,
    /// Source generated by a native `Function` constructor.
    Function,
    /// Explicit debugger frame-evaluation source.
    DebuggerEval,
}
/// Adapter-provided script identity and source submitted to the registration gate.
#[derive(Debug)]
pub struct NativeScriptMetadata {
    /// Adapter native script-table key, never a pointer.
    pub key: NativeScriptKey,
    /// Lossless source, canonical URI, and optional supplied mapping.
    pub source: ScriptSource,
    /// Origin/category used to preserve native script semantics.
    pub kind: ScriptKind,
}
/// Host-assigned immutable source identity returned before native execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScriptRegistration {
    /// Canonical debugger script identity.
    pub id: ScriptId,
    /// Monotonic revision for this source URI/content record.
    pub revision: u64,
    /// SHA-256 of lossless UTF-16LE source units.
    pub digest: SourceDigest,
}
/// Authoritative debug state, separate from ordinary scheduler readiness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugExecutionState {
    /// Development hooks may exist, but no interactive session is attached.
    Detached,
    /// A session is attached without a current application pause.
    Running,
    /// Application execution is suspended under this pause identity.
    Paused(PauseId),
    /// Debugger/native access has been permanently revoked.
    Closed,
}
/// Native event causing an application pause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseReason {
    /// Requested startup break before application entry proceeds.
    Entry,
    /// A verified executable breakpoint was hit.
    Breakpoint,
    /// A native step operation completed.
    Step,
    /// An exception matched the configured debugger policy.
    Exception,
    /// An attached native `debugger` statement requested a pause.
    DebuggerStatement,
    /// An urgent external pause reached a native safe point.
    Requested,
    /// A drained checkpoint qualified an unhandled rejection; no live frame is implied.
    UnhandledRejection,
}
/// One admitted execution decision for a current pause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugContinue {
    /// Resume normal application execution.
    Resume,
    /// Resume until the next native step, entering called functions.
    StepInto,
    /// Resume until the next native step in the current frame.
    StepOver,
    /// Resume until the current native frame returns.
    StepOut,
}
/// Source-URI glob rules for step stops. `*` stays within one path segment;
/// `**` crosses slashes. Matching is case-sensitive without URI normalization.
#[derive(Clone, Debug, Default)]
pub struct StepPolicy {
    /// At most 64 bounded patterns; an empty list disables source skipping.
    pub skip_sources: Vec<String>,
}
/// Native debugger exception-pause policy, independent of application error policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExceptionPolicy {
    /// Do not pause for exceptions.
    None,
    /// Pause for uncaught exceptions only.
    Uncaught,
    /// Pause for caught and uncaught exceptions.
    All,
}
/// Host-issued session identity and startup pause policy for native attachment.
#[derive(Clone, Copy, Debug)]
pub struct DebugAttachOptions {
    /// Live issuer-owned session to attach; adapters must not manufacture IDs.
    pub session: DebugSessionId,
    /// Request an entry pause before application code executes.
    pub break_on_start: bool,
}
/// Progress and authoritative execution state after servicing native debug controls.
#[derive(Clone, Copy, Debug)]
pub struct DebugServiceReport {
    /// Whether native/control work advanced during this service attempt.
    pub made_progress: bool,
    /// Current attached/running/paused/closed state after service.
    pub state: DebugExecutionState,
}
/// Typed debugger failure containing no live engine handle.
///
/// Reference/session/pause checks precede native access. A wake failure does not
/// by itself imply that a previously published command was unaccepted.
#[derive(Clone, Debug, PartialEq)]
pub enum DebugError {
    /// The native capability or requested behavior is unavailable.
    Unsupported,
    /// An operation requires an attached session.
    NotAttached,
    /// An operation requires suspended application execution.
    NotPaused,
    /// The supplied session is no longer current.
    StaleSession,
    /// The supplied pause has resumed or been replaced.
    StalePause,
    /// Evaluation or editing invalidated the inspection view.
    StaleInspection,
    /// The reference kind, slot, or generation is invalid.
    InvalidReference,
    /// The request shape, expression, or pagination is invalid.
    InvalidRequest,
    /// The source identity/revision or source admission is invalid.
    InvalidSource,
    /// Supplied source mapping is malformed, mismatched, or conflicts with retained mapping.
    InvalidMap,
    /// A configured bound, reservation, allocation, or identity space is exhausted.
    Capacity,
    /// Evaluator-only cancellation; earlier effects are retained.
    EvaluationCancelled {
        /// Earlier application writes or queued work may remain.
        effects_may_remain: bool,
    },
    /// Evaluator-only expiry; earlier effects are retained.
    EvaluationDeadlineExceeded {
        /// Earlier application writes or queued work may remain.
        effects_may_remain: bool,
    },
    /// A conflicting session/pause/control/evaluation decision is already active.
    InFlight,
    /// The session/context/backend has permanently closed native access.
    Closed,
    /// Detached failure signaling already-published control or transport state.
    WakeFailed(FailureReport),
    /// Detached native engine/owner contract failure.
    EngineFailure(FailureReport),
}
impl fmt::Display for DebugError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for DebugError {}
/// Bounded page request; source offsets/counts use UTF-16 units, collections use items.
#[derive(Clone, Copy, Debug)]
pub struct Page {
    /// Zero-based first item or UTF-16 code-unit offset.
    pub start: usize,
    /// Maximum requested item or code-unit count, additionally subject to reply bounds.
    pub count: usize,
}
/// Canonical source URI and optional content identity for breakpoint configuration.
#[derive(Clone, Debug)]
pub struct ScriptSelector {
    /// Host/adapter canonical URI, without implicit filesystem normalization.
    pub origin: String,
    /// Optional digest requiring a particular source revision/content.
    pub expected_digest: Option<SourceDigest>,
}
/// Normalized zero-based source position, measured in UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourcePosition {
    /// Zero-based line.
    pub line: u32,
    /// Zero-based UTF-16 code-unit column.
    pub column: u32,
}
/// Client-correlated requested source breakpoint in normalized coordinates.
#[derive(Clone, Debug)]
pub struct BreakpointSpec {
    /// Client identifier echoed in breakpoint replies and asynchronous updates.
    pub client_id: u64,
    /// Zero-based line in the selected generated or original source.
    pub line: u32,
    /// Optional zero-based UTF-16 code-unit column.
    pub column: Option<u32>,
    /// Optional native frame expression, bounded before installation.
    pub condition: Option<JsString>,
    /// Whether this logical breakpoint participates; disabling preserves its count.
    pub enabled: bool,
    /// Raw-hit predicate, applied after evaluating the condition.
    pub hit_condition: Option<HitCondition>,
    /// Nonempty templates log instead of pausing; {{ and }} escape braces.
    pub log_message: Option<JsString>,
}
impl Default for BreakpointSpec {
    fn default() -> Self {
        Self {
            client_id: 0,
            line: 0,
            column: None,
            condition: None,
            enabled: true,
            hit_condition: None,
            log_message: None,
        }
    }
}
/// Current-pause lexical scope or retained object whose children are requested.
#[derive(Clone, Copy, Debug)]
pub enum VariableParent {
    /// Read variables in the validated lexical scope.
    Scope(ScopeRef),
    /// Read properties of the validated retained inspection object.
    Object(ObjectRef),
}
/// Normal inspection/configuration command requiring one reserved terminal reply.
///
/// Pause-dependent requests must reject stale identities before native access.
/// Inspection/preview must not call getters, proxies, or `toString`; explicit
/// evaluation is the separate side-effecting operation. No request pumps ordinary
/// application jobs or performs an implicit microtask checkpoint.
#[derive(Debug)]
pub enum DebugRequest {
    /// Configure URI globs affecting only step stops, not explicit pauses.
    StepPolicy(StepPolicy),
    /// Read a bounded typed inspection page without executing application code.
    Inspect(InspectionRequest),
    /// Release one owner-issued continuation and its retained native resources.
    ReleaseCursor(CursorRef),
    /// Explicitly release abandoned exact targets; stale releases cannot revoke replacements.
    ReleaseWriteTargets(Vec<WriteTarget>),
    /// Evaluate a replacement in the selected frame and write exactly one issued target.
    /// The owner backend must capture and validate the target, invalidate inspection,
    /// then evaluate the RHS and recheck native write permissions before mutation.
    Write {
        /// Exact binding or own-property identity from an inspection entry.
        target: WriteTarget,
        /// Current frame in which the replacement expression is evaluated.
        frame: FrameRef,
        /// Lossless replacement expression.
        expression: JsString,
    },
    /// Resolve a restricted binding/member path without invoking application code.
    LookupPath {
        /// Current frame for lexical binding lookup.
        frame: FrameRef,
        /// Restricted path; never implicitly evaluated as an expression.
        path: JsString,
    },
    /// Explicit evaluation with a deadline and a structured terminal outcome.
    EvaluateWithOptions {
        /// Current frame for evaluation.
        frame: FrameRef,
        /// Lossless bounded expression.
        expression: JsString,
        /// Explicit execution policy.
        options: EvaluationOptions,
    },
    /// Read a page of retained source metadata.
    Sources {
        /// Requested source-record page.
        page: Page,
    },
    /// Read a bounded UTF-16 page from one immutable source revision.
    Source {
        /// Registered script identity.
        script: ScriptId,
        /// Exact retained source revision.
        revision: u64,
        /// UTF-16 offset/count page; successful text is never silently truncated.
        page: Page,
    },
    /// Read supplied original text by URI/digest through the common bounded host lane.
    OriginalSource {
        /// Canonical original URI and optional actual content digest.
        selector: ScriptSelector,
        /// UTF-16 offset/count; unavailable original text returns `InvalidSource`.
        page: Page,
    },
    /// Discover sorted unique executable positions in an inclusive source range.
    /// Reversed ranges are invalid; capacity exhaustion never returns a partial set.
    BreakpointLocations {
        /// Generated or original source selection, optionally pinned to its digest.
        selector: ScriptSelector,
        /// Inclusive first position.
        start: SourcePosition,
        /// Inclusive last position; columns beyond line length include its end.
        end: SourcePosition,
    },
    /// Replace the selected source’s requested breakpoint set after validating limits.
    SetBreakpoints {
        /// Canonical generated/original source selection and optional digest.
        selector: ScriptSelector,
        /// Requested set correlated by client IDs; pending sources remain pending.
        breakpoints: Vec<BreakpointSpec>,
    },
    /// Toggle one exact source/client identity without resetting its hit count.
    SetBreakpointEnabled {
        /// Source selection used by the original replacement operation.
        selector: ScriptSelector,
        /// Client identity within that exact selection.
        client_id: u64,
        /// New enabled state; counts remain unchanged.
        enabled: bool,
    },
    /// Resume toward one exact generated native location using a temporary marker.
    /// Any intervening pause, other run control, detach, stop or source invalidation
    /// cancels it. It never moves the program counter or skips application code.
    ContinueTo {
        /// Current pause authorizing the resume operation.
        pause: PauseId,
        /// Exact current source revision and executable position.
        location: DebugLocation,
    },
    /// Configure native debugger exception pauses.
    ExceptionPolicy(ExceptionPolicy),
    /// Inspect the original application throw under its current exception pause.
    ExceptionInfo {
        /// Current exception pause authorizing inspection.
        pause: PauseId,
    },
    /// Enable historical pauses after checkpoint-qualified unhandled rejections.
    UnhandledRejections(bool),
    /// Read bounded retained rejection metadata, including pending states.
    PromiseRejections {
        /// Requested metadata page; no original value or native pointer crosses threads.
        page: Page,
    },
    /// Read copied async scheduling ancestry; no historical entry is a live frame.
    AsyncStack {
        /// Live pause whose current job supplies the ancestry head.
        pause: PauseId,
    },
    /// Read a page of native stack frames for the current pause.
    Stack {
        /// Current pause authorizing native stack access.
        pause: PauseId,
        /// Requested frame page.
        page: Page,
    },
    /// Read lexical scope metadata for a current paused frame.
    Scopes {
        /// Current-pause frame whose native lexical scopes are requested.
        frame: FrameRef,
    },
    /// Read a page of safe variables/properties, marking unsafe access opaque.
    Variables {
        /// Current-pause scope or object authorizing inspection.
        parent: VariableParent,
        /// Requested variable/property page.
        page: Page,
    },
    /// Explicitly evaluate in a paused frame without awaiting a returned Promise.
    Evaluate {
        /// Validated current-pause frame used for native evaluation.
        frame: FrameRef,
        /// Lossless expression bounded by UTF-16 units and request bytes.
        expression: JsString,
        /// Whether side effects are permitted; false requires advertised read-only support.
        allow_side_effects: bool,
    },
}
/// Immutable exception stack metadata; never authorizes evaluation or editing.
#[derive(Clone, Debug)]
pub struct ExceptionFrame {
    /// Native function label, bounded as diagnostic text.
    pub name: JsString,
    /// Exact generated source revision when retained.
    pub generated: Option<DebugLocation>,
    /// Resolved original source metadata when available.
    pub original: Option<OriginalLocation>,
}
/// Safe snapshot of the original application throw at an exception pause.
#[derive(Clone, Debug)]
pub struct ExceptionInfo {
    /// Non-executing preview and optional pause-scoped expansion of the throw.
    pub value: DebugValue,
    /// Original generated throw location when available.
    pub location: Option<DebugLocation>,
    /// Whether a user catch, rather than merely finally, handles this throw.
    pub caught: bool,
    /// Bounded immutable stack metadata; contains no live frame references.
    pub stack: Vec<ExceptionFrame>,
    /// Non-executing diagnostic description of the thrown value.
    pub description: JsString,
}
/// Admitted normal command paired with its reserved terminal-reply identity.
#[derive(Debug)]
pub struct DebugCommand {
    /// Session-bound request ID retained until terminal reply consumption/disposal.
    pub id: DebugRequestId,
    /// Owned bounded inspection or configuration request.
    pub request: DebugRequest,
}
/// Owner-directed normal command or urgent control, separate from application jobs.
///
/// Urgent controls use preallocated state independent of normal queue saturation.
/// Stop takes precedence; detach must not leave an orphaned paused native call.
#[derive(Debug)]
pub enum DebugDirective {
    /// An admitted normal command with reserved terminal reply capacity.
    Request(DebugCommand),
    /// Coalesced urgent request to pause at a native safe point.
    Pause,
    /// One execution decision admitted for the specified current pause.
    Continue {
        /// Current pause whose references are invalidated by the decision.
        pause: PauseId,
        /// Native resume/step decision; competing decisions fail `InFlight` or `StalePause`.
        action: DebugContinue,
    },
    /// Remove session breakpoints and resume by default without destroying the VM.
    Detach,
    /// Close admission and request native termination/unwind without another application job.
    Stop,
}
/// Generated or original location tied to an exact retained script revision.
#[derive(Clone, Debug)]
pub struct DebugLocation {
    /// Registered source identity.
    pub script: ScriptId,
    /// Exact retained source revision, preventing replacement aliasing.
    pub revision: u64,
    /// Normalized zero-based line.
    pub line: u32,
    /// Normalized zero-based UTF-16 column, absent when native metadata is unavailable.
    pub column: Option<u32>,
}
/// Safe lazy native value preview with optional current-pause object expansion.
///
/// Producing a preview must not execute getters, proxies, coercion, or user code.
/// Unsupported inspection is reported as opaque/unavailable, never fabricated.
#[derive(Clone, Debug)]
pub struct DebugValue {
    /// Lossless safely obtained bounded display text.
    pub preview: JsString,
    /// Optional current-pause retained object for paged expansion.
    pub object: Option<ObjectRef>,
    /// Whether this member is an accessor whose getter was not invoked.
    pub accessor: bool,
    /// Optional reason safe inspection could not provide a value.
    pub unavailable: Option<String>,
}
/// One safely inspected lexical variable or object member.
#[derive(Clone, Debug)]
pub struct DebugVariable {
    /// Lossless variable/property display name.
    pub name: JsString,
    /// Safe lazy preview and optional expansion reference.
    pub value: DebugValue,
}
/// Native paused frame and its generated/optional mapped location.
#[derive(Clone, Debug)]
pub struct DebugFrame {
    /// Current-pause reference required for scope reads or explicit evaluation.
    pub id: FrameRef,
    /// Safely obtained native frame name.
    pub name: JsString,
    /// Actual normalized generated-code location when available.
    pub generated: Option<DebugLocation>,
    /// Validated mapped original location, absent when unavailable.
    pub original: Option<OriginalLocation>,
}
/// Native lexical-scope metadata for a paused frame.
#[derive(Clone, Debug)]
pub struct DebugScope {
    /// Current-pause reference authorizing paged variable reads.
    pub id: ScopeRef,
    /// Lossless native scope name.
    pub name: JsString,
    /// Whether expansion may require comparatively expensive native inspection.
    pub expensive: bool,
}
/// Retained immutable source registration and source length metadata.
#[derive(Clone, Debug)]
pub struct ScriptInfo {
    /// Canonical script ID, exact revision, and lossless-source digest.
    pub registration: ScriptRegistration,
    /// Canonical host/adapter-supplied UTF-8 source URI.
    pub origin: String,
    /// Full retained source length in UTF-16 code units.
    pub length_units: usize,
}
/// Truthful source breakpoint verification state.
#[derive(Clone, Debug)]
pub enum BreakpointState {
    /// The source has not loaded or verification is not yet complete.
    Pending,
    /// Actual executable native locations, possibly multiple per original breakpoint.
    Verified(Vec<DebugLocation>),
    /// The breakpoint cannot be mapped/installed; contains its reason.
    Rejected(String),
}
/// Client-correlated result of breakpoint verification or later re-verification.
#[derive(Clone, Debug)]
pub struct BreakpointInfo {
    /// Identifier from the corresponding requested breakpoint.
    pub client_id: u64,
    /// Pending, verified executable locations, or an explicit rejection reason.
    pub state: BreakpointState,
}
/// Terminal successful response to one admitted normal debugger command.
///
/// Collections/source text use bounded pages. Inspection values remain valid
/// only for their issuing pause; a Promise from evaluation is returned as a
/// value without awaiting it or draining application microtasks.
#[derive(Clone, Debug)]
pub enum DebugReply {
    /// Source skip policy was installed.
    StepPolicy,
    /// Bounded inspection page with explicit continuation and scan progress.
    Inspection(InspectionPage),
    /// The requested continuation and native resources were released.
    CursorReleased,
    /// Exact write-target resources and identity slots were released.
    WriteTargetsReleased,
    /// Safe preview after an identity-based write.
    Write(DebugValue),
    /// Safe result of a non-executing path lookup.
    Lookup(DebugValue),
    /// Structured outcome of an explicitly budgeted evaluation.
    EvaluationOutcome(EvaluationOutcome),
    /// One source-metadata page.
    Sources {
        /// Source registrations in the requested page.
        scripts: Vec<ScriptInfo>,
        /// Total number of available source records.
        total: usize,
    },
    /// One lossless UTF-16 source page from the requested revision.
    Source {
        /// Lossless source units in this page.
        text: JsString,
        /// Full source length in UTF-16 units for complete reassembly.
        total_units: usize,
    },
    /// Correlated breakpoint verification results, not a mere admission receipt.
    Breakpoints(Vec<BreakpointInfo>),
    /// Complete sorted unique native executable locations in the requested range.
    Locations(Vec<DebugLocation>),
    /// Executable positions in the original source selected by a discovery query.
    OriginalLocations(Vec<OriginalLocation>),
    /// Temporary continue-to armed; the owner resumes after publishing this reply.
    ContinueTo,
    /// Unhandled-rejection pause policy after successful configuration.
    UnhandledRejections,
    /// One bounded page of rejection state; dropped tracking is explicit.
    PromiseRejections {
        /// Detached records in native observation order.
        records: Vec<super::PromiseRejectionInfo>,
        /// Current retained record count.
        total: usize,
        /// Rejections evicted or not retained within configured limits.
        dropped: u64,
        /// Retained Rust table/root-wrapper allocation; native JS graphs are separate.
        retained_bytes: usize,
    },
    /// Detached immutable async ancestry and retention/truncation metadata.
    AsyncStack(super::AsyncStackTrace),
    /// Native exception-pause policy after successful configuration.
    ExceptionPolicy(ExceptionPolicy),
    /// Non-executing snapshot of the original application throw.
    ExceptionInfo(ExceptionInfo),
    /// One native frame page for the requested pause.
    Stack {
        /// Native frames in the requested page.
        frames: Vec<DebugFrame>,
        /// Total number of native frames available for this pause.
        total: usize,
    },
    /// Native lexical scopes for the requested frame.
    Scopes(Vec<DebugScope>),
    /// One safely inspected variable/property page.
    Variables {
        /// Safe variables/properties in the requested page.
        variables: Vec<DebugVariable>,
        /// Total child count reported by native inspection.
        total: usize,
    },
    /// Explicit evaluation result or original thrown value preview.
    Evaluation {
        /// Safe result/exception preview and optional current-pause expansion.
        value: DebugValue,
        /// Whether evaluation threw instead of returning normally.
        exception: bool,
    },
}
/// Reply or asynchronous state/source notification from native debug service.
///
/// A coordinator must reserve terminal replies, latest-state transitions, and
/// closure independently of lossy diagnostics. Sequence numbers allow a slow
/// consumer to reconcile coalesced transitions to the newest pause/state.
#[derive(Clone, Debug)]
pub enum DebugUpdate {
    /// Exactly one terminal success/error for an admitted normal command.
    Reply {
        /// Request identity whose terminal reply reservation is being fulfilled.
        id: DebugRequestId,
        /// Successful typed reply or terminal typed error.
        result: Result<DebugReply, DebugError>,
    },
    /// Authoritative pause published before inspection admission.
    Paused {
        /// Monotonic authoritative execution-state update sequence.
        sequence: u64,
        /// New current pause authorizing subsequent inspection.
        pause: PauseId,
        /// Native event causing suspension.
        reason: PauseReason,
        /// Actual normalized generated location when available.
        location: Option<DebugLocation>,
        /// Host-enriched original URI/coordinates, without a fabricated script ID.
        original: Option<OriginalLocation>,
        /// Matching user breakpoint IDs; temporary markers and logpoints have no ID.
        breakpoint_ids: Vec<u64>,
    },
    /// Pause invalidation published before subsequent application callbacks run.
    Resumed {
        /// Monotonic authoritative execution-state update sequence.
        sequence: u64,
        /// Invalidated pause; its frame/scope/object references must no longer reach native state.
        previous: PauseId,
    },
    /// A source registration became available before that source executed.
    ScriptRegistered(ScriptInfo),
    /// Qualified unhandled/handled-later transition; pending rejections are never emitted here.
    PromiseRejection(super::PromiseRejectionInfo),
    /// Complete bounded logpoint output; never application stdout or user coercion.
    Output {
        /// Lossless formatted message, including its final newline.
        message: JsString,
    },
    /// Previously requested breakpoints were re-verified after source changes.
    BreakpointsChanged(Vec<BreakpointInfo>),
    /// Bounded detached diagnostic whose coalesced losses are counted.
    Diagnostic {
        /// Bounded owned failure report with no live engine handle.
        report: FailureReport,
        /// Count of omitted/coalesced diagnostics represented by this update.
        dropped: u64,
    },
    /// Reserved terminal session-closure record, observable despite queue saturation.
    SessionClosed {
        /// Session whose requests/references have been closed.
        session: DebugSessionId,
        /// Optional detached shutdown or unsafe-to-resume evaluation failure reason.
        reason: Option<FailureReport>,
    },
}
/// Thread-safe native interrupt/control wake lease containing no owner-only values.
///
/// A poller wake alone cannot interrupt executing JavaScript. Implementations
/// must synchronize running/idle handoff and revoke outstanding short calls
/// before native destruction; native host functions remain cooperative.
pub trait DebugSignal: Send + Sync {
    /// Signals authoritative already-published control state after transport locks release.
    ///
    /// Must remain safe if execution became idle or close raced with signaling.
    /// Failure is reported without treating an accepted command as retryable.
    fn wake(&self) -> Result<(), DebugError>;
}
/// Owner-local coordinator boundary used by a native debug backend.
///
/// This interface defines registration, control and publication contracts without
/// implementing a coordinator. Drop all coordinator/registry/transport borrows
/// before any call that may reenter JavaScript or wait with native execution suspended.
pub trait DebugPort {
    /// Host-configured bounds for owner-thread native operations.
    fn limits(&self) -> &DebugLimits;
    /// Shared metadata issuer; adapters cannot construct opaque IDs themselves.
    fn identities(&self) -> &super::DebugIdentities;
    /// Registers/deduplicates a native source and validates supplied maps before execution.
    ///
    /// Return the canonical script ID/revision/digest even when no client is
    /// attached. Enforce retained-source capacity without silent live-source eviction;
    /// invalid/conflicting maps return `InvalidMap` rather than being ignored.
    fn register_script(
        &self,
        script: NativeScriptMetadata,
    ) -> Result<ScriptRegistration, DebugError>;
    /// Resolves a generated/original breakpoint to retained generated candidates.
    ///
    /// This owner-local host query validates source identity and supplied maps; it
    /// does not install breakpoints or certify an executable native instruction.
    /// The backend must verify every returned candidate against native code and
    /// preserve client breakpoint identity across multiple mapped locations.
    /// An empty result can mean unloaded or unmappable source, never verification.
    fn resolve_breakpoint(
        &self,
        selector: &ScriptSelector,
        line: u32,
        column: Option<u32>,
    ) -> Result<Vec<DebugLocation>, DebugError>;
    /// Selects an immutable generated source revision for executable discovery.
    /// Backends without discovery support retain an explicit unsupported result.
    fn select_script(
        &self,
        _selector: &ScriptSelector,
    ) -> Result<Option<ScriptRegistration>, DebugError> {
        Err(DebugError::Unsupported)
    }
    /// Checks whether an exact generated source revision is still current.
    fn is_current_script(&self, _location: &DebugLocation) -> Result<bool, DebugError> {
        Err(DebugError::Unsupported)
    }
    /// Projects a retained executable marker into a selected original source.
    /// Newest selectors exclude superseded generated revisions; exact original
    /// digests remain pinned. This never certifies a source-map segment as code.
    fn original_locations(
        &self,
        _selector: &ScriptSelector,
        _location: &DebugLocation,
    ) -> Result<Vec<OriginalLocation>, DebugError> {
        Err(DebugError::Unsupported)
    }
    /// Matches a retained source's canonical URI without executing application code.
    fn skip_source(
        &self,
        _location: &DebugLocation,
        _policy: &StepPolicy,
    ) -> Result<bool, DebugError> {
        Err(DebugError::Unsupported)
    }
    /// Takes the next normal/urgent directive, optionally waiting in a restricted pump.
    ///
    /// Waiting must recheck authoritative controls and cannot execute ordinary
    /// jobs, timers, completions, readiness callbacks, or microtask checkpoints.
    /// Urgent detach/stop remain reachable despite normal queue saturation.
    fn next_directive(&self, wait: bool) -> Option<DebugDirective>;
    /// Publishes a typed reply/update through the corresponding reserved capacity.
    ///
    /// Every admitted normal request terminates once; pause invalidation yields
    /// `StalePause` without native access. Diagnostics may coalesce with counts,
    /// but required terminal/current-state records must remain observable.
    fn publish(&self, update: DebugUpdate) -> Result<(), DebugError>;
    /// Reports authoritative stop state that closes admission and precedes other controls.
    fn is_stopping(&self) -> bool;
    /// Clone only the exact currently dispatched evaluator operation.
    fn evaluation_control(&self, _id: DebugRequestId) -> Option<EvaluationControl> {
        None
    }
}
/// Owner-local native debugging adapter independent of application value scopes.
///
/// Retained Rust storage/descriptor outlive context close; later native operations
/// return `Closed` without touching freed state. Native ownership back-references
/// are non-owning and must not form coordinator cycles. This trait does not create
/// a debugger, scheduler, protocol frontend, or JavaScript interpreter.
pub trait DebugBackend {
    /// Borrows immutable truthful native capabilities, including after native close.
    fn descriptor(&self) -> &DebugDescriptor;
    /// Installs the owner-local coordinator port before native source/application entry.
    ///
    /// Validate initialization/liveness and reject unsupported or conflicting setup.
    /// Do not create an ownership cycle with the port/native context.
    fn initialize(&self, port: Rc<dyn DebugPort>) -> Result<(), DebugError>;
    /// Attaches the issued session before application entry and returns its signal lease.
    ///
    /// Honor startup pause policy and reject stale/conflicting/closed state. The
    /// first attachment during an already-running native call is not a baseline promise.
    fn attach(&self, options: DebugAttachOptions) -> Result<Arc<dyn DebugSignal>, DebugError>;
    /// Services native debug controls on the owner without pumping application work.
    ///
    /// An outer application call may remain suspended while this restricted path
    /// handles inspection, evaluation, resume, detach, or stop. Report actual progress
    /// and state; no hard pause latency or evaluation time limit is implied.
    fn service(&self) -> Result<DebugServiceReport, DebugError>;
    /// Captures a copied scheduling boundary. Missing/over-budget metadata must
    /// not reject application work; unsupported adapters return `None`.
    fn capture_async(&self, _kind: super::AsyncScheduleKind) -> Option<super::AsyncStackId> {
        None
    }
    /// Returns this owner's current scheduling context, without native roots.
    fn async_parent(&self) -> Option<super::AsyncStackId> {
        None
    }
    /// Changes only debugger ancestry for a callback; never executes JavaScript.
    fn set_async_parent(&self, _parent: Option<super::AsyncStackId>) {}
    /// Finishes one combined nextTick/native-job checkpoint slice. `drained` is
    /// true only after the complete checkpoint empties, never on budget exhaustion.
    /// Implementations may open a historical pause without fabricating a live frame.
    fn checkpoint(&self, _drained: bool) -> Result<(), DebugError> {
        Ok(())
    }
    /// Closes session controls/breakpoints and resumes a suspended application by default.
    ///
    /// Invalidate inspection references first. If an in-flight evaluator cannot be
    /// safely canceled/restored, request stop with a detached reason rather than
    /// promising resume. This does not cancel application work or destroy the VM.
    fn detach(&self) -> Result<(), DebugError>;
}
