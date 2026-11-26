//! Rust-only call-graph fixtures. Optional named Rust programs exercise source
//! dispatch without parsing JavaScript or loading foreign bytecode. No native GC
//! or interactive debugger is simulated or advertised.
// Each integration binary uses a different subset of these shared Rust fixtures.
#![allow(dead_code)]
use coldmind_runtime::{debug::*, engine::*};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    num::NonZeroUsize,
    rc::{Rc, Weak},
    sync::Arc,
    time::Duration,
};

pub trait Representation: Sized + 'static {
    fn encode(value: RustValue<Self>) -> Self;
    fn value(&self) -> &RustValue<Self>;
}
pub struct Inline(RustValue<Inline>);
pub struct Boxed(Box<RustValue<Boxed>>);
impl Representation for Inline {
    fn encode(v: RustValue<Self>) -> Self {
        Self(v)
    }
    fn value(&self) -> &RustValue<Self> {
        &self.0
    }
}
impl Representation for Boxed {
    fn encode(v: RustValue<Self>) -> Self {
        Self(Box::new(v))
    }
    fn value(&self) -> &RustValue<Self> {
        &self.0
    }
}
pub enum RustValue<R: Representation> {
    Data(JsData),
    Object(RefCell<Vec<(JsString, Rc<R>)>>),
    Function(Rc<dyn HostFunction>),
    Promise(RefCell<Option<(bool, Rc<R>)>>),
}
type CheckpointWork = Box<dyn FnOnce(&dyn HostServices) -> Result<(), FailureReport>>;
type Programs = Rc<Vec<(ScriptSource, Rc<dyn HostFunction>)>>;
type ModuleSpecs = Rc<Vec<(JsString, Vec<JsString>)>>;
struct FixtureModule<R: Representation> {
    source: ScriptSource,
    identity: JsString,
    imports: Vec<JsString>,
    bindings: Option<Vec<ModuleBinding>>,
    namespace: Rc<R>,
    evaluated: bool,
}

struct Context<R: Representation> {
    values: ValueStore<R>,
    global: Rc<R>,
    heap: RefCell<Vec<Weak<R>>>,
    export_limits: ExportLimits,
    invalid_resolver: bool,
    programs: Option<Programs>,
    module_specs: Option<ModuleSpecs>,
    module_images: bool,
    failing_image_compile: bool,
    failing_module_imports: bool,
    dynamic_modules: bool,
    image_only_modules: bool,
    descriptor: EngineDescriptor,
    modules: RefCell<std::collections::HashMap<u64, FixtureModule<R>>>,
    module_requests: RefCell<VecDeque<ModuleImportRequest>>,
    module_resolvers: RefCell<std::collections::HashMap<u64, RootedDeferred>>,
    next_import: Cell<u64>,
    checkpoint_work: RefCell<VecDeque<CheckpointWork>>,
    checkpoint_calls: Rc<Cell<usize>>,
    compile_calls: Rc<Cell<usize>>,
    bind_calls: Rc<Cell<usize>>,
}
impl<R: Representation> Context<R> {
    fn allocate(&self, value: RustValue<R>) -> Rc<R> {
        let value = Rc::new(R::encode(value));
        self.heap.borrow_mut().push(Rc::downgrade(&value));
        value
    }
    fn clear_values(&self) {
        let work = std::mem::take(&mut *self.checkpoint_work.borrow_mut());
        drop(work);
        let heap = std::mem::take(&mut *self.heap.borrow_mut());
        for value in heap.into_iter().filter_map(|v| v.upgrade()) {
            match value.value() {
                RustValue::Object(entries) => entries.borrow_mut().clear(),
                RustValue::Promise(state) => {
                    state.borrow_mut().take();
                }
                _ => {}
            }
        }
    }
}
impl<R: Representation> Drop for Context<R> {
    fn drop(&mut self) {
        let _ = self.values.close();
        self.clear_values();
    }
}
/// Runs `body`, calling `stop` if it is still running after 5 s, so a hang fails the
/// caller's assertions instead of stalling the suite.
pub fn with_watchdog<T>(stop: impl FnOnce() + Send + 'static, body: impl FnOnce() -> T) -> T {
    let (done, finished) = std::sync::mpsc::sync_channel::<()>(1);
    let watchdog = std::thread::spawn(move || {
        if finished
            .recv_timeout(std::time::Duration::from_secs(5))
            .is_err()
        {
            stop();
        }
    });
    let output = body();
    let _ = done.send(());
    watchdog.join().unwrap();
    output
}
pub struct RustFixture<R: Representation> {
    identity: EngineIdentity,
    descriptor: EngineDescriptor,
    context: Rc<RefCell<Option<Rc<Context<R>>>>>,
    created: Rc<Cell<usize>>,
    closed: Rc<Cell<usize>>,
    invalid_resolver: bool,
    programs: Option<Programs>,
    module_specs: Option<ModuleSpecs>,
    module_images: bool,
    failing_image_compile: bool,
    failing_module_imports: bool,
    dynamic_modules: bool,
    image_only_modules: bool,
    checkpoint_calls: Rc<Cell<usize>>,
    compile_calls: Rc<Cell<usize>>,
    bind_calls: Rc<Cell<usize>>,
}

