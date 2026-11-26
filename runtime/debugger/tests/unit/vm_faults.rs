//! Faulting dependencies over the real Rust fixture. Successful operations delegate
//! to its ValueStore, and failures throw actual scope-protected Rust values. This
//! exercises facade error/capture contracts, not JavaScript or native-debug conformance.
use super::*;
use crate::debug::*;
use contract_fixture::{FixtureProbe, Inline, RustFixture};
use std::{num::NonZeroUsize, rc::Weak, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScopeFault {
    None,
    HostFunction,
    Global,
    Set,
    Evaluate,
    Artifact,
    Local,
    Kind,
    Import,
    ImportSecond,
    SourceMissing,
    WithScope,
    Close,
    ClosePanic,
    Compile,
    Checkpoint,
    DebugBackend,
    DebugService,
    CreateContext,
    Settle,
    DebugDetach,
}
#[derive(Clone, Copy)]
pub(crate) enum AttachFault {
    Unsupported,
    Capacity,
    Panic,
    Accept,
}
pub(crate) struct Control {
    pub scope: Rc<Cell<ScopeFault>>,
    pub attach: Rc<Cell<AttachFault>>,
    pub attach_calls: Rc<Cell<usize>>,
    pub program_calls: Rc<Cell<usize>>,
    native: Weak<RustFixture<Inline>>,
    context: Rc<Cell<Option<ContextId>>>,
}
impl Control {
    pub fn close_native(&self) {
        self.native
            .upgrade()
            .unwrap()
            .close_context(self.context.get().unwrap())
            .unwrap();
    }
}
pub(crate) struct FaultEngine {
    inner: Rc<RustFixture<Inline>>,
    scope: Rc<Cell<ScopeFault>>,
    backend: Rc<Backend>,
    context: Rc<Cell<Option<ContextId>>>,
}
struct Program(Rc<Cell<usize>>);
impl HostFunction for Program {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.0.set(self.0.get() + 1);
        call.import_data(JsData::Number(42.0))
    }
}
pub(crate) fn source() -> ScriptSource {
    ScriptSource::new(
        JsString::from_utf8("registered-rust-program"),
        "rust-fixture://program".into(),
    )
}
pub(crate) fn options(info: DebugInfo) -> CompileOptions {
    CompileOptions {
        mode: CodeMode::Script,
        debug_info: info,
        fingerprint: "rust-program-v1".into(),
    }
}
type Created = (FaultEngine, Control, FixtureProbe<Inline>);
impl FaultEngine {
    pub fn new(programs: bool) -> Created {
        Self::with_owner(programs, None)
    }
    pub fn with_owner(programs: bool, owner: Option<ThreadId>) -> Created {
        let calls = Rc::new(Cell::new(0));
        let mut inner = RustFixture::<Inline>::new("named-vm-fault-wrapper");
        if programs {
            inner = inner.with_programs(vec![(source(), Rc::new(Program(calls.clone())))]);
        }
        if let Some(owner) = owner {
            inner = inner.with_reported_owner(owner);
        }
        let probe = inner.probe();
        let inner = Rc::new(inner);
        let scope = Rc::new(Cell::new(ScopeFault::None));
        let attach = Rc::new(Cell::new(AttachFault::Unsupported));
        let attach_calls = Rc::new(Cell::new(0));
        let context = Rc::new(Cell::new(None));
        let backend = Rc::new(Backend {
            descriptor: DebugDescriptor {
                api_version: 1,
                capabilities: DebugCapabilities::baseline(),
                native_requirements: vec![
                    "deliberately failing Rust attachment; no native qualification".into(),
                ],
                interrupt_mechanism: "none: attachment always rejects or panics".into(),
            },
            mode: attach.clone(),
            fault: scope.clone(),
            calls: attach_calls.clone(),
            port: RefCell::new(None),
        });
        let control = Control {
            scope: scope.clone(),
            attach,
            attach_calls,
            program_calls: calls,
            native: Rc::downgrade(&inner),
            context: context.clone(),
        };
        (
            Self {
                inner,
                scope,
                backend,
                context,
            },
            control,
            probe,
        )
    }
}
impl JsEngine for FaultEngine {
    fn descriptor(&self) -> &EngineDescriptor {
        self.inner.descriptor()
    }
    fn create_context(&self, limits: ContextLimits) -> Result<ContextId, FailureReport> {
        if self.scope.get() == ScopeFault::CreateContext {
            return Err(FailureReport::fault(
                FailureCode::EngineFailure,
                "adapter context creation failed",
            ));
        }
        let id = self.inner.create_context(limits)?;
        self.context.set(Some(id));
        Ok(id)
    }
    fn close_context(&self, id: ContextId) -> Result<(), FailureReport> {
        self.inner.close_context(id)?;
        assert!(
            self.scope.get() != ScopeFault::ClosePanic,
            "native close destructor reporting panic"
        );
        if self.scope.get() == ScopeFault::Close {
            return Err(FailureReport::new(
                FailureClass::Engine,
                FailureCode::EngineFailure,
                "adapter close failed after real context cleanup",
            ));
        }
        Ok(())
    }
    fn debug_backend(&self, _: ContextId) -> Result<Option<Rc<dyn DebugBackend>>, FailureReport> {
        if self.scope.get() == ScopeFault::DebugBackend {
            return Err(FailureReport::fault(
                FailureCode::EngineFailure,
                "adapter backend discovery failed",
            ));
        }
        Ok(Some(self.backend.clone()))
    }
    fn with_scope(
        &self,
        id: ContextId,
        host: &dyn HostServices,
        body: &mut dyn ScopeCall,
    ) -> Result<(), FailureReport> {
        if self.scope.get() == ScopeFault::WithScope {
            return Err(FailureReport::new(
                FailureClass::Engine,
                FailureCode::EngineFailure,
                "adapter scope entry failed",
            ));
        }
        self.inner.with_scope(id, host, &mut |scope: &dyn JsScope| {
            body.run(&FaultScope {
                inner: scope,
                fault: self.scope.get(),
                imports: Cell::new(0),
            })
        })
    }
}
struct Backend {
    descriptor: DebugDescriptor,
    mode: Rc<Cell<AttachFault>>,
    fault: Rc<Cell<ScopeFault>>,
    calls: Rc<Cell<usize>>,
    port: RefCell<Option<Rc<dyn DebugPort>>>,
}
impl DebugBackend for Backend {
    fn descriptor(&self) -> &DebugDescriptor {
        &self.descriptor
    }
    fn initialize(&self, port: Rc<dyn DebugPort>) -> Result<(), DebugError> {
        *self.port.borrow_mut() = Some(port);
        Ok(())
    }
    fn attach(&self, _: DebugAttachOptions) -> Result<Arc<dyn DebugSignal>, DebugError> {
        self.calls.set(self.calls.get() + 1);
        assert_eq!(
            self.port.borrow().as_ref().unwrap().identities().state(),
            DebugExecutionState::Running,
            "coordinator must issue the real session before attachment"
        );
        match self.mode.get() {
            AttachFault::Unsupported => Err(DebugError::Unsupported),
            AttachFault::Capacity => Err(DebugError::Capacity),
            AttachFault::Panic => panic!("adapter attachment panic"),
            AttachFault::Accept => Ok(Arc::new(Signal)),
        }
    }
    fn service(&self) -> Result<DebugServiceReport, DebugError> {
        if self.fault.get() == ScopeFault::DebugService {
            return Err(DebugError::EngineFailure(FailureReport::fault(
                FailureCode::EngineFailure,
                "adapter debug service failed",
            )));
        }
        Ok(DebugServiceReport {
            made_progress: false,
            state: self.port.borrow().as_ref().unwrap().identities().state(),
        })
    }
    fn detach(&self) -> Result<(), DebugError> {
        self.port.borrow_mut().take();
        if self.fault.get() == ScopeFault::DebugDetach {
            Err(DebugError::Unsupported)
        } else {
            Ok(())
        }
    }
}
struct Signal;
impl DebugSignal for Signal {
    fn wake(&self) -> Result<(), DebugError> {
        Ok(())
    }
}

