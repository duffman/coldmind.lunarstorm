//! Owner-local prepared native graphs. Native entry never calls a platform provider.
use super::*;
use std::ffi::CString;

pub(super) struct ModuleRecord {
    value: Rc<NativeValue>,
    imports: Vec<JsString>,
    bindings: RefCell<Option<Vec<ModuleBinding>>>,
    linked: Cell<bool>,
    failed: Cell<bool>,
    builtin: bool,
    source_binding: RefCell<Option<(SourceDigest, JsString, String)>>,
    evaluation: RefCell<Option<Rc<NativeValue>>>,
}
impl Scope<'_, '_> {
    fn write_prepared_module(
        &self,
        module: &ModuleId,
        preserve_source: bool,
    ) -> Result<(Rc<ModuleRecord>, Vec<u8>), FailureReport> {
        let m = self.module(module)?;
        if m.builtin || m.linked.get() || m.failed.get() {
            return Err(FailureReport::fault(
                FailureCode::InvalidHandle,
                "serialize only uninstantiated source modules",
            ));
        }
        let mut length = 0;
        // SAFETY: Owned uninstantiated native module remains retained through serialization.
        let bytes = unsafe {
            if preserve_source {
                ffi::jbq_module_cache_write(self.ptr(), m.value.value, &mut length)
            } else {
                ffi::jbq_module_write(self.ptr(), m.value.value, &mut length)
            }
        };
        if bytes.is_null() {
            return Err(self.detach(self.exception()));
        }
        let result = if length > self.state.limits.export.max_bytes {
            Err(FailureReport::fault(
                FailureCode::Capacity,
                "native image copy limit",
            ))
        } else {
            scope::copied_buffer(length, "native image allocation").map(|mut out| {
                // SAFETY: Native writer returned exactly length initialized bytes, protected until the free below.
                out.extend_from_slice(unsafe { std::slice::from_raw_parts(bytes, length) });
                out
            })
        };
        // SAFETY: Free writer output exactly once on Rust success and allocation/limit failure.
        unsafe { ffi::jbq_buffer_free(self.ptr(), bytes.cast()) };
        Ok((m, result?))
    }
    fn module(&self, id: &ModuleId) -> Result<Rc<ModuleRecord>, FailureReport> {
        if id.context() != self.state.values.context() {
            return Err(FailureReport::fault(
                FailureCode::WrongContext,
                "foreign module",
            ));
        }
        self.state.raw.ptr()?;
        self.state
            .modules
            .borrow()
            .get(&id.slot())
            .cloned()
            .ok_or_else(|| FailureReport::fault(FailureCode::InvalidHandle, "unknown module"))
    }
    fn module_admission(&self) -> Result<(), FailureReport> {
        let count = self.state.module_attempts.get();
        if count >= 4096 {
            return Err(FailureReport::fault(
                FailureCode::Capacity,
                "prepared module limit",
            ));
        }
        // Failed native preparation may retain records in QuickJS: attempts consume quota too.
        self.state.module_attempts.set(count + 1);
        Ok(())
    }
    fn register_module(
        &self,
        value: Rc<NativeValue>,
        builtin: bool,
    ) -> Result<ModuleId, FailureReport> {
        // SAFETY: owned compiler/reader result remains protected throughout enumeration.
        let count = unsafe { ffi::jbq_module_count(self.ptr(), value.value) };
        if !(0..=65536).contains(&count) {
            return Err(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "invalid module import count",
            ));
        }
        let mut imports = Vec::new();
        imports.try_reserve_exact(count as usize).map_err(|_| {
            FailureReport::fault(FailureCode::Capacity, "module imports allocation")
        })?;
        for i in 0..count {
            // SAFETY: Index is inside the native module's checked import range.
            let request = self
                .owned(unsafe { ffi::jbq_module_import(self.ptr(), value.value, i) })
                .map_err(|e| self.detach(e))?;
            let request = self
                .utf16(request.value, 4096, false)
                .map_err(|e| self.detach(e))?
                .0;
            if imports.contains(&request) {
                return Err(FailureReport::fault(
                    FailureCode::InvalidArtifact,
                    "duplicate native import",
                ));
            }
            imports.push(request);
        }
        let id = self.state.values.issue_module_id()?;
        self.state.modules.borrow_mut().insert(
            id.slot(),
            Rc::new(ModuleRecord {
                value,
                imports,
                bindings: RefCell::new(None),
                linked: Cell::new(false),
                failed: Cell::new(false),
                builtin,
                source_binding: RefCell::new(None),
                evaluation: RefCell::new(None),
            }),
        );
        Ok(id)
    }
    fn preflight_graph(&self, root: ModuleId) -> Result<Vec<Rc<ModuleRecord>>, FailureReport> {
        let mut seen = HashSet::new();
        let mut pending = vec![root];
        let mut records = Vec::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id.slot()) {
                continue;
            }
            let record = self.module(&id)?;
            if record.failed.get() {
                return Err(FailureReport::fault(
                    FailureCode::InvalidHandle,
                    "module graph instantiation previously failed",
                ));
            }
            let edges = record.bindings.borrow();
            if !record.imports.is_empty() && edges.is_none() {
                return Err(FailureReport::fault(
                    FailureCode::InvalidHandle,
                    "module graph has unbound edges",
                ));
            }
            if let Some(edges) = edges.as_ref() {
                pending.extend(edges.iter().map(|b| b.module));
            }
            drop(edges);
            records.push(record);
        }
        Ok(records)
    }
}
impl ModuleApi for Scope<'_, '_> {
    fn features(&self) -> ModuleFeatures {
        ModuleFeatures {
            top_level_await: true,
            dynamic_import: true,
        }
    }
    fn prepare(&self, source: &ScriptSource, key: &JsString) -> Result<ModuleId, FailureReport> {
        self.module_admission()?;
        if source.source_map.is_some() && !self.state.debug.is_development() {
            return Err(FailureReport::fault(
                FailureCode::Unsupported,
                "native source-map attachment unavailable",
            ));
        }
        if source.text.as_units().len() > self.state.limits.export.max_bytes / 2 {
            return Err(FailureReport::fault(
                FailureCode::Capacity,
                "module source exceeds copy budget",
            ));
        }
        let source_identity = key.clone();
        let key = key
            .to_utf8()
            .ok()
            .and_then(|s| CString::new(s).ok())
            .ok_or_else(|| {
                FailureReport::fault(
                    FailureCode::InvalidHandle,
                    "invalid module diagnostic origin",
                )
            })?;
        self.state
            .debug
            .stage_host_script(source, crate::debug::ScriptKind::Module)?;
        // SAFETY: Buffers live through compile-only native entry, which defers all resolution.
        let value = self
            .owned(unsafe {
                ffi::jbq_module_prepare(
                    self.ptr(),
                    source.text.as_units().as_ptr(),
                    source.text.as_units().len(),
                    key.as_ptr(),
                )
            })
            .map_err(|e| self.detach(e))?;
        let id = self.register_module(value, false)?;
        *self.module(&id)?.source_binding.borrow_mut() = Some((
            SourceDigest::of_utf16(&source.text),
            source_identity,
            source.origin.clone(),
        ));
        Ok(id)
    }
    fn imports(&self, module: &ModuleId) -> Result<Vec<JsString>, FailureReport> {
        Ok(self.module(module)?.imports.clone())
    }
    fn bind(&self, module: &ModuleId, bindings: &[ModuleBinding]) -> Result<(), FailureReport> {
        let record = self.module(module)?;
        if let Some(old) = record.bindings.borrow().as_ref() {
            return if old == bindings {
                Ok(())
            } else {
                Err(FailureReport::fault(
                    FailureCode::InvalidHandle,
                    "conflicting module bindings",
                ))
            };
        }
        if record.imports.len() != bindings.len()
            || record
                .imports
                .iter()
                .zip(bindings)
                .any(|(a, b)| a != &b.specifier)
        {
            return Err(FailureReport::fault(
                FailureCode::InvalidHandle,
                "incomplete or unordered module bindings",
            ));
        }
        let targets = bindings
            .iter()
            .map(|b| self.module(&b.module))
            .collect::<Result<Vec<_>, _>>()?;
        for (index, target) in targets.iter().enumerate() {
            // SAFETY: All module identities and the complete index range were checked first.
            self.detached_status(unsafe {
                ffi::jbq_module_bind(
                    self.ptr(),
                    record.value.value,
                    index as i32,
                    target.value.value,
                )
            })?;
        }
        *record.bindings.borrow_mut() = Some(bindings.to_vec());
        Ok(())
    }
    fn link(&self, module: &ModuleId, bindings: &[ModuleBinding]) -> Result<(), FailureReport> {
        self.bind(module, bindings)?;
        let graph = self.preflight_graph(*module)?;
        let root = self.module(module)?;
        if root.linked.get() {
            return Ok(());
        }
        // SAFETY: Every reachable native edge has been validated and prebound, including cycles.
        let result =
            self.detached_status(unsafe { ffi::jbq_module_link(self.ptr(), root.value.value) });
        for record in graph {
            if result.is_ok() {
                record.linked.set(true);
            } else if !record.linked.get() {
                // A failed new graph must not invalidate a dependency already instantiated
                // successfully by another graph in this context.
                record.failed.set(true);
            }
        }
        result
    }
    fn namespace<'s>(&'s self, module: &ModuleId) -> JsResult<'s, ScopedValue<'s>> {
        let m = self.module(module)?;
        if !m.linked.get() {
            return Err(FailureReport::fault(
                FailureCode::InvalidHandle,
                "module not instantiated",
            )
            .into());
        }
        // SAFETY: Checked instantiated owner-local module remains retained by record.
        self.put(unsafe { ffi::jbq_module_namespace(self.ptr(), m.value.value) })
    }
    fn evaluate<'s>(&'s self, module: &ModuleId) -> JsResult<'s, ScopedValue<'s>> {
        let m = self.module(module)?;
        if !m.linked.get() {
            return Err(FailureReport::fault(
                FailureCode::InvalidHandle,
                "module not instantiated",
            )
            .into());
        }
        if let Some(value) = m.evaluation.borrow().clone() {
            return Ok(self.values.insert_shared(value)?);
        }
        // SAFETY: Native evaluation receives a duplicated reference and preserves module single evaluation.
        let value = self.owned(unsafe { ffi::jbq_eval_compiled(self.ptr(), m.value.value) })?;
        *m.evaluation.borrow_mut() = Some(value.clone());
        Ok(self.values.insert_shared(value)?)
    }
    fn evaluation_state(&self, module: &ModuleId) -> Result<ModuleEvaluationState, FailureReport> {
        let m = self.module(module)?;
        let Some(value) = m.evaluation.borrow().clone() else {
            return Ok(ModuleEvaluationState::NotStarted);
        };
        // SAFETY: Value is the retained native evaluation result for this live context.
        match unsafe { ffi::jbq_promise_state(self.ptr(), value.value) } {
            0 => Ok(ModuleEvaluationState::Pending),
            2 => {
                // SAFETY: Promise was checked above; native result transfers one owned reference.
                let thrown = self
                    .put(unsafe { ffi::jbq_promise_result(self.ptr(), value.value) })
                    .map_err(|e| self.detach(e))?;
                Ok(ModuleEvaluationState::Rejected(
                    self.detach(ScopedError::Exception(thrown)),
                ))
            }
            _ => Ok(ModuleEvaluationState::Fulfilled),
        }
    }
    fn define_builtin(
        &self,
        identity: &JsString,
        exports: &[BuiltinExport],
    ) -> Result<ModuleId, FailureReport> {
        self.module_admission()?;
        if exports.len() > 1024 {
            return Err(FailureReport::fault(
                FailureCode::Capacity,
                "builtin exports limit",
            ));
        }
        let name = identity
            .to_utf8()
            .ok()
            .and_then(|s| CString::new(s).ok())
            .ok_or_else(|| {
                FailureReport::fault(FailureCode::InvalidHandle, "invalid builtin identity")
            })?;
        let object = self
            .import_data(JsData::Record(Vec::new()))
            .map_err(|e| self.detach(e))?;
        let mut names = HashSet::new();
        for export in exports {
            if export.name.as_units().contains(&0) || !names.insert(export.name.clone()) {
                return Err(FailureReport::fault(
                    FailureCode::InvalidHandle,
                    "duplicate or NUL builtin export",
                ));
            }
            let value = self
                .materialize(&export.value)
                .map_err(|e| self.detach(e))?;
            self.set(object, PropertyKey::String(&export.name), value)
                .map_err(|e| self.detach(e))?;
        }
        let object = self.read(object)?;
        // SAFETY: Export object contains validated context-local materialized values.
        let value = self
            .owned(unsafe { ffi::jbq_module_builtin(self.ptr(), name.as_ptr(), object.value) })
            .map_err(|e| self.detach(e))?;
        self.register_module(value, true)
    }

    fn take_dynamic_imports(&self) -> Result<ModuleImportBatch, FailureReport> {
        let mut made_progress = false;
        let mut requests = Vec::new();
        requests.try_reserve_exact(128).map_err(|_| {
            FailureReport::fault(FailureCode::Capacity, "dynamic import request allocation")
        })?;
        for _ in 0..128 {
            let mut base = ffi::Value { words: [0, 0] };
            let mut specifier = base;
            // SAFETY: Writable outputs receive owned native strings; queue retains each token until finish.
            let sequence = unsafe { ffi::jbq_import_take(self.ptr(), &mut base, &mut specifier) };
            if sequence == 0 {
                break;
            }
            made_progress = true;
            let id = ModuleImportId {
                context: self.state.values.context(),
                sequence,
            };
            let request: Result<ModuleImportRequest, FailureReport> = (|| {
                // Own both returned strings before any conversion can fail.
                let base = Rc::new(NativeValue::new(base, self.state.raw.clone()));
                let specifier = Rc::new(NativeValue::new(specifier, self.state.raw.clone()));
                Ok(ModuleImportRequest {
                    id,
                    referrer: self
                        .utf16(base.value, 4096, false)
                        .map_err(|e| self.detach(e))?
                        .0,
                    specifier: self
                        .utf16(specifier.value, 4096, false)
                        .map_err(|e| self.detach(e))?
                        .0,
                })
            })();
            match request {
                Ok(request) => requests.push(request),
                Err(report)
                    if matches!(report.class, FailureClass::Engine | FailureClass::Host) =>
                {
                    return Err(report);
                }
                Err(report) => self.finish_dynamic_import(id, Err(report))?,
            }
        }
        Ok(ModuleImportBatch {
            requests,
            made_progress,
        })
    }
    fn finish_dynamic_import(
        &self,
        id: ModuleImportId,
        result: Result<ModuleId, FailureReport>,
    ) -> Result<(), FailureReport> {
        if id.context != self.state.values.context() {
            return Err(FailureReport::fault(
                FailureCode::WrongContext,
                "foreign dynamic import token",
            ));
        }
        let (module, evaluation, failure) = match result {
            Ok(id) => {
                let module = self.module(&id)?;
                let value = module.evaluation.borrow().clone().ok_or_else(|| {
                    FailureReport::fault(FailureCode::InvalidHandle, "dynamic module not evaluated")
                })?;
                (Some(module), Some(value), None)
            }
            Err(report) => {
                // The importer sees the real cause, not a fixed placeholder.
                let cause = report
                    .message
                    .as_ref()
                    .map(|m| String::from_utf16_lossy(m.as_units()))
                    .unwrap_or_else(|| format!("{:?}", report.code));
                let text = format!("module load failed: {cause}").replace('\0', "");
                (None, None, Some(CString::new(text).expect("NUL removed")))
            }
        };
        // SAFETY: Checked owner-local tokens are consumed once; module/evaluation wrappers remain retained.
        let status = unsafe {
            ffi::jbq_import_finish(
                self.ptr(),
                id.sequence,
                module
                    .as_ref()
                    .map_or(ffi::Value { words: [0, 0] }, |m| m.value.value),
                evaluation
                    .as_ref()
                    .map_or(ffi::Value { words: [0, 0] }, |v| v.value),
                failure.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            )
        };
        self.detached_status(status)
    }
    fn images(&self) -> Option<&dyn ModuleImageApi> {
        Some(self)
    }
}
impl ModuleImageApi for Scope<'_, '_> {
    fn profile(&self) -> ImageProfile {
        ImageProfile {
            adapter_id: "quickjs-ng".into(),
            native_revision: NATIVE_REVISION.into(),
            format_version: 1,
            target_fingerprint: env!("JETBUNNY_QJS_TARGET").into(),
            build_fingerprint: env!("JETBUNNY_QJS_BUILD_ID").into(),
            debug_info: true,
        }
    }
    fn compile(&self, module: &ModuleId) -> Result<CompiledModuleImage, FailureReport> {
        let (m, bytes) = self.write_prepared_module(module, false)?;
        // SAFETY: Native distribution writer strips source and supplies this exact profile/import table.
        Ok(unsafe {
            CompiledModuleImage::from_trusted_compiler_output(
                self.profile(),
                bytes,
                m.imports.clone(),
            )
        })
    }
    fn compile_source_cache(
        &self,
        module: &ModuleId,
    ) -> Result<Option<SourceModuleCache>, FailureReport> {
        let Some((digest, identity, origin)) = self.module(module)?.source_binding.borrow().clone()
        else {
            return Ok(None);
        };
        let (m, bytes) = self.write_prepared_module(module, true)?;
        // SAFETY: Native cache writer preserves source; digest belongs to this immutable prepared module.
        Ok(Some(unsafe {
            SourceModuleCache::from_compiler_output(
                self.profile(),
                bytes,
                m.imports.clone(),
                digest,
                identity,
                origin,
            )
        }))
    }
    fn prepare(
        &self,
        image: &VerifiedModuleImage,
        identity: &JsString,
    ) -> Result<ModuleId, FailureReport> {
        // Builtin platform images load without source; their frames have no debug location.
        if self.state.debug.is_development() && !image.is_platform() {
            return Err(FailureReport::fault(
                FailureCode::Unsupported,
                "Development requires module source registration; source-free images are unsupported",
            ));
        }
        if image.profile() != &self.profile() {
            return Err(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "native image compatibility mismatch",
            ));
        }
        if image.payload().len() > self.state.limits.export.max_bytes {
            return Err(FailureReport::fault(
                FailureCode::Capacity,
                "native image input limit",
            ));
        }
        self.module_admission()?;
        let key = self
            .import_data(JsData::String(identity.clone()))
            .map_err(|e| self.detach(e))?;
        let key = self.read(key)?;
        // SAFETY: Opaque verified image carries trusted compiler provenance and exact compatibility.
        let value = self
            .owned(unsafe {
                ffi::jbq_module_read(
                    self.ptr(),
                    image.payload().as_ptr(),
                    image.payload().len(),
                    key.value,
                )
            })
            .map_err(|e| self.detach(e))?;
        let id = self.register_module(value, false)?;
        if self.module(&id)?.imports != image.imports() {
            return Err(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "native imports disagree with authenticated manifest",
            ));
        }
        Ok(id)
    }
}
