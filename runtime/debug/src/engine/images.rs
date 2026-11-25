//! Source-free distribution images and source-preserving local execution caches.
use super::*;
use std::sync::Arc;

/// Exact native module-image compatibility identity; equality never implies trust.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ImageProfile {
    /// Stable adapter identity.
    pub adapter_id: String,
    /// Pinned native revision.
    pub native_revision: String,
    /// Adapter-defined native image ABI.
    pub format_version: u32,
    /// Architecture and platform identity.
    pub target_fingerprint: String,
    /// Compiler flags, patches, and bridge identity.
    pub build_fingerprint: String,
    /// Whether native diagnostic positions are retained.
    pub debug_info: bool,
}
/// Trusted compiler output without retained original source.
#[derive(Clone, Debug)]
pub struct CompiledModuleImage {
    profile: ImageProfile,
    payload: Arc<[u8]>,
    imports: Vec<JsString>,
}
impl CompiledModuleImage {
    /// Admits trusted compiler output without interpreting native bytes.
    ///
    /// # Safety
    /// Bytes must be unmodified source-free native distribution output for this exact profile.
    /// Imports must be the compiler's complete ordered unique static requests.
    pub unsafe fn from_trusted_compiler_output(
        profile: ImageProfile,
        payload: Vec<u8>,
        imports: Vec<JsString>,
    ) -> Self {
        Self {
            profile,
            payload: payload.into(),
            imports,
        }
    }
    /// Borrows exact native compatibility metadata.
    pub fn profile(&self) -> &ImageProfile {
        &self.profile
    }
    /// Borrows trusted native bytes without granting arbitrary-byte loading.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    /// Borrows native static import requests.
    pub fn imports(&self) -> &[JsString] {
        &self.imports
    }
}
/// Detached source-preserving execution cache, distinct from source-free distribution output.
/// It contains no native handles and cannot be passed to PackageBuilder as a compiled graph image.
///
/// ```compile_fail
/// use coldmind_runtime::engine::{CompiledModuleImage, SourceModuleCache};
/// fn distribution(cache: SourceModuleCache) -> CompiledModuleImage { cache }
/// ```
#[derive(Clone, Debug)]
pub struct SourceModuleCache {
    profile: ImageProfile,
    payload: Arc<[u8]>,
    imports: Vec<JsString>,
    source_digest: SourceDigest,
    identity: JsString,
    origin: String,
}
impl SourceModuleCache {
    /// Admits genuine compiler output preserving the source module's observable semantics.
    ///
    /// # Safety
    /// Payload must be native compiler output for this exact profile and the source identified
    /// by the digest, module identity and origin, with complete ordered imports. It must preserve source-sensitive behavior,
    /// including Function.prototype.toString, and remain valid after the compiler context closes.
    pub unsafe fn from_compiler_output(
        profile: ImageProfile,
        payload: Vec<u8>,
        imports: Vec<JsString>,
        source_digest: SourceDigest,
        identity: JsString,
        origin: String,
    ) -> Self {
        Self {
            profile,
            payload: payload.into(),
            imports,
            source_digest,
            identity,
            origin,
        }
    }
    /// Borrows exact native cache compatibility metadata.
    pub fn profile(&self) -> &ImageProfile {
        &self.profile
    }
    /// Borrows genuine native execution bytes, which may contain original source.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    /// Borrows the compiler's complete static request table.
    pub fn imports(&self) -> &[JsString] {
        &self.imports
    }
    /// Returns whether the cache belongs to this exact module text and diagnostic origin.
    pub fn matches_source(&self, identity: &JsString, source: &ScriptSource) -> bool {
        &self.identity == identity
            && self.origin == source.origin
            && self.source_digest == SourceDigest::of_utf16(&source.text)
    }
    /// Returns the compiler-attested identity of the original source text.
    pub fn source_digest(&self) -> SourceDigest {
        self.source_digest
    }
}
/// Immutable authorized native image: authenticated distribution bytes or a local compiler cache.
/// Public distribution admission still requires package verification. Compiler caches are
/// converted only inside the checked local-cache loader and never become package graph images.
#[derive(Clone, Debug)]
pub struct VerifiedModuleImage {
    pub(crate) profile: ImageProfile,
    pub(crate) payload: Arc<[u8]>,
    pub(crate) imports: Vec<JsString>,
    /// From the binary's own builtin package: Development loads it without source, and the
    /// debugger sees its frames without a location.
    pub(crate) platform: bool,
}
impl VerifiedModuleImage {
    /// Whether the image is builtin platform code rather than application code.
    pub fn is_platform(&self) -> bool {
        self.platform
    }
    /// Borrows authenticated compatibility metadata.
    pub fn profile(&self) -> &ImageProfile {
        &self.profile
    }
    /// Borrows verified, decompressed native bytes.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    /// Borrows authenticated import declarations for native cross-checking.
    pub fn imports(&self) -> &[JsString] {
        &self.imports
    }
}
/// Optional native image compiler/loader; bound to the active owner-local scope.
pub trait ModuleImageApi {
    /// Returns this context's actual image compatibility identity.
    fn profile(&self) -> ImageProfile;
    /// Serializes a prepared, uninstantiated module without application execution.
    fn compile(&self, module: &ModuleId) -> Result<CompiledModuleImage, FailureReport>;
    /// Optionally compiles a source-preserving local cache; adapters without it return None.
    fn compile_source_cache(
        &self,
        _module: &ModuleId,
    ) -> Result<Option<SourceModuleCache>, FailureReport> {
        Ok(None)
    }
    /// Loads a trusted local cache with the same native checks as authenticated image loading.
    fn prepare_source_cache(
        &self,
        cache: &SourceModuleCache,
        identity: &JsString,
    ) -> Result<ModuleId, FailureReport> {
        if cache.profile() != &self.profile() || &cache.identity != identity {
            return Err(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "source cache profile mismatch",
            ));
        }
        self.prepare(
            &VerifiedModuleImage {
                profile: cache.profile.clone(),
                payload: cache.payload.clone(),
                imports: cache.imports.clone(),
                platform: false,
            },
            identity,
        )
    }
    /// Loads only authorized compatible images and checks native imports before linking.
    fn prepare(
        &self,
        image: &VerifiedModuleImage,
        identity: &JsString,
    ) -> Result<ModuleId, FailureReport>;
}
/// Native evaluation progress. Host/adapter faults remain the outer error.
#[derive(Clone, Debug, PartialEq)]
pub enum ModuleEvaluationState {
    /// Evaluation has not begun.
    NotStarted,
    /// Native evaluation promise has not settled.
    Pending,
    /// Evaluation completed successfully.
    Fulfilled,
    /// Application evaluation rejected with a detached script failure.
    Rejected(FailureReport),
}
/// Context-independent definition, materialized once per native context by identity.
/// Keep one `Rc` for values shared by globals, modules, or re-exports.
pub enum HostValueDefinition {
    /// Owner-local callable implementation with native name and length metadata.
    Function {
        /// Native function name.
        name: JsString,
        /// Declared callable arity.
        arity: u16,
        /// Owner-local host implementation.
        function: std::rc::Rc<dyn HostFunction>,
    },
    /// Copied data; objects retain identity after their first materialization.
    Data(JsData),
    /// Generic context-local initialization, allowed to use other definitions.
    Factory(std::rc::Rc<dyn HostValueFactory>),
}
/// Creates a value in its owner context, without retaining scoped handles.
/// Recursive materialization of the same definition fails; errors and panics
/// discard the incomplete entry so a later attempt can retry.
pub trait HostValueFactory {
    /// Creates the native value; no cache borrow is held during this call.
    fn create<'s>(&self, scope: &'s dyn JsScope) -> JsResult<'s, ScopedValue<'s>>;
}
/// Named native builtin export. Cloning preserves the value definition's identity.
#[derive(Clone)]
pub struct BuiltinExport {
    /// Lossless exported name.
    pub name: JsString,
    /// Context-independent value definition.
    pub value: std::rc::Rc<HostValueDefinition>,
}

/// Checked owner-local metadata for one outstanding dynamic import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleImportId {
    /// Native context that owns the import token.
    pub context: ContextId,
    /// Never-reused adapter sequence; adapters validate it before token access.
    pub sequence: u64,
}
/// Bounded detached dynamic import request. Providers consume it outside native entry.
#[derive(Clone, Debug)]
pub struct ModuleImportRequest {
    /// Context-bound completion identity.
    pub id: ModuleImportId,
    /// Native canonical referring module identity.
    pub referrer: JsString,
    /// Lossless requested specifier.
    pub specifier: JsString,
}

/// Owner-local import extraction result, including rejected requests consumed by an adapter.
#[derive(Clone, Debug, Default)]
pub struct ModuleImportBatch {
    /// Valid detached requests awaiting platform resolution.
    pub requests: Vec<ModuleImportRequest>,
    /// True when extraction consumed any native request, even one rejected locally.
    /// Its rejection checkpoint can enqueue subsequent imports, so the VM must keep pumping.
    pub made_progress: bool,
}
