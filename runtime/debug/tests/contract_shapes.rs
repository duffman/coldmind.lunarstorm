use coldmind_runtime::{debug::*, engine::*};
use static_assertions::{assert_impl_all, assert_not_impl_any};

assert_impl_all!(JsData: Send, Sync);
assert_impl_all!(FailureReport: Send, Sync);
assert_impl_all!(DebugRequest: Send, Sync);
assert_impl_all!(DebugUpdate: Send, Sync);
assert_not_impl_any!(ContextId: Send, Sync);
assert_not_impl_any!(RootedDeferred: Send, Sync, Clone);

#[test]
fn engine_scope_and_debugger_contracts_can_be_erased() {
    fn engine<T: JsEngine>(value: &T) -> &dyn JsEngine {
        value
    }
    fn scope<T: JsScope>(value: &T) -> &dyn JsScope {
        value
    }
    fn callback<T: HostFunction>(value: &T) -> &dyn HostFunction {
        value
    }
    fn debugger<T: DebugBackend>(value: &T) -> &dyn DebugBackend {
        value
    }
    fn port<T: DebugPort>(value: &T) -> &dyn DebugPort {
        value
    }
    fn signal<T: DebugSignal>(value: &T) -> &dyn DebugSignal {
        value
    }
    fn generated_candidates(
        port: &dyn DebugPort,
        selector: &ScriptSelector,
    ) -> Result<Vec<DebugLocation>, DebugError> {
        port.resolve_breakpoint(selector, 0, None)
    }
    let _ = generated_candidates;
    // Function items force trait object/vtable checking, even without an engine.
    let _ = (engine::<NeverEngine>, scope::<NeverEngine>);
    let _: Option<&dyn HostFunction> = None;
    let _: Option<&dyn DebugBackend> = None;
    let _: Option<&dyn DebugPort> = None;
    let _: Option<&dyn DebugSignal> = None;
    let _ = (
        callback::<NeverEngine>,
        debugger::<NeverEngine>,
        port::<NeverEngine>,
        signal::<NeverEngine>,
    );
}

// A compile-only adapter is an uninhabited type, never a fake running engine.
enum NeverEngine {}
macro_rules! impossible {
    ($this:ident) => {
        match *$this {}
    };
}
impl JsEngine for NeverEngine {
    fn descriptor(&self) -> &EngineDescriptor {
        impossible!(self)
    }
    fn create_context(&self, _: ContextLimits) -> Result<ContextId, FailureReport> {
        impossible!(self)
    }
    fn close_context(&self, _: ContextId) -> Result<(), FailureReport> {
        impossible!(self)
    }
    fn debug_backend(
        &self,
        _: ContextId,
    ) -> Result<Option<std::rc::Rc<dyn DebugBackend>>, FailureReport> {
        impossible!(self)
    }
    fn with_scope(
        &self,
        _: ContextId,
        _: &dyn HostServices,
        _: &mut dyn ScopeCall,
    ) -> Result<(), FailureReport> {
        impossible!(self)
    }
}
impl HostFunction for NeverEngine {
    fn invoke<'s>(&self, _: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        impossible!(self)
    }
}
impl DebugSignal for NeverEngine {
    fn wake(&self) -> Result<(), DebugError> {
        impossible!(self)
    }
}
impl DebugBackend for NeverEngine {
    fn descriptor(&self) -> &DebugDescriptor {
        impossible!(self)
    }
    fn initialize(&self, _: std::rc::Rc<dyn DebugPort>) -> Result<(), DebugError> {
        impossible!(self)
    }
    fn attach(&self, _: DebugAttachOptions) -> Result<std::sync::Arc<dyn DebugSignal>, DebugError> {
        impossible!(self)
    }
    fn service(&self) -> Result<DebugServiceReport, DebugError> {
        impossible!(self)
    }
    fn detach(&self) -> Result<(), DebugError> {
        impossible!(self)
    }
}
impl DebugPort for NeverEngine {
    fn limits(&self) -> &DebugLimits {
        impossible!(self)
    }
    fn identities(&self) -> &DebugIdentities {
        impossible!(self)
    }
    fn register_script(&self, _: NativeScriptMetadata) -> Result<ScriptRegistration, DebugError> {
        impossible!(self)
    }
    fn resolve_breakpoint(
        &self,
        _: &ScriptSelector,
        _: u32,
        _: Option<u32>,
    ) -> Result<Vec<DebugLocation>, DebugError> {
        impossible!(self)
    }
    fn next_directive(&self, _: bool) -> Option<DebugDirective> {
        impossible!(self)
    }
    fn publish(&self, _: DebugUpdate) -> Result<(), DebugError> {
        impossible!(self)
    }
    fn is_stopping(&self) -> bool {
        impossible!(self)
    }
}
impl JsScope for NeverEngine {
    fn data_limits(&self) -> ExportLimits {
        unreachable!()
    }
    fn context(&self) -> ContextId {
        impossible!(self)
    }
    fn global<'s>(&'s self) -> JsResult<'s, ScopedValue<'s>> {
        impossible!(self)
    }
    fn kind(&self, _: ScopedValue<'_>) -> Result<ValueKind, FailureReport> {
        impossible!(self)
    }
    fn local<'s>(&'s self, _: &RootedValue) -> JsResult<'s, ScopedValue<'s>> {
        impossible!(self)
    }
    fn retain(&self, _: ScopedValue<'_>) -> Result<RootedValue, FailureReport> {
        impossible!(self)
    }
    fn import_data<'s>(&'s self, _: JsData) -> JsResult<'s, ScopedValue<'s>> {
        impossible!(self)
    }
    fn export_data<'s>(&'s self, _: ScopedValue<'s>, _: ExportLimits) -> JsResult<'s, JsData> {
        impossible!(self)
    }
    fn get<'s>(&'s self, _: ScopedValue<'s>, _: PropertyKey<'_>) -> JsResult<'s, ScopedValue<'s>> {
        impossible!(self)
    }
    fn set<'s>(
        &'s self,
        _: ScopedValue<'s>,
        _: PropertyKey<'_>,
        _: ScopedValue<'s>,
    ) -> JsResult<'s, ()> {
        impossible!(self)
    }
    fn call<'s>(
        &'s self,
        _: ScopedValue<'s>,
        _: Option<ScopedValue<'s>>,
        _: &[ScopedValue<'s>],
    ) -> JsResult<'s, ScopedValue<'s>> {
        impossible!(self)
    }
    fn construct<'s>(
        &'s self,
        _: ScopedValue<'s>,
        _: &[ScopedValue<'s>],
        _: Option<ScopedValue<'s>>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        impossible!(self)
    }
    fn host_function<'s>(
        &'s self,
        _: &JsString,
        _: u16,
        _: std::rc::Rc<dyn HostFunction>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        impossible!(self)
    }
    fn deferred<'s>(&'s self) -> JsResult<'s, Deferred<'s>> {
        impossible!(self)
    }
    fn settle<'s>(&'s self, _: RootedDeferred, _: Settlement<'s>) -> JsResult<'s, ()> {
        impossible!(self)
    }
    fn drain_microtasks(
        &self,
        _: std::num::NonZeroUsize,
    ) -> Result<CheckpointReport, FailureReport> {
        impossible!(self)
    }
    fn detach(&self, _: ScopedError<'_>) -> FailureReport {
        impossible!(self)
    }
    fn source(&self) -> Option<&dyn SourceApi> {
        impossible!(self)
    }
    fn compiler(&self) -> Option<&dyn CompilerApi> {
        impossible!(self)
    }
    fn artifacts(&self) -> Option<&dyn ArtifactApi> {
        impossible!(self)
    }
    fn modules(&self) -> Option<&dyn ModuleApi> {
        impossible!(self)
    }
}

