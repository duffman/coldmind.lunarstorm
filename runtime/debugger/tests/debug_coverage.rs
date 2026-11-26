//! Debugger metadata is tested without simulating a native debugger backend.
use coldmind_runtime::{debug::*, engine::*};
use std::num::NonZeroU64;

fn identities(capacity: usize) -> DebugIdentities {
    let engine = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    DebugIdentities::new(NonZeroU64::new(17).unwrap(), store.context(), capacity).unwrap()
}

#[test]
fn debug_capabilities_keep_the_baseline_distinct_from_optional_extensions() {
    let baseline = DebugCapabilities::baseline();
    let required = [
        DebugCapability::Attach,
        DebugCapability::Sources,
        DebugCapability::LineBreakpoints,
        DebugCapability::ConditionalBreakpoints,
        DebugCapability::Pause,
        DebugCapability::Resume,
        DebugCapability::StepInto,
        DebugCapability::StepOver,
        DebugCapability::StepOut,
        DebugCapability::PauseOnCaught,
        DebugCapability::PauseOnUncaught,
        DebugCapability::StackFrames,
        DebugCapability::Scopes,
        DebugCapability::Variables,
        DebugCapability::FrameEvaluate,
        DebugCapability::DynamicSourceGate,
    ];
    let optional = [
        DebugCapability::AsyncStacks,
        DebugCapability::ColumnBreakpoints,
        DebugCapability::Logpoints,
        DebugCapability::VariableMutation,
        DebugCapability::ReadOnlyEvaluation,
        DebugCapability::CpuProfiler,
        DebugCapability::HeapSnapshot,
        DebugCapability::ReverseExecution,
        DebugCapability::LiveEdit,
        DebugCapability::CancelEvaluation,
    ];
    assert!(DebugCapabilities::default().is_empty());
    assert!(baseline.contains_all(DebugCapabilities::empty()));
    let mut assembled = DebugCapabilities::empty();
    for capability in required {
        assert!(baseline.contains(capability));
        assembled = assembled.with(capability);
    }
    assert_eq!(baseline, assembled);
    for capability in optional {
        assert!(!baseline.contains(capability));
        let extension = DebugCapabilities::empty().with(capability);
        assert!(!extension.contains_all(baseline));
        let combined = baseline.union(extension);
        assert!(combined.contains(capability));
        assert!(combined.contains_all(baseline));
        assert!(!baseline.contains_all(combined));
        assert_eq!(combined.union(extension), combined);
    }
}

#[test]
fn development_is_default_and_explicit_disable_clears_required_extensions() {
    assert_eq!(DebugMode::default(), DebugMode::Development);
    let requirements = DebugRequirements::default();
    assert_eq!(requirements.mode, DebugMode::Development);
    assert!(requirements.additional.is_empty());
    let disabled = DebugRequirements::disabled();
    assert_eq!(disabled.mode, DebugMode::Disabled);
    assert!(disabled.additional.is_empty());
    let limits = DebugLimits::default();
    assert_eq!((limits.requests, limits.updates), (64, 128));
    assert_eq!(
        (limits.request_bytes, limits.reply_bytes),
        (65_536, 262_144)
    );
    assert_eq!(
        (
            limits.inspection_objects,
            limits.scripts,
            limits.breakpoints
        ),
        (1024, 1024, 1024)
    );
    assert_eq!(limits.source_bytes, 16 * 1024 * 1024);
    assert_eq!(limits.condition_bytes, 1024 * 1024);
    assert_eq!(limits.expression_units, 16_384);
    assert_eq!(limits.dap_output_bytes, 64 * 1024 * 1024);
}

#[test]
fn native_script_keys_preserve_nonzero_sequences_and_context_identity() {
    let engine = EngineIdentity::new().unwrap();
    let first = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    let second = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    let key = NativeScriptKey::new(first.context(), NonZeroU64::new(19).unwrap());
    assert_eq!(key.context(), first.context());
    assert_eq!(key.sequence(), 19);
    assert_ne!(
        key,
        NativeScriptKey::new(second.context(), NonZeroU64::new(19).unwrap())
    );
    assert_ne!(
        key,
        NativeScriptKey::new(first.context(), NonZeroU64::new(20).unwrap())
    );
}

#[test]
fn debug_error_display_retains_diagnostic_details() {
    for error in [
        DebugError::Unsupported,
        DebugError::NotAttached,
        DebugError::NotPaused,
        DebugError::StaleSession,
        DebugError::StalePause,
        DebugError::InvalidReference,
        DebugError::InvalidRequest,
        DebugError::InvalidSource,
        DebugError::InvalidMap,
        DebugError::Capacity,
        DebugError::InFlight,
        DebugError::Closed,
    ] {
        assert_eq!(format!("{error}"), format!("{error:?}"));
    }
    let report = FailureReport::fault(FailureCode::EngineFailure, "native diagnostic");
    let wake = DebugError::WakeFailed(report.clone());
    let native = DebugError::EngineFailure(report);
    assert!(format!("{wake}").starts_with("WakeFailed(FailureReport"));
    assert!(format!("{native}").contains("native diagnostic"));
    let _: &dyn std::error::Error = &native;
}