/// Weak observer of actual Rust fixture state; retains no context/native values.
pub struct FixtureProbe<R: Representation> {
    context: Weak<RefCell<Option<Rc<Context<R>>>>>,
    created: Rc<Cell<usize>>,
    closed: Rc<Cell<usize>>,
    checkpoint_calls: Rc<Cell<usize>>,
    compile_calls: Rc<Cell<usize>>,
    bind_calls: Rc<Cell<usize>>,
}
impl<R: Representation> Clone for FixtureProbe<R> {
    fn clone(&self) -> Self {
        Self {
            context: self.context.clone(),
            created: self.created.clone(),
            closed: self.closed.clone(),
            checkpoint_calls: self.checkpoint_calls.clone(),
            compile_calls: self.compile_calls.clone(),
            bind_calls: self.bind_calls.clone(),
        }
    }
}
impl<R: Representation> FixtureProbe<R> {
    /// Queues actual Rust checkpoint work; these are not JavaScript Promise reactions.
    /// Creates an actual Rust deferred and enqueues native-operation metadata, without parsing JS.
    pub fn module_import(
        &self,
        referrer: JsString,
        specifier: JsString,
    ) -> (ModuleImportId, RootedValue) {
        let context = self
            .context
            .upgrade()
            .unwrap()
            .borrow()
            .as_ref()
            .unwrap()
            .clone();
        context
            .values
            .with_scope(|values| {
                let scope = NativeScope {
                    context: &context,
                    values,
                    host: &NoLoop,
                };
                let deferred = scope.deferred().map_err(|e| scope.detach(e))?;
                let promise = scope.retain(deferred.promise)?;
                let sequence = context.next_import.get() + 1;
                context.next_import.set(sequence);
                let id = ModuleImportId {
                    context: context.values.context(),
                    sequence,
                };
                context
                    .module_resolvers
                    .borrow_mut()
                    .insert(sequence, deferred.resolver);
                context
                    .module_requests
                    .borrow_mut()
                    .push_back(ModuleImportRequest {
                        id,
                        referrer: referrer.clone(),
                        specifier: specifier.clone(),
                    });
                Ok((id, promise))
            })
            .unwrap()
    }
    pub fn queue_checkpoint(
        &self,
        work: impl FnOnce(&dyn HostServices) -> Result<(), FailureReport> + 'static,
    ) -> Result<(), FailureReport> {
        let cell = self.context.upgrade().ok_or_else(|| {
            FailureReport::fault(FailureCode::ClosedContext, "fixture owner dropped")
        })?;
        let context = cell.borrow().clone().ok_or_else(|| {
            FailureReport::fault(FailureCode::ClosedContext, "fixture context closed")
        })?;
        context
            .checkpoint_work
            .borrow_mut()
            .push_back(Box::new(work));
        Ok(())
    }
    pub fn checkpoint_calls(&self) -> usize {
        self.checkpoint_calls.get()
    }
    pub fn compile_calls(&self) -> usize {
        self.compile_calls.get()
    }
    /// Native bind/link calls made by the VM.
    pub fn bind_calls(&self) -> usize {
        self.bind_calls.get()
    }
    /// Native module records held by the live context, including ones never bound.
    pub fn module_records(&self) -> usize {
        self.context
            .upgrade()
            .and_then(|c| c.borrow().as_ref().map(|c| c.modules.borrow().len()))
            .unwrap_or(0)
    }
    pub fn contexts_created(&self) -> usize {
        self.created.get()
    }
    pub fn contexts_closed(&self) -> usize {
        self.closed.get()
    }
    pub fn is_context_open(&self) -> bool {
        self.context
            .upgrade()
            .is_some_and(|cell| cell.borrow().is_some())
    }
    pub fn live_roots(&self) -> Option<usize> {
        self.context.upgrade().and_then(|cell| {
            cell.borrow()
                .as_ref()
                .map(|context| context.values.live_roots())
        })
    }
    /// Reads the real RustValue::Promise state through its ValueStore token.
    pub fn promise(&self, root: &RootedValue) -> Result<Option<(bool, JsData)>, FailureReport> {
        let cell = self.context.upgrade().ok_or_else(|| {
            FailureReport::fault(FailureCode::ClosedContext, "fixture owner dropped")
        })?;
        let context = cell.borrow().clone().ok_or_else(|| {
            FailureReport::fault(FailureCode::ClosedContext, "fixture context closed")
        })?;
        context.values.with_scope(|scope| {
            let native = scope.read(scope.local(root)?)?;
            match native.value() {
                RustValue::Promise(state) => state
                    .borrow()
                    .as_ref()
                    .map(|(rejected, value)| {
                        NativeScope::export(value.clone(), ExportLimits::default())
                            .map(|value| (*rejected, value))
                    })
                    .transpose(),
                _ => Err(FailureReport::fault(
                    FailureCode::InvalidHandle,
                    "probe requires a real fixture Promise",
                )),
            }
        })
    }
}
impl<R: Representation> RustFixture<R> {
    pub fn new(name: &str) -> Self {
        let identity = EngineIdentity::new().unwrap();
        let descriptor = EngineDescriptor {
            adapter_id: name.into(),
            adapter_version: "test".into(),
            native_version: "none".into(),
            contract_version: 1,
            language_features: "Rust callables only; no JavaScript/native GC".into(),
            target_fingerprint: "rust-test".into(),
            build_fingerprint: "rust-test".into(),
            native_build_requirements: Vec::new(),
            owner: identity.owner(),
        };
        Self {
            identity,
            descriptor,
            context: Rc::new(RefCell::new(None)),
            created: Rc::new(Cell::new(0)),
            closed: Rc::new(Cell::new(0)),
            invalid_resolver: false,
            programs: None,
            module_specs: None,
            module_images: false,
            failing_image_compile: false,
            failing_module_imports: false,
            dynamic_modules: false,
            image_only_modules: false,
            checkpoint_calls: Rc::new(Cell::new(0)),
            compile_calls: Rc::new(Cell::new(0)),
            bind_calls: Rc::new(Cell::new(0)),
        }
    }
    /// Enables a source-only Rust module fixture. Names and requests are explicit test inputs,
    /// not a JavaScript parser or native adapter qualification.
    pub fn with_modules(mut self, specs: Vec<(JsString, Vec<JsString>)>) -> Self {
        self.module_specs = Some(Rc::new(specs));
        self
    }
    /// Enables an opaque Rust fixture opcode image, independent of retained source text.
    pub fn with_module_images(mut self) -> Self {
        self.module_images = true;
        self
    }
    /// Image compilation fails after a successful prepare, for compile-failure cleanup tests.
    pub fn with_failing_image_compile(mut self) -> Self {
        self.module_images = true;
        self.failing_image_compile = true;
        self
    }
    /// Fault fixture: reading a prepared module's imports reports a fatal engine failure.
    pub fn with_failing_module_imports(mut self) -> Self {
        self.failing_module_imports = true;
        self
    }
    /// Fault fixture: source preparation is unavailable while genuine fixture image loading works.
    pub fn with_image_only_modules(mut self) -> Self {
        self.module_images = true;
        self.image_only_modules = true;
        self
    }
    /// Enables owner-local native request tokens for platform scheduling tests.
    pub fn with_dynamic_modules(mut self) -> Self {
        self.dynamic_modules = true;
        self
    }
    /// Creates an independent compile-only fixture with the same module declarations.
    /// It shares no context, probes, native handles or application bodies.
    pub fn compiler_fixture(&self) -> Self {
        let mut compiler = Self::new(&self.descriptor.adapter_id);
        compiler.module_specs = self.module_specs.clone();
        compiler.module_images = self.module_images;
        compiler
    }
    pub fn probe(&self) -> FixtureProbe<R> {
        FixtureProbe {
            context: Rc::downgrade(&self.context),
            created: self.created.clone(),
            closed: self.closed.clone(),
            checkpoint_calls: self.checkpoint_calls.clone(),
            compile_calls: self.compile_calls.clone(),
            bind_calls: self.bind_calls.clone(),
        }
    }
    /// Deliberately inconsistent adapter metadata for constructor owner validation.
    pub fn with_reported_owner(mut self, owner: std::thread::ThreadId) -> Self {
        self.descriptor.owner = owner;
        self
    }
    /// Adapter-fault fixture: deferred() retains Null as resolver while returning
    /// an actual Promise. This tests settle's real type rejection, not JS semantics.
    pub fn with_invalid_resolver(mut self) -> Self {
        self.invalid_resolver = true;
        self
    }
    /// Enables exact named-source lookup of registered Rust HostFunctions.
    /// No JavaScript text is parsed and empty artifacts are never deserialized.
    pub fn with_programs(mut self, programs: Vec<(ScriptSource, Rc<dyn HostFunction>)>) -> Self {
        self.programs = Some(Rc::new(programs));
        self
    }
    fn find(&self, context: ContextId) -> Result<Rc<Context<R>>, FailureReport> {
        self.context
            .borrow()
            .as_ref()
            .filter(|c| c.values.context() == context)
            .cloned()
            .ok_or_else(|| {
                FailureReport::fault(FailureCode::ClosedContext, "fixture context closed")
            })
    }
}
impl<R: Representation> JsEngine for RustFixture<R> {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }
    fn create_context(&self, limits: ContextLimits) -> Result<ContextId, FailureReport> {
        if self.context.borrow().is_some() {
            return Err(FailureReport::fault(
                FailureCode::ActiveContext,
                "one fixture context",
            ));
        }
        let values = ValueStore::new(&self.identity, limits)?;
        let id = values.context();
        let global = Rc::new(R::encode(RustValue::Object(RefCell::new(Vec::new()))));
        let heap = RefCell::new(vec![Rc::downgrade(&global)]);
        *self.context.borrow_mut() = Some(Rc::new(Context {
            values,
            global,
            heap,
            export_limits: limits.export,
            invalid_resolver: self.invalid_resolver,
            programs: self.programs.clone(),
            module_specs: self.module_specs.clone(),
            module_images: self.module_images,
            failing_image_compile: self.failing_image_compile,
            failing_module_imports: self.failing_module_imports,
            dynamic_modules: self.dynamic_modules,
            image_only_modules: self.image_only_modules,
            module_requests: RefCell::new(VecDeque::new()),
            module_resolvers: RefCell::new(std::collections::HashMap::new()),
            next_import: Cell::new(0),
            modules: RefCell::new(std::collections::HashMap::new()),
            descriptor: self.descriptor.clone(),
            checkpoint_work: RefCell::new(VecDeque::new()),
            checkpoint_calls: self.checkpoint_calls.clone(),
            compile_calls: self.compile_calls.clone(),
            bind_calls: self.bind_calls.clone(),
        }));
        self.created.set(self.created.get() + 1);
        Ok(id)
    }
    fn close_context(&self, id: ContextId) -> Result<(), FailureReport> {
        let context = self.find(id)?;
        context.values.close()?;
        context.clear_values();
        self.context.borrow_mut().take();
        self.closed.set(self.closed.get() + 1);
        Ok(())
    }
    fn debug_backend(&self, id: ContextId) -> Result<Option<Rc<dyn DebugBackend>>, FailureReport> {
        // This exercises metadata lifetime only; its descriptor advertises no
        // debugging feature and Development requirements therefore reject it.
        let context = self.find(id)?;
        Ok(Some(Rc::new(LifecycleBackend {
            context: Rc::downgrade(&context),
            descriptor: DebugDescriptor {
                api_version: 1,
                capabilities: DebugCapabilities::empty(),
                native_requirements: vec!["lifecycle fixture only".into()],
                interrupt_mechanism: "none".into(),
            },
        })))
    }
    fn with_scope(
        &self,
        id: ContextId,
        host: &dyn HostServices,
        body: &mut dyn ScopeCall,
    ) -> Result<(), FailureReport> {
        let context = self.find(id)?;
        let result = context.values.with_scope(|values| {
            body.run(&NativeScope {
                context: &context,
                values,
                host,
            })
        });
        if let Err(error) = context.values.flush_releases()
            && error.code != FailureCode::ActiveContext
        {
            return Err(error);
        }
        result
    }
}
struct LifecycleBackend<R: Representation> {
    context: Weak<Context<R>>,
    descriptor: DebugDescriptor,
}
impl<R: Representation> DebugBackend for LifecycleBackend<R> {
    fn descriptor(&self) -> &DebugDescriptor {
        &self.descriptor
    }
    fn initialize(&self, _: Rc<dyn DebugPort>) -> Result<(), DebugError> {
        Err(DebugError::Unsupported)
    }
    fn attach(&self, _: DebugAttachOptions) -> Result<Arc<dyn DebugSignal>, DebugError> {
        Err(DebugError::Unsupported)
    }
    fn service(&self) -> Result<DebugServiceReport, DebugError> {
        match self.context.upgrade() {
            Some(context) if !context.values.is_closed() => Ok(DebugServiceReport {
                made_progress: false,
                state: DebugExecutionState::Detached,
            }),
            _ => Err(DebugError::Closed),
        }
    }
    fn detach(&self) -> Result<(), DebugError> {
        self.service().map(|_| ())
    }
}

