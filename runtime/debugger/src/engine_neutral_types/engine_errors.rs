use super::{JsData, JsString};
use std::fmt;

/// Origin of a detached failure; only script failures may follow a continue policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureClass {
    /// An exception from application JavaScript.
    Script,
    /// A host callback or host service failure.
    Host,
    /// A violated ownership, identity, capacity, or API contract.
    Contract,
    /// A native engine or bridge failure requiring the fatal path.
    Engine,
}
/// Stable classification of a detached engine/host contract failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureCode {
    /// An uncaught JavaScript value was thrown.
    ScriptException,
    /// A promise was still rejected without a handler once the microtask queue drained;
    /// the report describes its reason.
    UnhandledRejection,
    /// A stale or invalid value/module handle was supplied.
    InvalidHandle,
    /// An operation was attempted outside its owner thread.
    WrongOwner,
    /// A token belongs to another engine or context.
    WrongContext,
    /// The referenced context has already been closed.
    ClosedContext,
    /// Closing or releasing requires all active native scopes to unwind first.
    ActiveContext,
    /// The requested capability is unavailable.
    Unsupported,
    /// A value cannot be represented by the base copied data contract.
    UnsupportedConversion,
    /// A configured limit, identity space, or checked allocation was exhausted.
    Capacity,
    /// Artifact metadata, provenance requirements, or compatibility checks failed.
    InvalidArtifact,
    /// A host panic was contained at an owner entry or native callback boundary.
    HostPanic,
    /// The configured synchronous host reentry limit was reached.
    CallbackDepth,
    /// The native engine or bridge reported a fatal failure.
    EngineFailure,
    /// Native execution unwound for an explicitly requested cooperative stop.
    /// Resource, host and unrelated engine failures must not use this code.
    Interrupted,
}
/// Detached source position normalized to zero-based lines and UTF-16 columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLocation {
    /// Stable host-supplied source URI.
    pub origin: String,
    /// Zero-based line, or absent when unavailable.
    pub line: Option<u32>,
    /// Zero-based UTF-16 code-unit column, or absent when unavailable.
    pub column: Option<u32>,
}
/// Owned diagnostic frame containing no native stack or value handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackFrameReport {
    /// Optional safely obtained function name.
    pub function: Option<JsString>,
    /// Optional normalized source position.
    pub location: Option<SourceLocation>,
}
/// Detached diagnostic that remains valid after context destruction.
///
/// Adapters must collect it without invoking `toString`, getters, proxies, or
/// application functions. Public fields permit construction; call [`Self::bounded`]
/// before publishing untrusted or potentially large diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub struct FailureReport {
    /// Failure origin controlling whether script continuation is eligible.
    pub class: FailureClass,
    /// Stable machine-readable failure category.
    pub code: FailureCode,
    /// The thrown error's `name` (`TypeError`), read from data properties only; absent
    /// for non-error values. Bounded to 256 UTF-16 units on normalization.
    pub name: Option<JsString>,
    /// Optional lossless message, bounded to 4,096 UTF-16 units on normalization.
    pub message: Option<JsString>,
    /// Optional safe owned snapshot; never a live exception handle.
    pub thrown: Option<JsData>,
    /// Detached frames, capped at 64 by normalization.
    pub stack: Vec<StackFrameReport>,
    /// Whether any message, frame, or thrown snapshot was omitted or shortened.
    pub truncated: bool,
}
impl FailureReport {
    /// Creates a bounded report with the supplied class, code, and UTF-8 message.
    pub fn new(class: FailureClass, code: FailureCode, message: &str) -> Self {
        Self {
            class,
            code,
            name: None,
            message: Some(JsString::from_utf8(message)),
            thrown: None,
            stack: Vec::new(),
            truncated: false,
        }
        .bounded()
    }
    /// Creates a bounded contract-class report for an API or ownership failure.
    pub fn fault(code: FailureCode, message: &str) -> Self {
        Self::new(FailureClass::Contract, code, message)
    }
    /// Consumes and normalizes diagnostic storage to the report limits.
    ///
    /// Messages retain at most 4,096 UTF-16 units, at most 64 frames survive, and
    /// accounted owned storage is bounded to 64 KiB. Oversized thrown snapshots
    /// are removed; any omission sets `truncated`. No JavaScript is executed.
    pub fn bounded(mut self) -> Self {
        const MAX: usize = 65_536;
        let mut bytes = std::mem::size_of::<Self>();
        self.name = self.name.take().map(|n| {
            let (n, truncated) = n.bound(256);
            self.truncated |= truncated;
            bytes += n.capacity_bytes();
            n
        });
        self.message = self.message.take().map(|m| {
            let (m, truncated) = m.bound(4096);
            self.truncated |= truncated;
            bytes += m.capacity_bytes();
            m
        });
        let mut frames = Vec::with_capacity(self.stack.len().min(64));
        bytes += frames.capacity() * std::mem::size_of::<StackFrameReport>();
        for mut frame in self.stack.drain(..) {
            if frames.len() == 64 {
                self.truncated = true;
                break;
            }
            let mut size = 0usize;
            if let Some(name) = frame.function.take() {
                let (name, shortened) = name.bound(256);
                self.truncated |= shortened;
                size += name.capacity_bytes();
                frame.function = Some(name);
            }
            if let Some(location) = &mut frame.location {
                if location.origin.len() > 1024 {
                    let mut end = 1024;
                    while !location.origin.is_char_boundary(end) {
                        end -= 1;
                    }
                    location.origin = location.origin[..end].to_owned();
                    self.truncated = true;
                } else {
                    location.origin = location.origin.as_str().to_owned();
                }
                size += location.origin.capacity();
            }
            if bytes.saturating_add(size) > MAX {
                self.truncated = true;
                break;
            }
            bytes += size;
            frames.push(frame);
        }
        self.stack = frames;
        if let Some(value) = &self.thrown {
            let limits = super::ExportLimits {
                max_bytes: MAX.saturating_sub(bytes),
                ..Default::default()
            };
            if value.footprint(limits).is_err() {
                self.thrown = None;
                self.truncated = true;
            }
        }
        self
    }
}
impl fmt::Display for FailureReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {:?}", self.code, self.message)
    }
}
impl std::error::Error for FailureReport {}