#[test]
fn request_and_update_sequences_survive_session_transitions_without_aliasing() {
    let ids = identities(3);
    assert_eq!(ids.state(), DebugExecutionState::Detached);
    assert_eq!(ids.next_update_sequence().unwrap(), 1);
    let session = ids.open_session().unwrap();
    assert_eq!(ids.state(), DebugExecutionState::Running);
    assert_eq!(ids.open_session(), Err(DebugError::InFlight));
    let first = ids.next_request(session).unwrap();
    assert_ne!(first, ids.next_request(session).unwrap());
    assert_eq!(ids.next_update_sequence().unwrap(), 2);
    let pause = ids.begin_pause(session).unwrap();
    assert_eq!(ids.state(), DebugExecutionState::Paused(pause));
    assert_eq!(ids.begin_pause(session), Err(DebugError::InFlight));
    assert_ne!(first, ids.next_request(session).unwrap());
    ids.close_session(session).unwrap();
    assert_eq!(ids.state(), DebugExecutionState::Detached);
    assert_eq!(ids.next_request(session), Err(DebugError::StaleSession));
    assert_eq!(ids.close_session(session), Err(DebugError::StaleSession));
    assert_eq!(ids.begin_pause(session), Err(DebugError::StaleSession));
    assert_eq!(ids.end_pause(pause), Err(DebugError::StaleSession));
    let next = ids.open_session().unwrap();
    assert_ne!(session, next);
    assert_ne!(first, ids.next_request(next).unwrap());
    assert_eq!(ids.next_update_sequence().unwrap(), 3);
}

#[test]
fn each_reference_kind_resolves_independently_and_pause_completion_reclaims_capacity() {
    let ids = identities(3);
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    let key = NonZeroU64::new(101).unwrap();
    let mut references = Vec::new();
    for kind in [
        ReferenceKind::Frame,
        ReferenceKind::Scope,
        ReferenceKind::Object,
    ] {
        let reference = ids.register(pause, kind, key).unwrap();
        match (kind, reference) {
            (ReferenceKind::Frame, DebugReference::Frame(_))
            | (ReferenceKind::Scope, DebugReference::Scope(_))
            | (ReferenceKind::Object, DebugReference::Object(_)) => {}
            _ => panic!("reference kind must match the registered kind"),
        }
        assert_eq!(reference.pause(), pause);
        assert_eq!(ids.resolve(reference).unwrap(), key);
        assert_eq!(ids.register(pause, kind, key).unwrap(), reference);
        references.push(reference);
    }
    assert_ne!(references[0], references[1]);
    assert_ne!(references[1], references[2]);
    assert_eq!(
        ids.register(pause, ReferenceKind::Frame, NonZeroU64::new(102).unwrap()),
        Err(DebugError::Capacity)
    );
    ids.end_pause(pause).unwrap();
    assert_eq!(ids.state(), DebugExecutionState::Running);
    assert_eq!(ids.end_pause(pause), Err(DebugError::StalePause));
    assert_eq!(
        ids.register(pause, ReferenceKind::Frame, key),
        Err(DebugError::StalePause)
    );
    for reference in references {
        assert_eq!(ids.resolve(reference), Err(DebugError::StalePause));
    }
    let next_pause = ids.begin_pause(session).unwrap();
    ids.register(next_pause, ReferenceKind::Object, key)
        .unwrap();
}

#[test]
fn identity_close_is_idempotent_and_rejects_all_public_issuing_operations() {
    let ids = identities(1);
    let session = ids.open_session().unwrap();
    let pause = ids.begin_pause(session).unwrap();
    let key = NonZeroU64::new(1).unwrap();
    let reference = ids.register(pause, ReferenceKind::Scope, key).unwrap();
    ids.close();
    ids.close();
    assert_eq!(ids.state(), DebugExecutionState::Closed);
    assert_eq!(ids.open_session(), Err(DebugError::Closed));
    assert_eq!(ids.begin_pause(session), Err(DebugError::Closed));
    assert_eq!(ids.end_pause(pause), Err(DebugError::Closed));
    assert_eq!(
        ids.register(pause, ReferenceKind::Scope, key),
        Err(DebugError::Closed)
    );
    assert_eq!(ids.resolve(reference), Err(DebugError::Closed));
    assert_eq!(ids.next_script(), Err(DebugError::Closed));
    assert_eq!(ids.next_request(session), Err(DebugError::Closed));
    assert_eq!(ids.next_update_sequence(), Err(DebugError::Closed));
    assert_eq!(ids.close_session(session), Err(DebugError::Closed));
}

#[test]
fn impossible_reference_capacity_is_a_typed_error_without_allocating() {
    let engine = EngineIdentity::new().unwrap();
    let store = ValueStore::<()>::new(&engine, ContextLimits::default()).unwrap();
    for capacity in [0, usize::MAX] {
        assert!(matches!(
            DebugIdentities::new(NonZeroU64::new(1).unwrap(), store.context(), capacity),
            Err(DebugError::Capacity)
        ));
    }
}