struct NativeScope<'a, R: Representation> {
    context: &'a Context<R>,
    values: &'a ValueScope<'a, R>,
    host: &'a dyn HostServices,
}
struct Invocation<'a, R: Representation> {
    inner: NativeScope<'a, R>,
    receiver: ScopedValue<'a>,
    arguments: Vec<ScopedValue<'a>>,
}
impl<R: Representation> NativeScope<'_, R> {
    fn put<'s>(&'s self, value: RustValue<R>) -> JsResult<'s, ScopedValue<'s>> {
        self.values
            .insert_shared(self.context.allocate(value))
            .map_err(Into::into)
    }
    fn import(&self, value: &JsData) -> Rc<R> {
        let native = match value {
            JsData::Record(entries) => RustValue::Object(RefCell::new(
                entries
                    .iter()
                    .map(|(key, value)| (key.clone(), self.import(value)))
                    .collect(),
            )),
            _ => RustValue::Data(value.clone()),
        };
        self.context.allocate(native)
    }
    fn export(value: Rc<R>, limits: ExportLimits) -> Result<JsData, FailureReport> {
        enum Work<R> {
            Visit(Rc<R>, usize),
            Object(Vec<JsString>),
        }
        let fail = || FailureReport::fault(FailureCode::Capacity, "fixture export limits exceeded");
        let mut work = vec![Work::Visit(value, 1)];
        let mut output = Vec::new();
        let mut path = Vec::new();
        let mut nodes = 0usize;
        let mut bytes = std::mem::size_of::<JsData>();
        while let Some(item) = work.pop() {
            match item {
                Work::Visit(value, depth) => {
                    nodes = nodes.checked_add(1).ok_or_else(fail)?;
                    if depth > limits.max_depth || nodes > limits.max_nodes {
                        return Err(fail());
                    }
                    match value.value() {
                        RustValue::Data(data) => {
                            let owned = data
                                .footprint(limits)?
                                .saturating_sub(std::mem::size_of::<JsData>());
                            bytes = bytes.checked_add(owned).ok_or_else(fail)?;
                            if bytes > limits.max_bytes {
                                return Err(fail());
                            }
                            output.push(data.clone());
                        }
                        RustValue::Object(entries) => {
                            let address = Rc::as_ptr(&value);
                            if path.contains(&address) {
                                return Err(FailureReport::fault(
                                    FailureCode::UnsupportedConversion,
                                    "cyclic fixture object",
                                ));
                            }
                            let entries = entries.borrow();
                            if entries.len() > limits.max_nodes.saturating_sub(nodes) {
                                return Err(fail());
                            }
                            let mut extra = entries
                                .len()
                                .checked_mul(std::mem::size_of::<(JsString, JsData)>())
                                .ok_or_else(fail)?;
                            for (key, _) in entries.iter() {
                                extra = extra
                                    .checked_add(
                                        key.as_units().len().checked_mul(2).ok_or_else(fail)?,
                                    )
                                    .ok_or_else(fail)?;
                            }
                            bytes = bytes.checked_add(extra).ok_or_else(fail)?;
                            if bytes > limits.max_bytes {
                                return Err(fail());
                            }
                            path.push(address);
                            work.push(Work::Object(
                                entries.iter().map(|(key, _)| key.clone()).collect(),
                            ));
                            work.extend(
                                entries
                                    .iter()
                                    .rev()
                                    .map(|(_, value)| Work::Visit(value.clone(), depth + 1)),
                            );
                        }
                        _ => {
                            return Err(FailureReport::fault(
                                FailureCode::UnsupportedConversion,
                                "fixture value is not copied data",
                            ));
                        }
                    }
                }
                Work::Object(keys) => {
                    let values = output.split_off(output.len() - keys.len());
                    output.push(JsData::Record(keys.into_iter().zip(values).collect()));
                    path.pop();
                }
            }
        }
        let data = output.pop().expect("one complete export root");
        data.footprint(limits)?;
        Ok(data)
    }
    fn type_error<'s>(&'s self) -> ScopedError<'s> {
        match self.put(RustValue::Data(JsData::String(JsString::from_utf8(
            "not callable/object",
        )))) {
            Ok(value) => ScopedError::Exception(value),
            Err(error) => error,
        }
    }
}
impl<R: Representation> JsScope for NativeScope<'_, R> {
    fn data_limits(&self) -> ExportLimits {
        self.context.export_limits
    }
    fn context(&self) -> ContextId {
        self.context.values.context()
    }
    fn global<'s>(&'s self) -> JsResult<'s, ScopedValue<'s>> {
        self.values
            .insert_shared(self.context.global.clone())
            .map_err(Into::into)
    }
    fn kind(&self, value: ScopedValue<'_>) -> Result<ValueKind, FailureReport> {
        Ok(match self.values.read(value)?.value() {
            RustValue::Data(JsData::Undefined) => ValueKind::Undefined,
            RustValue::Data(JsData::Null) => ValueKind::Null,
            RustValue::Data(JsData::Bool(_)) => ValueKind::Boolean,
            RustValue::Data(JsData::Number(_)) => ValueKind::Number,
            RustValue::Data(JsData::String(_)) => ValueKind::String,
            RustValue::Function(_) => ValueKind::Function,
            _ => ValueKind::Object,
        })
    }
    fn local<'s>(&'s self, root: &RootedValue) -> JsResult<'s, ScopedValue<'s>> {
        self.values.local(root).map_err(Into::into)
    }
    fn retain(&self, value: ScopedValue<'_>) -> Result<RootedValue, FailureReport> {
        self.values.retain(value)
    }
    fn import_data<'s>(&'s self, data: JsData) -> JsResult<'s, ScopedValue<'s>> {
        data.footprint(self.context.export_limits)?;
        self.values
            .insert_shared(self.import(&data))
            .map_err(Into::into)
    }
    fn export_data<'s>(
        &'s self,
        value: ScopedValue<'s>,
        limits: ExportLimits,
    ) -> JsResult<'s, JsData> {
        let native = self.values.read(value)?;
        let data = Self::export(native, limits)?;
        data.footprint(limits)?;
        Ok(data)
    }
    fn get<'s>(
        &'s self,
        object: ScopedValue<'s>,
        key: PropertyKey<'_>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        let PropertyKey::String(key) = key else {
            return Err(
                FailureReport::fault(FailureCode::Unsupported, "fixture has no symbols").into(),
            );
        };
        let value = self.values.read(object)?;
        let RustValue::Object(entries) = value.value() else {
            return Err(self.type_error());
        };
        let result = entries
            .borrow()
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, v)| v.clone());
        match result {
            Some(value) => self.values.insert_shared(value).map_err(Into::into),
            None => self.import_data(JsData::Undefined),
        }
    }
    fn set<'s>(
        &'s self,
        object: ScopedValue<'s>,
        key: PropertyKey<'_>,
        value: ScopedValue<'s>,
    ) -> JsResult<'s, ()> {
        let PropertyKey::String(key) = key else {
            return Err(
                FailureReport::fault(FailureCode::Unsupported, "fixture has no symbols").into(),
            );
        };
        let object = self.values.read(object)?;
        let value = self.values.read(value)?;
        let RustValue::Object(entries) = object.value() else {
            return Err(self.type_error());
        };
        let mut entries = entries.borrow_mut();
        if let Some((_, existing)) = entries.iter_mut().find(|(name, _)| name == key) {
            *existing = value;
        } else {
            entries.push((key.clone(), value));
        }
        Ok(())
    }
    fn call<'s>(
        &'s self,
        function: ScopedValue<'s>,
        receiver: Option<ScopedValue<'s>>,
        arguments: &[ScopedValue<'s>],
    ) -> JsResult<'s, ScopedValue<'s>> {
        let handler = match self.values.read(function)?.value() {
            RustValue::Function(handler) => handler.clone(),
            _ => return Err(self.type_error()),
        };
        let receiver = match receiver {
            Some(v) => self.values.read(v)?,
            None => self.import(&JsData::Undefined),
        };
        let arguments = arguments
            .iter()
            .map(|v| self.values.read(*v))
            .collect::<Result<Vec<_>, _>>()?;
        enum Outcome<R> {
            Value(Rc<R>),
            Exception(Rc<R>),
            Fault(FailureReport),
        }
        let outcome = self.context.values.with_scope(|values| {
            let receiver = values.insert_shared(receiver)?;
            let arguments = arguments
                .into_iter()
                .map(|v| values.insert_shared(v))
                .collect::<Result<Vec<_>, _>>()?;
            let frame = Invocation {
                inner: NativeScope {
                    context: self.context,
                    values,
                    host: self.host,
                },
                receiver,
                arguments,
            };
            Ok(match handler.invoke(&frame) {
                Ok(value) => Outcome::Value(values.read(value)?),
                Err(ScopedError::Exception(value)) => Outcome::Exception(values.read(value)?),
                Err(ScopedError::Fault(error)) => Outcome::Fault(error),
            })
        })?;
        match outcome {
            Outcome::Value(value) => self.values.insert_shared(value).map_err(Into::into),
            Outcome::Exception(value) => {
                Err(ScopedError::Exception(self.values.insert_shared(value)?))
            }
            Outcome::Fault(error) => Err(ScopedError::Fault(error)),
        }
    }
    fn construct<'s>(
        &'s self,
        constructor: ScopedValue<'s>,
        args: &[ScopedValue<'s>],
        new_target: Option<ScopedValue<'s>>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        if let Some(target) = new_target
            && self.kind(target)? != ValueKind::Function
        {
            return Err(self.type_error());
        }
        let receiver = self.put(RustValue::Object(RefCell::new(Vec::new())))?;
        let result = self.call(constructor, Some(receiver), args)?;
        Ok(
            if matches!(self.kind(result)?, ValueKind::Object | ValueKind::Function) {
                result
            } else {
                receiver
            },
        )
    }
    fn host_function<'s>(
        &'s self,
        _: &JsString,
        _: u16,
        function: Rc<dyn HostFunction>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.put(RustValue::Function(function))
    }
    fn deferred<'s>(&'s self) -> JsResult<'s, Deferred<'s>> {
        let promise = self.put(RustValue::Promise(RefCell::new(None)))?;
        let resolver = if self.context.invalid_resolver {
            self.put(RustValue::Data(JsData::Null))?
        } else {
            promise
        };
        Ok(Deferred {
            promise,
            resolver: self.values.retain_deferred(resolver)?,
        })
    }
    fn settle<'s>(
        &'s self,
        deferred: RootedDeferred,
        settlement: Settlement<'s>,
    ) -> JsResult<'s, ()> {
        let local = self.values.local_deferred(&deferred)?;
        let promise = self.values.read(local)?;
        let (rejected, value) = match settlement {
            Settlement::Resolve(v) => (false, v),
            Settlement::Reject(v) => (true, v),
        };
        let RustValue::Promise(state) = promise.value() else {
            return Err(
                FailureReport::fault(FailureCode::InvalidHandle, "not a fixture deferred").into(),
            );
        };
        if state.borrow().is_none() {
            *state.borrow_mut() = Some((rejected, self.values.read(value)?));
        }
        Ok(())
    }
    fn drain_microtasks(&self, hint: NonZeroUsize) -> Result<CheckpointReport, FailureReport> {
        self.context
            .checkpoint_calls
            .set(self.context.checkpoint_calls.get() + 1);
        let mut report = CheckpointReport::empty();
        for _ in 0..hint.get() {
            let task = self.context.checkpoint_work.borrow_mut().pop_front();
            let Some(task) = task else {
                break;
            };
            if let Err(error) = task(self.host) {
                report.push_script_failure(error)?;
            }
        }
        if !self.context.checkpoint_work.borrow().is_empty() {
            report.state = CheckpointState::Pending;
        }
        Ok(report)
    }
    fn detach(&self, error: ScopedError<'_>) -> FailureReport {
        match error {
            ScopedError::Fault(report) => report.bounded(),
            ScopedError::Exception(value) => {
                let mut report = FailureReport::new(
                    FailureClass::Script,
                    FailureCode::ScriptException,
                    "Rust fixture exception",
                );
                if let Ok(value) = self.values.read(value)
                    && let RustValue::Data(data) = value.value()
                {
                    report.thrown = Some(data.clone());
                }
                report.bounded()
            }
        }
    }
    fn source(&self) -> Option<&dyn SourceApi> {
        self.context
            .programs
            .as_ref()
            .map(|_| self as &dyn SourceApi)
    }
    fn compiler(&self) -> Option<&dyn CompilerApi> {
        self.context
            .programs
            .as_ref()
            .map(|_| self as &dyn CompilerApi)
    }
    fn artifacts(&self) -> Option<&dyn ArtifactApi> {
        self.context
            .programs
            .as_ref()
            .map(|_| self as &dyn ArtifactApi)
    }
    fn modules(&self) -> Option<&dyn ModuleApi> {
        self.context
            .module_specs
            .as_ref()
            .map(|_| self as &dyn ModuleApi)
    }
}