struct FaultScope<'a> {
    inner: &'a dyn JsScope,
    fault: ScopeFault,
    imports: Cell<usize>,
}
impl FaultScope<'_> {
    fn exception<'s>(&'s self, label: &str) -> ScopedError<'s> {
        match self
            .inner
            .import_data(JsData::String(JsString::from_utf8(label)))
        {
            Ok(value) => ScopedError::Exception(value),
            Err(error) => error,
        }
    }
    fn assert_source(&self, input: &ScriptSource) {
        assert_eq!(input.text, source().text);
        assert_eq!(input.origin, source().origin);
        assert!(input.source_map.is_none());
    }
}
impl JsScope for FaultScope<'_> {
    fn data_limits(&self) -> ExportLimits {
        self.inner.data_limits()
    }
    fn context(&self) -> ContextId {
        self.inner.context()
    }
    fn global<'s>(&'s self) -> JsResult<'s, ScopedValue<'s>> {
        if self.fault == ScopeFault::Global {
            Err(self.exception("global access failed"))
        } else {
            self.inner.global()
        }
    }
    fn kind(&self, value: ScopedValue<'_>) -> Result<ValueKind, FailureReport> {
        if self.fault == ScopeFault::Kind {
            return Err(FailureReport::new(
                FailureClass::Engine,
                FailureCode::EngineFailure,
                "adapter value-kind lookup failed",
            ));
        }
        self.inner.kind(value)
    }
    fn local<'s>(&'s self, value: &RootedValue) -> JsResult<'s, ScopedValue<'s>> {
        if self.fault == ScopeFault::Local {
            return Err(FailureReport::fault(
                FailureCode::InvalidHandle,
                "adapter root localization failed",
            )
            .into());
        }
        self.inner.local(value)
    }
    fn retain(&self, value: ScopedValue<'_>) -> Result<RootedValue, FailureReport> {
        self.inner.retain(value)
    }
    fn import_data<'s>(&'s self, value: JsData) -> JsResult<'s, ScopedValue<'s>> {
        self.imports.set(self.imports.get() + 1);
        if self.fault == ScopeFault::Import
            || self.fault == ScopeFault::ImportSecond && self.imports.get() == 2
        {
            return Err(self.exception("data import failed"));
        }
        self.inner.import_data(value)
    }
    fn export_data<'s>(
        &'s self,
        value: ScopedValue<'s>,
        limits: ExportLimits,
    ) -> JsResult<'s, JsData> {
        self.inner.export_data(value, limits)
    }
    fn get<'s>(
        &'s self,
        object: ScopedValue<'s>,
        key: PropertyKey<'_>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.get(object, key)
    }
    fn set<'s>(
        &'s self,
        object: ScopedValue<'s>,
        key: PropertyKey<'_>,
        value: ScopedValue<'s>,
    ) -> JsResult<'s, ()> {
        if self.fault == ScopeFault::Set {
            assert!(
                matches!(key, PropertyKey::String(key) if key == &JsString::from_utf8("fault_target"))
            );
            Err(self.exception("global assignment failed"))
        } else {
            self.inner.set(object, key, value)
        }
    }
    fn call<'s>(
        &'s self,
        function: ScopedValue<'s>,
        receiver: Option<ScopedValue<'s>>,
        args: &[ScopedValue<'s>],
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.call(function, receiver, args)
    }
    fn construct<'s>(
        &'s self,
        function: ScopedValue<'s>,
        args: &[ScopedValue<'s>],
        target: Option<ScopedValue<'s>>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.construct(function, args, target)
    }
    fn host_function<'s>(
        &'s self,
        name: &JsString,
        arity: u16,
        function: Rc<dyn HostFunction>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        if self.fault == ScopeFault::HostFunction {
            assert_eq!(name, &JsString::from_utf8("fault_target"));
            assert_eq!(arity, 0);
            Err(self.exception("host registration failed"))
        } else {
            self.inner.host_function(name, arity, function)
        }
    }
    fn deferred<'s>(&'s self) -> JsResult<'s, Deferred<'s>> {
        self.inner.deferred()
    }
    fn settle<'s>(&'s self, resolver: RootedDeferred, value: Settlement<'s>) -> JsResult<'s, ()> {
        if self.fault == ScopeFault::Settle {
            return Err(self.exception("native settlement failed"));
        }
        self.inner.settle(resolver, value)
    }
    fn drain_microtasks(&self, hint: NonZeroUsize) -> Result<CheckpointReport, FailureReport> {
        if self.fault == ScopeFault::Checkpoint {
            return Err(FailureReport::fault(
                FailureCode::EngineFailure,
                "adapter checkpoint failed",
            ));
        }
        self.inner.drain_microtasks(hint)
    }
    fn detach(&self, error: ScopedError<'_>) -> FailureReport {
        self.inner.detach(error)
    }
    fn source(&self) -> Option<&dyn SourceApi> {
        if self.fault == ScopeFault::SourceMissing {
            return None;
        }
        self.inner.source().map(|_| self as &dyn SourceApi)
    }
    fn compiler(&self) -> Option<&dyn CompilerApi> {
        self.inner.compiler().map(|_| self as &dyn CompilerApi)
    }
    fn artifacts(&self) -> Option<&dyn ArtifactApi> {
        self.inner.artifacts().map(|_| self as &dyn ArtifactApi)
    }
    fn modules(&self) -> Option<&dyn ModuleApi> {
        self.inner.modules()
    }
}
impl CompilerApi for FaultScope<'_> {
    fn compile(
        &self,
        source: &ScriptSource,
        options: CompileOptions,
    ) -> Result<CompiledArtifact, FailureReport> {
        if self.fault == ScopeFault::Compile {
            self.assert_source(source);
            return Err(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "adapter compilation failed",
            ));
        }
        self.inner.compiler().unwrap().compile(source, options)
    }
}
impl SourceApi for FaultScope<'_> {
    fn evaluate<'s>(&'s self, input: &ScriptSource) -> JsResult<'s, ScopedValue<'s>> {
        if self.fault == ScopeFault::Evaluate {
            self.assert_source(input);
            Err(self.exception("source entry failed"))
        } else {
            self.inner.source().unwrap().evaluate(input)
        }
    }
}
impl ArtifactApi for FaultScope<'_> {
    fn execute<'s>(&'s self, artifact: &CompiledArtifact) -> JsResult<'s, ScopedValue<'s>> {
        if self.fault == ScopeFault::Artifact {
            self.assert_source(artifact.source());
            assert!(artifact.payload().is_empty());
            Err(self.exception("artifact entry failed"))
        } else {
            self.inner.artifacts().unwrap().execute(artifact)
        }
    }
}