#[test]
fn development_requirements_reject_a_partial_debugger() {
    let requirements = EngineRequirements::default();
    let features = EngineFeatures {
        source: true,
        ..Default::default()
    };
    assert_eq!(
        requirements.validate(&features, None).unwrap_err().code,
        FailureCode::Unsupported
    );
    let disabled = EngineRequirements {
        debug: DebugRequirements::disabled(),
        ..requirements
    };
    disabled.validate(&features, None).unwrap();
}

#[test]
fn diagnostic_caps_do_not_suppress_a_later_fatal_fault() {
    let mut report = CheckpointReport::empty();
    for _ in 0..70 {
        report
            .push_script_failure(FailureReport::new(
                FailureClass::Script,
                FailureCode::ScriptException,
                "failure",
            ))
            .unwrap();
    }
    assert_eq!(report.failures.len(), 64);
    assert_eq!(report.suppressed_failures, 6);
    let fatal = FailureReport::fault(FailureCode::HostPanic, "fatal");
    assert_eq!(
        report.push_script_failure(fatal).unwrap_err().code,
        FailureCode::HostPanic
    );
    let long = FailureReport::new(
        FailureClass::Script,
        FailureCode::ScriptException,
        &"x".repeat(10_000),
    );
    assert_eq!(long.message.unwrap().as_units().len(), 4096);
    assert!(long.truncated);
}

#[test]
fn diagnostic_bounding_discards_unused_backing_capacity() {
    let mut units = Vec::with_capacity(1_000_000);
    units.push(65);
    let mut report = FailureReport::fault(FailureCode::Capacity, "");
    report.message = Some(JsString::from_units(units));
    let bounded = report.bounded();
    let message = bounded.message.unwrap();
    assert_eq!(message.as_units(), &[65]);
    assert!(
        message.capacity_bytes() <= 8192,
        "short text must not retain a huge native allocation"
    );
    let long = FailureReport::new(
        FailureClass::Script,
        FailureCode::ScriptException,
        &"x".repeat(40_000),
    );
    assert!(long.message.unwrap().capacity_bytes() <= 8192);
}