impl<R: Representation> NativeScope<'_, R> {
    fn program(&self, source: &ScriptSource) -> Result<Rc<dyn HostFunction>, FailureReport> {
        self.context
            .programs
            .as_ref()
            .and_then(|programs| {
                programs
                    .iter()
                    .find(|(expected, _)| {
                        expected.text == source.text
                            && expected.origin == source.origin
                            && source.source_map.is_none()
                    })
                    .map(|(_, program)| program.clone())
            })
            .ok_or_else(|| {
                FailureReport::fault(
                    FailureCode::InvalidArtifact,
                    "unknown exact named Rust program",
                )
            })
    }
    fn artifact_metadata(
        &self,
        source: &ScriptSource,
        options: CompileOptions,
    ) -> Result<ArtifactMetadata, FailureReport> {
        self.program(source)?;
        if options.mode != CodeMode::Script || options.fingerprint != "rust-program-v1" {
            return Err(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "unsupported named Rust compiler options",
            ));
        }
        let descriptor = &self.context.descriptor;
        Ok(ArtifactMetadata {
            adapter_id: descriptor.adapter_id.clone(),
            native_version: descriptor.native_version.clone(),
            format_version: 1,
            target_fingerprint: descriptor.target_fingerprint.clone(),
            build_fingerprint: descriptor.build_fingerprint.clone(),
            options,
            source_digest: SourceDigest::of_utf16(&source.text),
        })
    }
}
impl<R: Representation> SourceApi for NativeScope<'_, R> {
    fn evaluate<'s>(&'s self, source: &ScriptSource) -> JsResult<'s, ScopedValue<'s>> {
        let function = self.host_function(&source.text, 0, self.program(source)?)?;
        self.call(function, None, &[])
    }
}
impl<R: Representation> CompilerApi for NativeScope<'_, R> {
    fn compile(
        &self,
        source: &ScriptSource,
        options: CompileOptions,
    ) -> Result<CompiledArtifact, FailureReport> {
        let metadata = self.artifact_metadata(source, options)?;
        self.context
            .compile_calls
            .set(self.context.compile_calls.get() + 1);
        // SAFETY: Trusted empty output of this named Rust registry fixture. It is
        // validated as metadata only and never loaded by a native deserializer.
        unsafe {
            CompiledArtifact::from_trusted_compiler_output(metadata, Vec::new(), source.clone())
        }
    }
}
impl<R: Representation> ArtifactApi for NativeScope<'_, R> {
    fn execute<'s>(&'s self, artifact: &CompiledArtifact) -> JsResult<'s, ScopedValue<'s>> {
        let expected =
            self.artifact_metadata(artifact.source(), artifact.metadata().options.clone())?;
        artifact.validate(&expected, false)?;
        SourceApi::evaluate(self, artifact.source())
    }
}

