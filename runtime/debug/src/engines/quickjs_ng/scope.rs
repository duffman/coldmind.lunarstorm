//! Native scope operations and safe detached diagnostics.
use super::*;
use std::{ffi::CString, num::NonZeroUsize};

pub(super) struct Scope<'a, 'store> {
    pub(super) state: &'a Rc<ContextState>,
    pub(super) values: &'a ValueScope<'store, NativeValue>,
    pub(super) host: &'a dyn HostServices,
}
// Remove an incomplete entry even when factory code unwinds. Never hold a
// registry borrow while dropping user-owned definitions or entering host code.
struct Materializing<'a> {
    state: &'a ContextState,
    key: usize,
    ready: bool,
}
impl Drop for Materializing<'_> {
    fn drop(&mut self) {
        if !self.ready {
            let entry = self.state.host_values.borrow_mut().remove(&self.key);
            drop(entry);
        }
    }
}
impl Scope<'_, '_> {
    pub(super) fn compiled_payload(
        &self,
        compiled: &NativeValue,
        debug_info: bool,
    ) -> Result<Vec<u8>, FailureReport> {
        let mut length = 0;
        // SAFETY: The compiled object is owned/protected; write returns native allocated bytes.
        let bytes = unsafe {
            if debug_info {
                ffi::jbq_write_debug(self.ptr(), compiled.value, &mut length)
            } else {
                ffi::jbq_write(self.ptr(), compiled.value, &mut length)
            }
        };
        if bytes.is_null() {
            return Err(self.detach(self.exception()));
        }
        let mut payload = Vec::new();
        let copied = if length > self.state.limits.export.max_bytes {
            Err(FailureReport::fault(
                FailureCode::Capacity,
                "compiled artifact exceeds copied payload limit",
            ))
        } else {
            copied_buffer(length, "artifact copy allocation failed").map(|mut copied| {
                // SAFETY: The pointer was checked nonnull above, is aligned for
                // u8, and contains length initialized bytes. A zero length is
                // valid too; copied has reserved the full slice's storage.
                copied.extend_from_slice(unsafe { std::slice::from_raw_parts(bytes, length) });
                payload = copied;
            })
        };
        // SAFETY: Matching native allocation and live context; no use follows this free.
        unsafe { ffi::jbq_buffer_free(self.ptr(), bytes.cast()) };
        copied?;
        Ok(payload)
    }
    fn pending_fault(&self) -> Option<FailureReport> {
        // SAFETY: The current scope keeps this owner-local runtime alive. The
        // sticky flag is consumed even when an earlier host fault takes priority.
        let out_of_memory =
            self.state.raw.oom_latched() && unsafe { ffi::jbq_take_oom(self.ptr()) } != 0;
        self.state.raw.take_fault().or_else(|| {
            out_of_memory.then(|| {
                FailureReport::new(
                    FailureClass::Engine,
                    FailureCode::EngineFailure,
                    "QuickJS native allocation failed",
                )
            })
        })
    }
    pub(super) fn ptr(&self) -> *mut ffi::Context {
        self.state
            .raw
            .pointer
            .get()
            .expect("live scoped native context")
            .as_ptr()
    }
    pub(super) fn read(&self, value: ScopedValue<'_>) -> Result<Rc<NativeValue>, FailureReport> {
        let value = self.values.read(value)?;
        if value.resolver {
            return Err(FailureReport::fault(
                FailureCode::InvalidHandle,
                "resolver is not an application value",
            ));
        }
        Ok(value)
    }
    pub(super) fn native_kind(&self, value: ffi::Value) -> ValueKind {
        // SAFETY: Caller supplies a live borrowed value protected by this scope or a native wrapper.
        match unsafe { ffi::jbq_kind(self.ptr(), value) } {
            0 => ValueKind::Undefined,
            1 => ValueKind::Null,
            2 => ValueKind::Boolean,
            3 => ValueKind::Number,
            4 => ValueKind::String,
            5 => ValueKind::Symbol,
            6 => ValueKind::BigInt,
            8 => ValueKind::Function,
            _ => ValueKind::Object,
        }
    }
    /// lunarstorm patch: compiles a host script with the C parser and does what
    /// jetbunny's Rust compiler callback (`compiler.rs`, not taken) did around each
    /// compile: it takes the staged debugger registration and hands the compiled
    /// root's statement markers to the debugger. Without this, breakpoints set on a
    /// host script never bound on the C path (a conditional breakpoint never ran).
    ///
    /// # Arguments
    /// * `source` - the script; [`QuickJsDebug::stage_host_script`] has staged it.
    /// * `origin` - its origin as the engine records it.
    /// * `debug_info` - keep debug info in the compiled image.
    ///
    /// # Returns
    /// The compiled root, owned by this scope; nothing has run.
    ///
    /// # Errors
    /// The source does not compile, or the debugger refused it.
    fn compile_registered<'s>(
        &'s self,
        source: &ScriptSource,
        origin: &CString,
        debug_info: bool,
    ) -> JsResult<'s, Rc<NativeValue>> {
        let units = source.text.as_units();
        let registration = self
            .state
            .debug
            .register_source(units, &source.origin, crate::debug::ScriptKind::Script)
            .map_err(|e| ScopedError::Fault(super::debug::failure(e)))?;
        // jetbunny counted compiles for tests in its compiler callback; the count is
        // taken here, where source is handed to the C parser.
        #[cfg(test)]
        self.state.debug.lookup_observation.compilations.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let compile = if debug_info { ffi::jbq_compile_debug } else { ffi::jbq_compile };
        // SAFETY: compilation borrows valid source buffers and runs no application code.
        let compiled = self.owned(unsafe { compile(self.ptr(), units.as_ptr(), units.len(), origin.as_ptr()) })?;
        self.state
            .debug
            .loaded(compiled.value, registration)
            .map_err(|e| ScopedError::Fault(super::debug::failure(e)))?;
        Ok(compiled)
    }
    pub(super) fn owned<'s>(&'s self, value: ffi::Value) -> JsResult<'s, Rc<NativeValue>> {
        if value.is_exception() {
            return Err(self.exception());
        }
        let wrapper = Rc::new(NativeValue::new(value, self.state.raw.clone()));
        if let Some(error) = self.pending_fault() {
            return Err(ScopedError::Fault(error));
        }
        Ok(wrapper)
    }
    pub(super) fn put<'s>(&'s self, value: ffi::Value) -> JsResult<'s, ScopedValue<'s>> {
        let value = self.owned(value)?;
        self.values.insert_shared(value).map_err(Into::into)
    }
    /// The native values of a call's arguments, in order.
    ///
    /// Fix: each call built a `Vec` of the argument wrappers and a second `Vec` of their
    /// native values. Decision: up to [`INLINE_ARGUMENTS`] native values go into the caller's
    /// stack array and only longer lists use `spilled`. The wrappers need not be held: a
    /// frame keeps every value it was given until the frame ends, which is after the call.
    ///
    /// # Arguments
    /// * `arguments` - The scoped arguments.
    /// * `inline` - The caller's stack storage.
    /// * `spilled` - The caller's heap storage, used past the inline capacity.
    ///
    /// # Returns
    /// The native values, borrowed from `inline` or `spilled`.
    ///
    /// # Errors
    /// An argument is not a live value of this scope.
    fn native_arguments<'a>(
        &self,
        arguments: &[ScopedValue<'_>],
        inline: &'a mut [ffi::Value; INLINE_ARGUMENTS],
        spilled: &'a mut Vec<ffi::Value>,
    ) -> Result<&'a [ffi::Value], FailureReport> {
        if arguments.len() <= INLINE_ARGUMENTS {
            for (slot, argument) in inline.iter_mut().zip(arguments) {
                *slot = self.read(*argument)?.value;
            }
            return Ok(&inline[..arguments.len()]);
        }
        spilled.reserve_exact(arguments.len());
        for argument in arguments {
            spilled.push(self.read(*argument)?.value);
        }
        Ok(spilled)
    }
    pub(super) fn exception<'s>(&'s self) -> ScopedError<'s> {
        // SAFETY: owner-local latch survives consumed exception carriers. An
        // evaluator unwind must never become a persistent application fault.
        if unsafe { ffi::jbq_debug_unwinding(self.ptr()) } != 0 {
            // SAFETY: taking the inert carrier and replacing it with undefined
            // cannot run JavaScript. Native reentry/host return polls the latch.
            unsafe { ffi::jbq_free(self.ptr(), ffi::jbq_take_exception(self.ptr())) };
            // SAFETY: undefined is an immediate value with no native resources.
            let undefined = unsafe { ffi::jbq_undefined() };
            return scoped_exception(
                self.values
                    .insert(NativeValue::new(undefined, self.state.raw.clone())),
            );
        }
        // SAFETY: Taking the pending native exception transfers its owned reference.
        let error = NativeValue::new(
            unsafe { ffi::jbq_take_exception(self.ptr()) },
            self.state.raw.clone(),
        );
        if let Some(fault) = self.pending_fault() {
            return ScopedError::Fault(fault);
        }
        // SAFETY: this is owner-local sticky metadata, set only by debugger stop;
        // typed host/OOM faults above retain precedence over cooperative unwind.
        if unsafe { ffi::jbq_take_debug_stop(self.ptr()) } != 0 {
            return ScopedError::Fault(FailureReport::new(
                FailureClass::Engine,
                FailureCode::Interrupted,
                "debugger requested stop",
            ));
        }
        // SAFETY: This is the owned native exception just taken above. Inspecting
        // its internal flag executes no JavaScript or user-defined property access.
        if unsafe { ffi::jbq_is_uncatchable(error.value) } != 0 {
            return ScopedError::Fault(FailureReport::new(
                FailureClass::Engine,
                FailureCode::EngineFailure,
                "uncatchable QuickJS failure or interruption",
            ));
        }
        scoped_exception(self.values.insert(error))
    }
    pub(super) fn check_status(&self, status: i32) -> JsResult<'_, ()> {
        if status < 0 {
            Err(self.exception())
        } else if let Some(error) = self.pending_fault() {
            Err(ScopedError::Fault(error))
        } else {
            Ok(())
        }
    }
    pub(super) fn utf16<'s>(
        &'s self,
        value: ffi::Value,
        maximum: usize,
        truncate: bool,
    ) -> JsResult<'s, (JsString, bool)> {
        let mut length = 0;
        // SAFETY: Only a primitive native string is passed; the returned allocation
        // is copied before any further native entry and always explicitly released.
        let native = unsafe { ffi::jbq_string_data(self.ptr(), value, &mut length) };
        if native.is_null() {
            return Err(self.exception());
        }
        if length > maximum && !truncate {
            // SAFETY: This pointer came from string_data for this same context.
            unsafe { ffi::jbq_string_free(self.ptr(), native) };
            return Err(FailureReport::fault(
                FailureCode::Capacity,
                "native string exceeds copied data budget",
            )
            .into());
        }
        let count = length.min(maximum);
        let mut allocation = copied_buffer(count, "string copy allocation failed");
        if let Ok(units) = allocation.as_mut()
            && count != 0
        {
            // SAFETY: Native allocation contains length initialized u16 values;
            // count <= length, and the native reference remains held by the caller.
            units.extend_from_slice(unsafe { std::slice::from_raw_parts(native, count) });
        }
        // SAFETY: Matching context and the string allocation have remained live.
        unsafe { ffi::jbq_string_free(self.ptr(), native) };
        Ok((JsString::from_units(allocation?), length > maximum))
    }
    pub(super) fn detached_status(&self, status: i32) -> Result<(), FailureReport> {
        self.check_status(status)
            .map_err(|error| self.detach(error))
    }
    fn diagnostic_field(
        &self,
        value: ffi::Value,
        field: i32,
        maximum: usize,
    ) -> Result<Option<(JsString, bool)>, FailureReport> {
        // SAFETY: The caller protects value, and the shim reads only native Error
        // metadata or own data properties of null-prototype ordinary objects (host
        // failure records), without invoking mutable application accessors or proxies.
        let field = unsafe { ffi::jbq_error_field(self.ptr(), value, field) };
        let field = self.owned(field).map_err(diagnostic_failure)?;
        if self.native_kind(field.value) == ValueKind::String {
            self.utf16(field.value, maximum, true)
                .map(Some)
                .map_err(diagnostic_failure)
        } else {
            Ok(None)
        }
    }
    pub(super) fn string<'s>(
        &'s self,
        value: ffi::Value,
        maximum: usize,
    ) -> JsResult<'s, JsString> {
        self.utf16(value, maximum, false).map(|(s, _)| s)
    }
    pub(super) fn number(&self, value: ffi::Value) -> JsResult<'_, f64> {
        let mut number = 0.;
        // SAFETY: Callers classify this protected native value as a primitive
        // Number. No user coercion runs; check_status also preserves host faults.
        self.check_status(unsafe { ffi::jbq_to_number(self.ptr(), value, &mut number) })?;
        Ok(number)
    }
    /// The atom of a string property name: the context's cached one, or a temporary one when
    /// the cache does not take the name (see `RawContext::atoms`).
    ///
    /// # Arguments
    /// * `name` - The property name.
    ///
    /// # Returns
    /// The atom and whether it is temporary; the caller releases a temporary atom with
    /// [`Self::release_atom`] after its one native access.
    ///
    /// # Errors
    /// The engine failed to create the atom (out of memory).
    fn atom<'s>(&'s self, name: &JsString) -> JsResult<'s, (u32, bool)> {
        let units = name.as_units();
        if let Some(&atom) = self.state.raw.atoms.borrow().get(units) {
            return Ok((atom, false));
        }
        // SAFETY: the UTF-16 input is borrowed for this call and native code copies it.
        let atom = unsafe { ffi::jbq_atom(self.ptr(), units.as_ptr(), units.len()) };
        if atom == 0 {
            return Err(self.exception());
        }
        let mut atoms = self.state.raw.atoms.borrow_mut();
        if units.len() <= MAX_CACHED_ATOM_UNITS
            && atoms.len() < MAX_CACHED_ATOMS
            && atoms.try_reserve(1).is_ok()
        {
            atoms.insert(units.to_vec(), atom);
            return Ok((atom, false));
        }
        Ok((atom, true))
    }
    /// Releases a temporary atom of [`Self::atom`]; a cached one is left alone.
    ///
    /// # Arguments
    /// * `atom` - The atom and whether it is temporary.
    fn release_atom(&self, (atom, temporary): (u32, bool)) {
        if temporary {
            // SAFETY: the temporary atom owns one reference, released once here.
            unsafe { ffi::jbq_atom_free(self.ptr(), atom) };
        }
    }
    fn key<'s>(&'s self, key: PropertyKey<'_>) -> JsResult<'s, Rc<NativeValue>> {
        match key {
            PropertyKey::String(text) => {
                // SAFETY: The UTF-16 input is borrowed for this call and native code copies it.
                self.owned(unsafe {
                    ffi::jbq_string(self.ptr(), text.as_units().as_ptr(), text.as_units().len())
                })
            }
            PropertyKey::Symbol(value) => {
                let value = self.read(value)?;
                if self.native_kind(value.value) != ValueKind::Symbol {
                    return Err(FailureReport::fault(
                        FailureCode::InvalidHandle,
                        "property symbol required",
                    )
                    .into());
                }
                Ok(value)
            }
        }
    }
    fn compile_policy(&self, options: &CompileOptions) -> Result<(), FailureReport> {
        if options != &QuickJsNgEngine::script_compile_options()
            && options != &QuickJsNgEngine::debug_compile_options()
        {
            Err(FailureReport::fault(
                FailureCode::Unsupported,
                "QuickJS supports Script/Strip/default compiler policy",
            ))
        } else if self.state.debug.is_development() && options.debug_info != DebugInfo::Required {
            Err(FailureReport::fault(
                FailureCode::Unsupported,
                "Development requires DebugInfo::Required",
            ))
        } else {
            Ok(())
        }
    }
    fn source_origin(&self, source: &ScriptSource) -> Result<CString, FailureReport> {
        crate::debug::validate_source(source, &crate::debug::DebugLimits::default()).map_err(
            |_| FailureReport::fault(FailureCode::InvalidArtifact, "invalid source metadata"),
        )?;
        CString::new(source.origin.as_bytes()).map_err(|_| {
            FailureReport::fault(FailureCode::InvalidArtifact, "source origin contains NUL")
        })
    }
    fn artifact_metadata(
        &self,
        source: &ScriptSource,
        options: CompileOptions,
    ) -> ArtifactMetadata {
        ArtifactMetadata {
            adapter_id: "quickjs-ng".into(),
            native_version: NATIVE_REVISION.into(),
            format_version: 1,
            target_fingerprint: env!("JETBUNNY_QJS_TARGET").into(),
            build_fingerprint: env!("JETBUNNY_QJS_BUILD_ID").into(),
            options,
            source_digest: SourceDigest::of_utf16(&source.text),
        }
    }
}
/// The arguments of a call kept on the stack; see [`Scope::native_arguments`].
const INLINE_ARGUMENTS: usize = 8;
impl JsScope for Scope<'_, '_> {
    /// A primitive Number read with one tag test and one conversion; see
    /// [`JsScope::number`].
    ///
    /// # Arguments
    /// * `value` - The value.
    ///
    /// # Returns
    /// The number, or `None` for any other kind.
    ///
    /// # Errors
    /// Identity, liveness or engine faults.
    fn number<'s>(&'s self, value: ScopedValue<'s>) -> JsResult<'s, Option<f64>> {
        let native = self.read(value)?;
        if self.native_kind(native.value) != ValueKind::Number {
            return Ok(None);
        }
        Scope::number(self, native.value).map(Some)
    }
    fn data_limits(&self) -> ExportLimits {
        self.state.limits.export
    }
    fn context(&self) -> ContextId {
        self.state.values.context()
    }
    fn global<'s>(&'s self) -> JsResult<'s, ScopedValue<'s>> {
        // SAFETY: This active frame owns a live native context; the returned global is owned.
        self.put(unsafe { ffi::jbq_global(self.ptr()) })
    }
    fn kind(&self, value: ScopedValue<'_>) -> Result<ValueKind, FailureReport> {
        Ok(self.native_kind(self.read(value)?.value))
    }
    fn local<'s>(&'s self, value: &RootedValue) -> JsResult<'s, ScopedValue<'s>> {
        self.values.local(value).map_err(Into::into)
    }
    fn retain(&self, value: ScopedValue<'_>) -> Result<RootedValue, FailureReport> {
        self.read(value)?;
        self.values.retain(value)
    }
    fn import_data<'s>(&'s self, data: JsData) -> JsResult<'s, ScopedValue<'s>> {
        self.import(data)
    }
    fn materialize<'s>(
        &'s self,
        definition: &Rc<HostValueDefinition>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        let key = Rc::as_ptr(definition) as usize;
        let existing = {
            let cache = self.state.host_values.borrow();
            match cache.get(&key) {
                Some(HostValueEntry {
                    state: HostValueState::Ready(value),
                    ..
                }) => Some(value.clone()),
                Some(_) => {
                    return Err(FailureReport::fault(
                        FailureCode::InvalidHandle,
                        "recursive host value materialization",
                    )
                    .into());
                }
                None => None,
            }
        };
        if let Some(value) = existing {
            return self.values.insert_shared(value).map_err(Into::into);
        }
        self.state.host_values.borrow_mut().insert(
            key,
            HostValueEntry {
                _definition: definition.clone(),
                state: HostValueState::Building,
            },
        );
        let mut guard = Materializing {
            state: self.state,
            key,
            ready: false,
        };
        let value = match definition.as_ref() {
            HostValueDefinition::Function {
                name,
                arity,
                function,
            } => self.host_function(name, *arity, function.clone())?,
            HostValueDefinition::Data(data) => self.import_data(data.clone())?,
            HostValueDefinition::Factory(factory) => factory.create(self)?,
        };
        let native = self.read(value)?;
        self.state
            .host_values
            .borrow_mut()
            .get_mut(&key)
            .expect("materialization entry remains present")
            .state = HostValueState::Ready(native);
        guard.ready = true;
        Ok(value)
    }
    fn export_data<'s>(
        &'s self,
        value: ScopedValue<'s>,
        limits: ExportLimits,
    ) -> JsResult<'s, JsData> {
        self.export(value, limits)
    }
    /// One native copy to UTF-8 (`JS_ToCStringLen2`); see [`JsScope::export_utf8`].
    ///
    /// Decision: the string is taken only when `export_data` would surely take it. Its
    /// budget is `size_of::<JsData>()` plus two bytes per UTF-16 unit, and a string never has
    /// more UTF-16 units than UTF-8 bytes, so `size_of::<JsData>() + 2 * bytes` within
    /// `max_bytes` is enough. Anything else returns `None` for the exact slow path.
    fn export_utf8<'s>(
        &'s self,
        value: ScopedValue<'s>,
        limits: ExportLimits,
    ) -> JsResult<'s, Option<String>> {
        let native = self.read(value)?;
        if self.native_kind(native.value) != ValueKind::String
            || limits.max_nodes == 0
            || limits.max_depth == 0
        {
            return Ok(None);
        }
        let mut length = 0;
        // SAFETY: a protected primitive string; the bytes are copied before any further
        // native entry and released below on every path.
        let bytes = unsafe { ffi::jbq_string_utf8(self.ptr(), native.value, &mut length) };
        if bytes.is_null() {
            return Err(self.exception());
        }
        let fits = length
            .checked_mul(2)
            .and_then(|b| b.checked_add(size_of::<JsData>()))
            .is_some_and(|b| b <= limits.max_bytes);
        let text = fits
            .then(|| {
                // SAFETY: the shim returned `length` valid bytes, alive until the free below.
                let slice = unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), length) };
                std::str::from_utf8(slice).ok().map(str::to_owned)
            })
            .flatten();
        // SAFETY: the bytes came from jbq_string_utf8 of this context, released once.
        unsafe { ffi::jbq_utf8_free(self.ptr(), bytes) };
        Ok(text)
    }
    /// One native copy from UTF-8 (`JS_NewStringLen`); see [`JsScope::import_utf8`].
    fn import_utf8<'s>(&'s self, text: &str) -> JsResult<'s, ScopedValue<'s>> {
        // SAFETY: the bytes are borrowed for this call and native code copies them.
        self.put(unsafe {
            ffi::jbq_string_from_utf8(self.ptr(), text.as_ptr().cast(), text.len())
        })
    }
    fn get<'s>(
        &'s self,
        object: ScopedValue<'s>,
        key: PropertyKey<'_>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        let object = self.read(object)?;
        if let PropertyKey::String(name) = key {
            let atom = self.atom(name)?;
            // SAFETY: the object is protected and the atom is live during native access.
            let value = unsafe { ffi::jbq_get_atom(self.ptr(), object.value, atom.0) };
            self.release_atom(atom);
            return self.put(value);
        }
        let key = self.key(key)?;
        // SAFETY: Both non-consuming input references remain protected during native access.
        self.put(unsafe { ffi::jbq_get(self.ptr(), object.value, key.value) })
    }
    fn set<'s>(
        &'s self,
        object: ScopedValue<'s>,
        key: PropertyKey<'_>,
        value: ScopedValue<'s>,
    ) -> JsResult<'s, ()> {
        let object = self.read(object)?;
        let value = self.read(value)?;
        if let PropertyKey::String(name) = key {
            let atom = self.atom(name)?;
            // SAFETY: the shim duplicates the stored value; the atom is live during the call.
            let status =
                unsafe { ffi::jbq_set_atom(self.ptr(), object.value, atom.0, value.value) };
            self.release_atom(atom);
            return self.check_status(status);
        }
        let key = self.key(key)?;
        // SAFETY: The shim duplicates consumed setter inputs and enforces strict failure.
        self.check_status(unsafe { ffi::jbq_set(self.ptr(), object.value, key.value, value.value) })
    }
    fn call<'s>(
        &'s self,
        function: ScopedValue<'s>,
        receiver: Option<ScopedValue<'s>>,
        arguments: &[ScopedValue<'s>],
    ) -> JsResult<'s, ScopedValue<'s>> {
        let function = self.read(function)?;
        let receiver = receiver.map(|r| self.read(r)).transpose()?;
        let mut inline = [ffi::Value { words: [0; 2] }; INLINE_ARGUMENTS];
        let mut spilled = Vec::new();
        let raw = self.native_arguments(arguments, &mut inline, &mut spilled)?;
        // SAFETY: All values are owned by checked wrappers through this synchronous
        // call. Native argv conversion allocates stable per-invocation storage.
        self.put(unsafe {
            ffi::jbq_call(
                self.ptr(),
                function.value,
                receiver
                    .as_ref()
                    .map_or_else(|| ffi::jbq_undefined(), |v| v.value),
                raw.as_ptr(),
                raw.len(),
            )
        })
    }
    fn construct<'s>(
        &'s self,
        constructor: ScopedValue<'s>,
        arguments: &[ScopedValue<'s>],
        new_target: Option<ScopedValue<'s>>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        let constructor = self.read(constructor)?;
        let target = new_target.map(|r| self.read(r)).transpose()?;
        let mut inline = [ffi::Value { words: [0; 2] }; INLINE_ARGUMENTS];
        let mut spilled = Vec::new();
        let raw = self.native_arguments(arguments, &mut inline, &mut spilled)?;
        // SAFETY: Validated native values remain rooted; the shim borrows them and
        // uses the constructor as new_target only when no explicit target exists.
        self.put(unsafe {
            ffi::jbq_construct(
                self.ptr(),
                constructor.value,
                target.as_ref().map_or(constructor.value, |v| v.value),
                raw.as_ptr(),
                raw.len(),
            )
        })
    }
    fn host_function<'s>(
        &'s self,
        name: &JsString,
        arity: u16,
        function: Rc<dyn HostFunction>,
    ) -> JsResult<'s, ScopedValue<'s>> {
        callbacks::register(self, name, arity, function)
    }
    fn deferred<'s>(&'s self) -> JsResult<'s, Deferred<'s>> {
        // SAFETY: Undefined initializes an out parameter that the shim owns on success.
        let mut pair = unsafe { ffi::jbq_undefined() };
        // SAFETY: pair is writable; the promise and hidden pair are distinct owned outputs.
        let promise = unsafe { ffi::jbq_deferred(self.ptr(), &mut pair) };
        // Protect both native outputs before observing pending finalizer faults:
        // owned(promise) may reject an otherwise successful native allocation.
        let mut pair = NativeValue::new(pair, self.state.raw.clone());
        let promise = self.owned(promise)?;
        pair.resolver = true;
        let capability = self.values.insert(pair)?;
        let resolver = self.values.retain_deferred(capability)?;
        let promise = self.values.insert_shared(promise)?;
        Ok(Deferred { promise, resolver })
    }
    fn settle<'s>(
        &'s self,
        deferred: RootedDeferred,
        settlement: Settlement<'s>,
    ) -> JsResult<'s, ()> {
        let local = self.values.local_deferred(&deferred)?;
        let pair = self.values.read(local)?;
        if !pair.resolver || pair.settled.replace(true) {
            return Err(FailureReport::fault(
                FailureCode::InvalidHandle,
                "invalid or used resolver",
            )
            .into());
        }
        let (reject, value) = match settlement {
            Settlement::Resolve(v) => (0, v),
            Settlement::Reject(v) => (1, v),
        };
        let value = self.read(value)?;
        // SAFETY: The hidden pair is adapter-created and context-checked. Native
        // Promise settlement borrows it/value; the consumed Rust capability is one-use.
        self.owned(unsafe { ffi::jbq_settle(self.ptr(), pair.value, reject, value.value) })?;
        Ok(())
    }
    fn drain_microtasks(&self, hint: NonZeroUsize) -> Result<CheckpointReport, FailureReport> {
        let mut report = CheckpointReport::empty();
        for _ in 0..hint.get() {
            let Some(job) = self.state.raw.next_job() else {
                break;
            };
            // SAFETY: the job came from this context's sink and left the queue above; it runs
            // once, on the owner thread, synchronously under the current entry guard.
            let status = unsafe { ffi::jbq_run_job(self.ptr(), job.as_ptr()) };
            collect_job_result(&mut report, self.detached_status(status))?;
        }
        report.state = if self.state.raw.has_jobs() {
            CheckpointState::Pending
        } else {
            CheckpointState::Empty
        };
        Ok(report)
    }
    fn enqueue_microtask<'s>(&'s self, function: ScopedValue<'s>) -> JsResult<'s, ()> {
        let function = self.read(function)?;
        // SAFETY: The callback stays protected by this frame; the job queue takes its own
        // reference before this call returns.
        let status = unsafe { ffi::jbq_enqueue_job(self.ptr(), function.value) };
        self.check_status(status)
    }
    fn async_context<'s>(&'s self) -> JsResult<'s, ScopedValue<'s>> {
        // SAFETY: The live context returns an owned reference to the current value.
        self.put(unsafe { ffi::jbq_async_context(self.ptr()) })
    }
    fn set_async_context<'s>(&'s self, value: ScopedValue<'s>) -> JsResult<'s, ()> {
        let value = self.read(value)?;
        // SAFETY: The runtime slot takes its own reference; the input stays borrowed.
        unsafe { ffi::jbq_set_async_context(self.ptr(), value.value) };
        self.check_status(0)
    }
    fn take_rejection<'s>(&'s self) -> JsResult<'s, Option<Rejection<'s>>> {
        // SAFETY: Undefined initializes both out parameters; the shim overwrites them with
        // owned references only for an event.
        let (mut promise, mut reason) = unsafe { (ffi::jbq_undefined(), ffi::jbq_undefined()) };
        // SAFETY: Both out parameters are writable and the context is live.
        let status = unsafe { ffi::jbq_take_rejection(self.ptr(), &mut promise, &mut reason) };
        // Own both outputs before any early return so neither reference leaks.
        let promise = Rc::new(NativeValue::new(promise, self.state.raw.clone()));
        let reason = Rc::new(NativeValue::new(reason, self.state.raw.clone()));
        if status == -2 {
            return Err(FailureReport::fault(
                FailureCode::Capacity,
                "raw rejection queue exceeded 1024 events",
            )
            .into());
        }
        self.check_status(status)?;
        if status == 0 {
            return Ok(None);
        }
        Ok(Some(Rejection {
            promise: self.values.insert_shared(promise)?,
            reason: self.values.insert_shared(reason)?,
            handled: status == 2,
        }))
    }
    fn detach_buffer<'s>(&'s self, buffer: ScopedValue<'s>) -> JsResult<'s, ()> {
        let buffer = self.read(buffer)?;
        // SAFETY: The buffer stays protected by this frame during detachment.
        let status = unsafe { ffi::jbq_detach_buffer(self.ptr(), buffer.value) };
        self.check_status(status)
    }
    fn internals<'s>(&'s self, value: ScopedValue<'s>) -> JsResult<'s, Internals<'s>> {
        let value = self.read(value)?;
        // SAFETY: Undefined initializes both out parameters; the shim overwrites them with
        // owned references.
        let (mut first, mut second) = unsafe { (ffi::jbq_undefined(), ffi::jbq_undefined()) };
        // SAFETY: The value stays protected by this frame; both outputs are writable.
        let tag = unsafe { ffi::jbq_internals(self.ptr(), value.value, &mut first, &mut second) };
        let first = Rc::new(NativeValue::new(first, self.state.raw.clone()));
        let second = Rc::new(NativeValue::new(second, self.state.raw.clone()));
        self.check_status(tag)?;
        Ok(match tag {
            1 => Internals::Promise {
                state: 0,
                result: None,
            },
            2 | 3 => Internals::Promise {
                state: (tag - 1) as u8,
                result: Some(self.values.insert_shared(first)?),
            },
            4 => Internals::Proxy {
                target: self.values.insert_shared(first)?,
                handler: self.values.insert_shared(second)?,
            },
            _ => Internals::None,
        })
    }
    fn detach(&self, error: ScopedError<'_>) -> FailureReport {
        let value = match error {
            ScopedError::Exception(value) => value,
            ScopedError::Fault(error) => return error.bounded(),
        };
        let value = match self.read(value) {
            Ok(v) => v,
            Err(e) => return e.bounded(),
        };
        let mut report = FailureReport {
            name: None,
            class: FailureClass::Script,
            code: FailureCode::ScriptException,
            message: None,
            thrown: None,
            stack: Vec::new(),
            truncated: false,
        };
        match self.native_kind(value.value) {
            ValueKind::Undefined => report.thrown = Some(JsData::Undefined),
            ValueKind::Null => report.thrown = Some(JsData::Null),
            ValueKind::Boolean => {
                // SAFETY: Classification proved the primitive Boolean; no coercion occurs.
                report.thrown = Some(JsData::Bool(
                    unsafe { ffi::jbq_to_bool(self.ptr(), value.value) } != 0,
                ));
            }
            ValueKind::Number => {
                let number = match self.number(value.value) {
                    Ok(number) => number,
                    Err(error) => return diagnostic_failure(error),
                };
                report.thrown = Some(JsData::Number(number));
            }
            ValueKind::String => {
                let (text, truncated) = match self.utf16(value.value, 4096, true) {
                    Ok(text) => text,
                    Err(error) => return diagnostic_failure(error),
                };
                report.truncated |= truncated;
                report.message = Some(text.clone());
                report.thrown = Some(JsData::String(text));
            }
            _ => {
                let mut fields = [None, None];
                for (slot, (field, maximum)) in fields.iter_mut().zip([4096, 16384]).enumerate() {
                    *field = match self.diagnostic_field(value.value, slot as i32, maximum) {
                        Ok(field) => field,
                        Err(error) => return error,
                    };
                }
                let [message, stack] = fields;
                match self.diagnostic_field(value.value, 2, 256) {
                    Ok(Some((text, truncated))) => {
                        report.name = Some(text);
                        report.truncated |= truncated;
                    }
                    Ok(None) => {}
                    Err(error) => return error,
                }
                if let Some((text, truncated)) = message {
                    report.message = Some(text);
                    report.truncated |= truncated;
                }
                if let Some((text, truncated)) = stack {
                    report.truncated |= truncated;
                    if let Ok(text) = text.to_utf8() {
                        report.stack = stack_frames(&text);
                    } else {
                        report.truncated = true;
                    }
                }
            }
        }
        report.bounded()
    }
    fn source(&self) -> Option<&dyn SourceApi> {
        Some(self)
    }
    fn compiler(&self) -> Option<&dyn CompilerApi> {
        Some(self)
    }
    fn artifacts(&self) -> Option<&dyn ArtifactApi> {
        Some(self)
    }
    fn modules(&self) -> Option<&dyn ModuleApi> {
        Some(self)
    }
}
/// Convert the checked insertion result without losing a typed store failure.
fn scoped_exception(result: Result<ScopedValue<'_>, FailureReport>) -> ScopedError<'_> {
    match result {
        Ok(value) => ScopedError::Exception(value),
        Err(error) => ScopedError::Fault(error),
    }
}
/// Reserve host-copy storage before reading a native allocation.
pub(super) fn copied_buffer<T>(count: usize, message: &str) -> Result<Vec<T>, FailureReport> {
    let mut copied = Vec::new();
    copied
        .try_reserve_exact(count)
        .map_err(|_| FailureReport::fault(FailureCode::Capacity, message))?;
    Ok(copied)
}
/// Retain bounded script job failures and stop immediately on a fatal fault.
fn collect_job_result(
    report: &mut CheckpointReport,
    result: Result<(), FailureReport>,
) -> Result<(), FailureReport> {
    if let Err(failure) = result {
        if failure.class != FailureClass::Script {
            return Err(failure);
        }
        if report.failures.len() < 64 {
            report.failures.push(failure);
        } else {
            report.suppressed_failures = report.suppressed_failures.saturating_add(1);
        }
    }
    Ok(())
}
// Do not recurse into diagnostics after diagnostics themselves exhaust native
// resources. Preserve any typed host/engine fault rather than masking it as script.
fn diagnostic_failure(error: ScopedError<'_>) -> FailureReport {
    match error {
        ScopedError::Fault(error) => error.bounded(),
        ScopedError::Exception(_) => FailureReport::new(
            FailureClass::Engine,
            FailureCode::EngineFailure,
            "native diagnostic extraction failed",
        ),
    }
}
impl SourceApi for Scope<'_, '_> {
    fn evaluate<'s>(&'s self, source: &ScriptSource) -> JsResult<'s, ScopedValue<'s>> {
        let origin = self.source_origin(source)?;
        self.state
            .debug
            .stage_host_script(source, crate::debug::ScriptKind::Script)?;
        let compiled = self.compile_registered(source, &origin, false)?;
        // SAFETY: the scope owns `compiled`; the bridge runs a duplicate of it.
        self.put(unsafe { ffi::jbq_eval_compiled(self.ptr(), compiled.value) })
    }
}
impl CompilerApi for Scope<'_, '_> {
    fn compile(
        &self,
        source: &ScriptSource,
        options: CompileOptions,
    ) -> Result<CompiledArtifact, FailureReport> {
        self.compile_policy(&options)?;
        let origin = self.source_origin(source)?;
        self.state
            .debug
            .stage_host_script(source, crate::debug::ScriptKind::Script)?;
        let compiled = self
            .compile_registered(source, &origin, options.debug_info == DebugInfo::Required)
            .map_err(|e| self.detach(e))?;
        let payload =
            self.compiled_payload(&compiled, options.debug_info == DebugInfo::Required)?;
        // SAFETY: Bytes are unmodified output of this pinned native compiler and
        // metadata records exact target/build/options and original source digest.
        unsafe {
            CompiledArtifact::from_trusted_compiler_output(
                self.artifact_metadata(source, options),
                payload,
                source.clone(),
            )
        }
    }
}
impl ArtifactApi for Scope<'_, '_> {
    fn execute<'s>(&'s self, artifact: &CompiledArtifact) -> JsResult<'s, ScopedValue<'s>> {
        self.compile_policy(&artifact.metadata().options)?;
        self.source_origin(artifact.source())?;
        artifact.validate(
            &self.artifact_metadata(artifact.source(), artifact.metadata().options.clone()),
            self.state.debug.is_development(),
        )?;
        let registration = self
            .state
            .debug
            .register_host_source(artifact.source(), crate::debug::ScriptKind::Script)
            .map_err(super::debug::failure)?;
        // SAFETY: Opaque provenance and exact build/options/digest compatibility
        // were checked before native deserialization; payload borrows the artifact.
        let compiled = self.owned(unsafe {
            ffi::jbq_read(
                self.ptr(),
                artifact.payload().as_ptr(),
                artifact.payload().len(),
            )
        })?;
        self.state
            .debug
            .loaded(compiled.value, registration)
            .map_err(super::debug::failure)?;
        // SAFETY: The shim duplicates this borrowed compiled value before consuming it.
        self.put(unsafe { ffi::jbq_eval_compiled(self.ptr(), compiled.value) })
    }
}
fn stack_frames(text: &str) -> Vec<StackFrameReport> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim().strip_prefix("at ")?;
            let (function, location) = match line.rsplit_once(" (") {
                Some((name, loc)) => (Some(JsString::from_utf8(name)), loc.trim_end_matches(')')),
                None => (None, line),
            };
            let (prefix, last) = location.rsplit_once(':')?;
            let last = last.parse::<u32>().ok()?;
            let (origin, line, column) = match prefix
                .rsplit_once(':')
                .and_then(|(origin, n)| n.parse::<u32>().ok().map(|n| (origin, n)))
            {
                Some((origin, line)) => (origin, line, Some(last.saturating_sub(1))),
                None => (prefix, last, None),
            };
            Some(StackFrameReport {
                function,
                location: Some(SourceLocation {
                    origin: origin.into(),
                    line: Some(line.saturating_sub(1)),
                    column,
                }),
            })
        })
        // Keep one additional frame so FailureReport::bounded can mark omitted
        // frames while enforcing the published 64-frame limit.
        .take(65)
        .collect()
}

#[cfg(test)]
#[path = "../../../tests/unit/quickjs_ng_scope_coverage.rs"]
mod tests;

impl QuickJsNgEngine {
    /// Script artifact policy retaining native lexical and statement debug information.
    pub fn debug_compile_options() -> CompileOptions {
        CompileOptions {
            mode: CodeMode::Script,
            debug_info: DebugInfo::Required,
            fingerprint: "quickjs:debug".into(),
        }
    }
}