/// Unaccepted owned input paired with the reason admission failed.
#[derive(Debug)]
pub struct Rejected<T> {
    /// The original input returned to its caller.
    pub value: T,
    /// Why the input was not accepted.
    pub reason: RejectionReason,
}
/// Reason an owned input was rejected before admission.
#[derive(Clone, Debug, PartialEq)]
pub enum RejectionReason {
    /// The relevant bounded queue or reservation capacity is full.
    Full,
    /// Admission has closed.
    Closed,
    /// The requested delivery path or capability is unavailable.
    Unsupported,
    /// An owner-local action was requested from another thread.
    WrongOwner,
    /// An input carries a token for another context.
    WrongContext,
    /// A repeating timer has an invalid period.
    InvalidPeriod,
    /// The requested monotonic deadline cannot be represented.
    DeadlineOverflow,
    /// A non-reusable identity or generation counter was exhausted.
    IdentityExhausted,
    /// Copied input data failed validation; the report is detached.
    InvalidData(FailureReport),
    /// Source registration or native source admission failed.
    InvalidSource(FailureReport),
    /// Transport signaling failed; the diagnostic contains no engine handle.
    WakeFailed(FailureReport),
    /// Debugger admission failed with a typed debugger error.
    Debug(crate::debug::DebugError),
}
/// Result of deciding cancellation for a task or reserved operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    /// Cancellation won before dispatch; operation failure delivery remains reserved.
    Canceled,
    /// A repeating callback is currently running and will not be rearmed.
    Running,
    /// Delivery or completion was already decided and cannot be replaced.
    TooLate,
    /// No matching live task/operation identity exists.
    NotFound,
    /// The cancellation was requested outside the owner thread.
    WrongOwner,
}
/// Truthful native microtask queue state after a drain attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointState {
    /// No native microtasks remain.
    Empty,
    /// Microtasks remain and require another checkpoint before ordinary work.
    Pending,
}
/// Bounded script diagnostics and queue state produced by a microtask drain.
///
/// Host, contract, and engine faults must instead use the fatal error return;
/// normal Promise rejection is Promise state, not automatically a failure report.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckpointReport {
    /// Whether native microtasks remain after the drain.
    pub state: CheckpointState,
    /// At most 64 detached uncaught script-job failures.
    pub failures: Vec<FailureReport>,
    /// Saturating count of additional script failures omitted from `failures`.
    pub suppressed_failures: u64,
}
impl CheckpointReport {
    /// Creates an empty-queue report with no failures.
    pub fn empty() -> Self {
        Self {
            state: CheckpointState::Empty,
            failures: Vec::new(),
            suppressed_failures: 0,
        }
    }
    /// Bounds and retains a script failure, counting excess reports after 64.
    ///
    /// Returns a bounded non-script report as `Err`; fatal failures are never
    /// hidden by the script diagnostic cap.
    pub fn push_script_failure(&mut self, report: FailureReport) -> Result<(), FailureReport> {
        if report.class != FailureClass::Script {
            return Err(report.bounded());
        }
        if self.failures.len() < 64 {
            self.failures.push(report.bounded());
        } else {
            self.suppressed_failures = self.suppressed_failures.saturating_add(1);
        }
        Ok(())
    }
}