// HostCall adds only invocation-local arguments/receiver. The delegate below is
// intentionally explicit so signature changes are checked by the compiler.
impl<R: Representation> JsScope for Invocation<'_, R> {
    fn data_limits(&self) -> ExportLimits {
        self.inner.data_limits()
    }
    fn context(&self) -> ContextId {
        self.inner.context()
    }
    fn global<'s>(&'s self) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.global()
    }
    fn kind(&self, v: ScopedValue<'_>) -> Result<ValueKind, FailureReport> {
        self.inner.kind(v)
    }
    fn local<'s>(&'s self, v: &RootedValue) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.local(v)
    }
    fn retain(&self, v: ScopedValue<'_>) -> Result<RootedValue, FailureReport> {
        self.inner.retain(v)
    }
    fn import_data<'s>(&'s self, v: JsData) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.import_data(v)
    }
    fn export_data<'s>(&'s self, v: ScopedValue<'s>, l: ExportLimits) -> JsResult<'s, JsData> {
        self.inner.export_data(v, l)
    }
    fn get<'s>(&'s self, v: ScopedValue<'s>, k: PropertyKey<'_>) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.get(v, k)
    }
    fn set<'s>(
        &'s self,
        o: ScopedValue<'s>,
        k: PropertyKey<'_>,
        v: ScopedValue<'s>,
    ) -> JsResult<'s, ()> {
        self.inner.set(o, k, v)
    }
    fn call<'s>(
        &'s self,
        f: ScopedValue<'s>,
        r: Option<ScopedValue<'s>>,
        a: &[ScopedValue<'s>],
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.call(f, r, a)
    }
    fn construct<'s>(
        &'s self,
        f: ScopedValue<'s>,
        a: &[ScopedValue<'s>],
        n: Option<ScopedValue<'s>>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.construct(f, a, n)
    }
    fn host_function<'s>(
        &'s self,
        n: &JsString,
        a: u16,
        f: Rc<dyn HostFunction>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        self.inner.host_function(n, a, f)
    }
    fn deferred<'s>(&'s self) -> JsResult<'s, Deferred<'s>> {
        self.inner.deferred()
    }
    fn settle<'s>(&'s self, d: RootedDeferred, s: Settlement<'s>) -> JsResult<'s, ()> {
        self.inner.settle(d, s)
    }
    fn drain_microtasks(&self, h: NonZeroUsize) -> Result<CheckpointReport, FailureReport> {
        self.inner.drain_microtasks(h)
    }
    fn detach(&self, e: ScopedError<'_>) -> FailureReport {
        self.inner.detach(e)
    }
    fn source(&self) -> Option<&dyn SourceApi> {
        self.inner.source()
    }
    fn compiler(&self) -> Option<&dyn CompilerApi> {
        self.inner.compiler()
    }
    fn artifacts(&self) -> Option<&dyn ArtifactApi> {
        self.inner.artifacts()
    }
    fn modules(&self) -> Option<&dyn ModuleApi> {
        self.inner.modules()
    }
}
impl<R: Representation> HostCall for Invocation<'_, R> {
    fn receiver<'s>(&'s self) -> ScopedValue<'s> {
        self.receiver
    }
    fn arguments<'s>(&'s self) -> &'s [ScopedValue<'s>] {
        &self.arguments
    }
    fn host(&self) -> &dyn HostServices {
        self.inner.host
    }
}

