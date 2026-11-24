use super::*;
use sha2::{Digest, Sha256};

/// SHA-256 identity of lossless UTF-16LE source units without a byte-order mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceDigest([u8; 32]);
impl SourceDigest {
    /// Hashes each original code unit in little-endian order without normalization.
    pub fn of_utf16(text: &JsString) -> Self {
        let mut hash = Sha256::new();
        for unit in text.as_units() {
            hash.update(unit.to_le_bytes());
        }
        Self(hash.finalize().into())
    }
    /// Borrows the 32-byte SHA-256 digest.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
/// Host-supplied original source for a source-map attachment; never fetched implicitly.
#[derive(Clone, Debug)]
pub struct OriginalSource {
    /// Canonical UTF-8 URI of the original source.
    pub origin: String,
    /// Lossless original text, or absent when unavailable.
    pub text: Option<JsString>,
}
/// Owned Source Map v3 input and optional transformation-chain metadata.
///
/// A host debug coordinator must validate maps and source identities before
/// registration. Attachments do not imply that an adapter implements source maps.
#[derive(Clone, Debug)]
pub struct SourceMapAttachment {
    /// Generated source’s Source Map v3 JSON, including indexed maps when supported.
    pub json: Vec<u8>,
    /// Host-supplied original source identities and optional content.
    pub originals: Vec<OriginalSource>,
    /// Supplied preceding transformation maps in chain order; no network fetch is implied.
    pub preceding_maps: Vec<Vec<u8>>,
}
/// Lossless source text, stable origin, and optional host-supplied debug mapping.
///
/// The origin is supplied by the host/adapter; distinct URIs are not silently
/// merged using filesystem case or symlink assumptions.
#[derive(Clone, Debug)]
pub struct ScriptSource {
    /// Original JavaScript source units.
    pub text: JsString,
    /// Stable canonical UTF-8 source URI used for diagnostics and source identity.
    pub origin: String,
    /// Optional supplied source mapping; invalid or unsupported maps must fail admission.
    pub source_map: Option<SourceMapAttachment>,
}
impl ScriptSource {
    /// Takes source text and origin without attaching or fetching a source map.
    pub fn new(text: JsString, origin: String) -> Self {
        Self {
            text,
            origin,
            source_map: None,
        }
    }
}
/// Native compilation goal for a source unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeMode {
    /// Compile as a standalone script.
    Script,
    /// Compile as a native module when the compiler supports it.
    Module,
}
/// Required native debug-information policy recorded in artifact compatibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugInfo {
    /// Emit native metadata sufficient for the development debugger baseline.
    Required,
    /// Permit stripped debug information; incompatible with development loading.
    Strip,
}
/// Compilation goal and adapter-defined options identity.
///
/// Development builds require debug metadata; adapters reject incompatible options
/// before compilation rather than silently weakening the profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileOptions {
    /// Script or module compilation goal.
    pub mode: CodeMode,
    /// Native debug-information policy.
    pub debug_info: DebugInfo,
    /// Adapter-defined identity of all other relevant compilation options.
    pub fingerprint: String,
}
/// Complete compatibility identity required before native artifact loading.
///
/// Matching metadata establishes compatibility, not validation of adversarial
/// bytecode; trusted compiler/cache provenance remains a separate obligation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactMetadata {
    /// Stable identity of the compiler/engine adapter.
    pub adapter_id: String,
    /// Exact native engine version or revision.
    pub native_version: String,
    /// Native artifact-format version.
    pub format_version: u32,
    /// Target architecture/platform compatibility identity.
    pub target_fingerprint: String,
    /// Native build flags, patches, and debug-information configuration identity.
    pub build_fingerprint: String,
    /// Compilation goal, debug policy, and options fingerprint.
    pub options: CompileOptions,
    /// Digest of the retained original lossless source text.
    pub source_digest: SourceDigest,
}
/// Opaque compiler output. No safe constructor accepts arbitrary native bytecode.
#[derive(Debug)]
pub struct CompiledArtifact {
    metadata: ArtifactMetadata,
    payload: Vec<u8>,
    source: ScriptSource,
}
impl CompiledArtifact {
    /// Takes native compiler output and its retained source without deserializing it.
    ///
    /// Returns `InvalidArtifact` if the source digest does not match. This check
    /// does not establish the payload provenance required by the safety contract.
    ///
    /// # Safety
    ///
    /// `payload` must be unmodified output of the trusted native compiler described
    /// by `metadata`. The adapter must check compatibility before deserialization.
    /// Fingerprints alone do not validate untrusted bytecode.
    pub unsafe fn from_trusted_compiler_output(
        metadata: ArtifactMetadata,
        payload: Vec<u8>,
        source: ScriptSource,
    ) -> Result<Self, FailureReport> {
        if metadata.source_digest != SourceDigest::of_utf16(&source.text) {
            return Err(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "source digest mismatch",
            ));
        }
        Ok(Self {
            metadata,
            payload,
            source,
        })
    }
    /// Borrows immutable compatibility metadata.
    pub fn metadata(&self) -> &ArtifactMetadata {
        &self.metadata
    }
    /// Borrows opaque trusted native compiler output; this does not grant a safe loader.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    /// Borrows the retained source and supplied source-map attachment.
    pub fn source(&self) -> &ScriptSource {
        &self.source
    }
    /// Checks exact metadata equality, retained-source digest, and debug policy.
    ///
    /// Development loading rejects stripped debug information. Returns
    /// `InvalidArtifact` on mismatch, before a caller may deserialize native bytes.
    /// Success is a compatibility check, not a verifier for untrusted payloads.
    pub fn validate(
        &self,
        expected: &ArtifactMetadata,
        development: bool,
    ) -> Result<(), FailureReport> {
        if &self.metadata != expected
            || self.metadata.source_digest != SourceDigest::of_utf16(&self.source.text)
            || (development && self.metadata.options.debug_info != DebugInfo::Required)
        {
            return Err(FailureReport::fault(
                FailureCode::InvalidArtifact,
                "incompatible artifact",
            ));
        }
        Ok(())
    }
}
/// Opaque owner-local identity of an adapter-owned prepared native module.
///
/// The context identity binds it to one engine/context. Issuing an ID does not
/// create or retain native module storage; adapters must validate before use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleId {
    pub(crate) context: ContextId,
    pub(crate) slot: u64,
}
impl ModuleId {
    /// Returns the engine/context identity that owns this module.
    pub fn context(self) -> ContextId {
        self.context
    }
    /// Returns opaque module-table metadata for its adapter, never a native pointer.
    pub fn slot(self) -> u64 {
        self.slot
    }
}
/// Host-resolved mapping from one import specifier to a prepared native module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleBinding {
    /// Lossless imported module specifier.
    pub specifier: JsString,
    /// Prepared target module, required to belong to the linking context.
    pub module: ModuleId,
}
/// Optional native module semantics advertised independently of basic module support.
#[derive(Clone, Copy, Debug, Default)]
pub struct ModuleFeatures {
    /// Whether native module evaluation supports top-level `await`.
    pub top_level_await: bool,
    /// Whether the adapter supports native dynamic `import()` with host resolution.
    pub dynamic_import: bool,
}
/// Optional source-entry interface bound to the current value scope.
pub trait SourceApi {
    /// Executes source in the current context and returns its native result.
    ///
    /// Returned values/exceptions borrow this scope. Source/debug admission must
    /// finish before application execution; no implicit microtask checkpoint runs.
    fn evaluate<'s>(&'s self, source: &ScriptSource) -> JsResult<'s, ScopedValue<'s>>;
}
/// Optional trusted native compiler interface; compilation executes no application code.
pub trait CompilerApi {
    /// Compiles with validated options and retains required source/debug metadata.
    ///
    /// Returns a detached failure for unsupported options, capacity, syntax, or
    /// native compiler errors; the result must originate from a trusted compiler.
    fn compile(
        &self,
        source: &ScriptSource,
        options: CompileOptions,
    ) -> Result<CompiledArtifact, FailureReport>;
}
/// Optional native artifact execution interface independent of compiler availability.
pub trait ArtifactApi {
    /// Validates compatibility and trusted provenance before native deserialization.
    ///
    /// Returns the scope-bound native result/exception, without an implicit
    /// checkpoint. Incompatible/stripped development artifacts fail before entry.
    fn execute<'s>(&'s self, artifact: &CompiledArtifact) -> JsResult<'s, ScopedValue<'s>>;
}
/// Optional native module preparation, host linking, and execution interface.
///
/// Every module handle is context-bound. These operations do not supply host
/// resolution, a graph cache, bundling, or a second application scheduler.
pub trait ModuleApi {
    /// Returns the stable optional module semantics supported for this context.
    fn features(&self) -> ModuleFeatures;
    /// Prepares native module code under a host-supplied identity without evaluating it.
    ///
    /// Returns detached preparation/admission/capacity errors; the module table and
    /// its native lifetimes remain adapter-owned.
    fn prepare(&self, source: &ScriptSource, key: &JsString) -> Result<ModuleId, FailureReport>;
    /// Returns the prepared module’s lossless static import specifiers.
    ///
    /// Rejects foreign, stale, or closed-context module IDs before native access.
    fn imports(&self, module: &ModuleId) -> Result<Vec<JsString>, FailureReport>;
    /// Records complete static edges before graph instantiation, permitting cycles.
    /// Repeating identical bindings is allowed; conflicting rebinding must fail.
    fn bind(&self, module: &ModuleId, bindings: &[ModuleBinding]) -> Result<(), FailureReport>;
    /// Returns the native namespace, preserving identity and live bindings.
    fn namespace<'s>(&'s self, module: &ModuleId) -> JsResult<'s, ScopedValue<'s>>;
    /// Observes actual native evaluation state without running a checkpoint.
    fn evaluation_state(&self, module: &ModuleId) -> Result<ModuleEvaluationState, FailureReport>;
    /// Defines an owner-local host module; native linking validates named exports.
    fn define_builtin(
        &self,
        identity: &JsString,
        exports: &[BuiltinExport],
    ) -> Result<ModuleId, FailureReport>;
    /// Discovers native image support independently of source-module support.
    fn images(&self) -> Option<&dyn ModuleImageApi>;
    /// Drains bounded owner-local requests when dynamic imports are supported.
    /// No provider or worker is invoked in this scope. Unsupported adapters return no requests.
    fn take_dynamic_imports(&self) -> Result<ModuleImportBatch, FailureReport> {
        Ok(ModuleImportBatch::default())
    }
    /// Completes one checked import with an evaluated native module or a detached load failure.
    /// Native tokens must be consumed exactly once, including error and shutdown paths.
    fn finish_dynamic_import(
        &self,
        _id: ModuleImportId,
        _result: Result<ModuleId, FailureReport>,
    ) -> Result<(), FailureReport> {
        Err(FailureReport::fault(
            FailureCode::Unsupported,
            "dynamic imports unavailable",
        ))
    }
    /// Links static imports through explicit host-provided bindings.
    ///
    /// Rejects missing or foreign targets before exposing a successful link. No
    /// network/module resolution or application evaluation is implicit.
    fn link(&self, module: &ModuleId, bindings: &[ModuleBinding]) -> Result<(), FailureReport>;
    /// Evaluates a validated native module and returns its result or native Promise.
    ///
    /// The result/exception borrows the scope; normal explicit checkpoint ordering
    /// applies. Top-level await/dynamic import require advertised feature support.
    fn evaluate<'s>(&'s self, module: &ModuleId) -> JsResult<'s, ScopedValue<'s>>;
}