pub struct NoLoop;
impl HostServices for NoLoop {
    #[allow(
        clippy::result_large_err,
        reason = "The host contract preserves the rejected owned callback inline."
    )]
    fn post(&self, job: CallbackJob) -> Result<TaskId, Rejected<CallbackJob>> {
        Err(Rejected {
            value: job,
            reason: RejectionReason::Unsupported,
        })
    }
    #[allow(
        clippy::result_large_err,
        reason = "The host contract preserves the rejected owned callback inline."
    )]
    fn timer(
        &self,
        _: Duration,
        _: Option<Duration>,
        job: CallbackJob,
    ) -> Result<TaskId, Rejected<CallbackJob>> {
        self.post(job)
    }
    fn cancel(&self, _: TaskId) -> CancelOutcome {
        CancelOutcome::NotFound
    }
    fn request_stop(&self) -> Result<(), FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "no loop in ownership fixture",
        ))
    }
    fn reserve_operation(&self, _: usize) -> Result<OperationReservation, FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "no async bridge in ownership fixture",
        ))
    }
    fn bind_deferred(
        &self,
        _: OperationReservation,
        _: RootedDeferred,
        _: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "no async bridge in ownership fixture",
        ))
    }
    fn bind_callback(
        &self,
        _: OperationReservation,
        _: RootedValue,
        _: Option<Rc<dyn CancelNative>>,
    ) -> Result<CompletionTicket, FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "no async bridge in ownership fixture",
        ))
    }
    fn cancel_operation(&self, _: OperationId) -> CancelOutcome {
        CancelOutcome::NotFound
    }
}

pub fn engines() -> Vec<Box<dyn JsEngine>> {
    vec![
        Box::new(RustFixture::<Inline>::new("inline-fixture")),
        Box::new(RustFixture::<Boxed>::new("boxed-fixture")),
    ]
}

impl<R: Representation> NativeScope<'_, R> {
    fn prepare_declared_module(
        &self,
        source: &ScriptSource,
        identity: &JsString,
    ) -> Result<ModuleId, FailureReport> {
        let imports = self
            .context
            .module_specs
            .as_ref()
            .and_then(|specs| specs.iter().find(|(text, _)| text == &source.text))
            .map(|(_, imports)| imports.clone())
            .ok_or_else(|| {
                FailureReport::fault(
                    FailureCode::InvalidArtifact,
                    "unknown Rust module declaration",
                )
            })?;
        let id = self.context.values.issue_module_id()?;
        let namespace = self
            .import_data(JsData::Record(vec![(
                JsString::from_utf8("value"),
                JsData::Number(42.0),
            )]))
            .map_err(|e| self.detach(e))?;
        let namespace = self.values.read(namespace)?;
        self.context.modules.borrow_mut().insert(
            id.slot(),
            FixtureModule {
                source: source.clone(),
                identity: identity.clone(),
                imports,
                bindings: None,
                namespace,
                evaluated: false,
            },
        );
        Ok(id)
    }
}
impl<R: Representation> ModuleApi for NativeScope<'_, R> {
    fn features(&self) -> ModuleFeatures {
        ModuleFeatures {
            dynamic_import: self.context.dynamic_modules,
            ..Default::default()
        }
    }
    fn prepare(
        &self,
        source: &ScriptSource,
        identity: &JsString,
    ) -> Result<ModuleId, FailureReport> {
        if self.context.image_only_modules {
            return Err(FailureReport::fault(
                FailureCode::Unsupported,
                "fixture source parser disabled",
            ));
        }
        self.prepare_declared_module(source, identity)
    }
    fn imports(&self, id: &ModuleId) -> Result<Vec<JsString>, FailureReport> {
        if self.context.failing_module_imports {
            return Err(FailureReport::new(
                FailureClass::Engine,
                FailureCode::EngineFailure,
                "fixture imports failed",
            ));
        }
        Ok(self
            .context
            .modules
            .borrow()
            .get(&id.slot())
            .ok_or_else(|| {
                FailureReport::fault(FailureCode::InvalidHandle, "fixture module missing")
            })?
            .imports
            .clone())
    }
    fn bind(&self, id: &ModuleId, bindings: &[ModuleBinding]) -> Result<(), FailureReport> {
        self.context
            .bind_calls
            .set(self.context.bind_calls.get() + 1);
        let mut modules = self.context.modules.borrow_mut();
        if id.context() != self.context.values.context()
            || bindings.iter().any(|b| {
                b.module.context() != id.context() || !modules.contains_key(&b.module.slot())
            })
        {
            return Err(FailureReport::fault(
                FailureCode::WrongContext,
                "foreign Rust module binding",
            ));
        }
        let module = modules.get_mut(&id.slot()).unwrap();
        if module.imports
            != bindings
                .iter()
                .map(|b| b.specifier.clone())
                .collect::<Vec<_>>()
        {
            return Err(FailureReport::fault(
                FailureCode::InvalidHandle,
                "incomplete Rust module bindings",
            ));
        }
        module.bindings = Some(bindings.to_vec());
        Ok(())
    }
    fn link(&self, id: &ModuleId, bindings: &[ModuleBinding]) -> Result<(), FailureReport> {
        self.bind(id, bindings)
    }
    fn namespace<'s>(&'s self, id: &ModuleId) -> JsResult<'s, ScopedValue<'s>> {
        let namespace = self
            .context
            .modules
            .borrow()
            .get(&id.slot())
            .unwrap()
            .namespace
            .clone();
        Ok(self.values.insert_shared(namespace)?)
    }
    fn evaluate<'s>(&'s self, id: &ModuleId) -> JsResult<'s, ScopedValue<'s>> {
        let source = {
            let mut modules = self.context.modules.borrow_mut();
            let module = modules.get_mut(&id.slot()).unwrap();
            let source = (!module.evaluated).then(|| module.source.clone());
            module.evaluated = true;
            source
        };
        if let Some(source) = source
            && self.context.programs.is_some()
        {
            SourceApi::evaluate(self, &source)?;
        }
        self.namespace(id)
    }
    fn evaluation_state(&self, id: &ModuleId) -> Result<ModuleEvaluationState, FailureReport> {
        Ok(
            if self
                .context
                .modules
                .borrow()
                .get(&id.slot())
                .unwrap()
                .evaluated
            {
                ModuleEvaluationState::Fulfilled
            } else {
                ModuleEvaluationState::NotStarted
            },
        )
    }
    fn define_builtin(
        &self,
        identity: &JsString,
        _: &[BuiltinExport],
    ) -> Result<ModuleId, FailureReport> {
        let id = self.context.values.issue_module_id()?;
        let namespace = self
            .import_data(JsData::Record(vec![]))
            .map_err(|e| self.detach(e))?;
        let namespace = self.values.read(namespace)?;
        self.context.modules.borrow_mut().insert(
            id.slot(),
            FixtureModule {
                source: ScriptSource::new(identity.clone(), "rust:builtin".into()),
                identity: identity.clone(),
                imports: vec![],
                bindings: None,
                namespace,
                evaluated: false,
            },
        );
        Ok(id)
    }
    fn take_dynamic_imports(&self) -> Result<ModuleImportBatch, FailureReport> {
        let requests: Vec<_> = self
            .context
            .module_requests
            .borrow_mut()
            .drain(..)
            .collect();
        Ok(ModuleImportBatch {
            made_progress: !requests.is_empty(),
            requests,
        })
    }
    fn finish_dynamic_import(
        &self,
        id: ModuleImportId,
        result: Result<ModuleId, FailureReport>,
    ) -> Result<(), FailureReport> {
        if id.context != self.context.values.context() {
            return Err(FailureReport::fault(
                FailureCode::WrongContext,
                "foreign Rust import token",
            ));
        }
        let resolver = self
            .context
            .module_resolvers
            .borrow_mut()
            .remove(&id.sequence)
            .ok_or_else(|| {
                FailureReport::fault(
                    FailureCode::InvalidHandle,
                    "Rust import token already completed",
                )
            })?;
        let settlement = match result {
            Ok(module) => Settlement::Resolve(self.namespace(&module).map_err(|e| self.detach(e))?),
            Err(report) => Settlement::Reject(
                self.import_data(JsData::String(
                    report
                        .message
                        .unwrap_or_else(|| JsString::from_utf8("module rejected")),
                ))
                .map_err(|e| self.detach(e))?,
            ),
        };
        self.settle(resolver, settlement)
            .map_err(|e| self.detach(e))
    }
    fn images(&self) -> Option<&dyn ModuleImageApi> {
        self.context
            .module_images
            .then_some(self as &dyn ModuleImageApi)
    }
}

impl<R: Representation> ModuleImageApi for NativeScope<'_, R> {
    fn profile(&self) -> ImageProfile {
        let d = &self.context.descriptor;
        ImageProfile {
            adapter_id: d.adapter_id.clone(),
            native_revision: d.native_version.clone(),
            format_version: 1,
            target_fingerprint: d.target_fingerprint.clone(),
            build_fingerprint: d.build_fingerprint.clone(),
            debug_info: true,
        }
    }
    fn compile(&self, id: &ModuleId) -> Result<CompiledModuleImage, FailureReport> {
        if self.context.failing_image_compile {
            return Err(FailureReport::fault(
                FailureCode::Unsupported,
                "fixture image compile failure",
            ));
        }
        let modules = self.context.modules.borrow();
        let module = modules.get(&id.slot()).unwrap();
        let opcode = self
            .context
            .module_specs
            .as_ref()
            .and_then(|specs| {
                specs
                    .iter()
                    .position(|(text, _)| text == &module.source.text)
            })
            .ok_or_else(|| {
                FailureReport::fault(FailureCode::Unsupported, "builtin has no fixture opcode")
            })?;
        // SAFETY: An eight-byte instruction index is the complete trusted Rust fixture compiler format.
        Ok(unsafe {
            CompiledModuleImage::from_trusted_compiler_output(
                self.profile(),
                (opcode as u64).to_le_bytes().to_vec(),
                module.imports.clone(),
            )
        })
    }
    fn compile_source_cache(
        &self,
        id: &ModuleId,
    ) -> Result<Option<SourceModuleCache>, FailureReport> {
        let modules = self.context.modules.borrow();
        let module = &modules[&id.slot()];
        let payload =
            serde_json::to_vec(&(module.source.text.as_units(), &module.source.origin)).unwrap();
        // SAFETY: Detached fixture cache preserves this exact source declaration, module key and origin.
        Ok(Some(unsafe {
            SourceModuleCache::from_compiler_output(
                self.profile(),
                payload,
                module.imports.clone(),
                SourceDigest::of_utf16(&module.source.text),
                module.identity.clone(),
                module.source.origin.clone(),
            )
        }))
    }
    fn prepare_source_cache(
        &self,
        cache: &SourceModuleCache,
        key: &JsString,
    ) -> Result<ModuleId, FailureReport> {
        if cache.profile() != &self.profile() {
            return Err(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "fixture cache profile",
            ));
        }
        let (text, origin): (Vec<u16>, String) =
            serde_json::from_slice(cache.payload()).map_err(|_| {
                FailureReport::fault(FailureCode::InvalidArtifact, "fixture source cache")
            })?;
        let source = ScriptSource::new(JsString::from_units(text), origin);
        if !cache.matches_source(key, &source) {
            return Err(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "fixture cache source binding",
            ));
        }
        let id = self.prepare_declared_module(&source, key)?;
        if ModuleApi::imports(self, &id)? != cache.imports() {
            return Err(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "fixture cache import table",
            ));
        }
        Ok(id)
    }
    fn prepare(
        &self,
        image: &VerifiedModuleImage,
        key: &JsString,
    ) -> Result<ModuleId, FailureReport> {
        if image.profile() != &self.profile() {
            return Err(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "fixture image profile mismatch",
            ));
        }
        let bytes: [u8; 8] = image.payload().try_into().map_err(|_| {
            FailureReport::fault(FailureCode::InvalidArtifact, "fixture opcode width")
        })?;
        let (text, imports) = self
            .context
            .module_specs
            .as_ref()
            .and_then(|specs| specs.get(u64::from_le_bytes(bytes) as usize))
            .ok_or_else(|| {
                FailureReport::fault(FailureCode::InvalidArtifact, "fixture opcode missing")
            })?;
        if imports != image.imports() {
            return Err(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "fixture opcode imports mismatch",
            ));
        }
        self.prepare_declared_module(
            &ScriptSource::new(text.clone(), key.to_utf8().unwrap()),
            key,
        )
    }
}
