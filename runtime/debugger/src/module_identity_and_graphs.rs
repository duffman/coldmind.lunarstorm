//! Engine-independent module identity, providers, and bounded owner-local graphs.
#[path = "module_identity_and_graphs/module_providers.rs"]
mod providers;
#[path = "module_identity_and_graphs/confined_source_tree.rs"]
mod vfs;
use crate::engine::*;
pub use providers::{FileModuleProvider, MemoryModuleProvider};
use std::{fmt, rc::Rc};
pub(crate) use vfs::{Kind as FileKind, VirtualDir};

/// Lossless canonical module identity, stable for a VM's lifetime.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModuleKey(JsString);
impl ModuleKey {
    /// Admits a nonempty bounded identity without normalizing JavaScript code units.
    pub fn new(text: JsString) -> Result<Self, ModuleError> {
        if text.as_units().is_empty() || text.as_units().len() > 4096 {
            return Err(ModuleError::Limit);
        }
        Ok(Self(text))
    }
    /// Converts a valid UTF-8 identity to lossless code units.
    pub fn from_utf8(text: &str) -> Result<Self, ModuleError> {
        Self::new(JsString::from_utf8(text))
    }
    /// Borrows canonical identity units.
    pub fn text(&self) -> &JsString {
        &self.0
    }
}
/// Detached platform failure; NotFound alone permits configured builtin fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleError {
    /// No explicitly available module matches this request.
    NotFound,
    /// Invalid identity, path, or conflicting registration.
    InvalidIdentity,
    /// A provider capability or graph budget rejected the operation.
    Limit,
    /// Filesystem I/O failed; no ambient fallback is attempted.
    Io(String),
    /// Package authentication, compatibility, or authorization failed.
    Package(String),
}
impl fmt::Display for ModuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ModuleError {}
/// Selected engine metadata supplied to every provider load.
#[derive(Clone, Debug)]
pub struct ModuleLoadRequest {
    /// Previously resolved canonical identity.
    pub key: ModuleKey,
    /// Actual active adapter profile; absent when native images are unsupported.
    pub image_profile: Option<ImageProfile>,
}
/// One immutable graph node's source or authenticated native representation.
#[derive(Clone, Debug)]
pub enum ModuleInput {
    /// Lossless source and supplied diagnostic metadata.
    Source(ScriptSource),
    /// Source-free authorized native image.
    Image(VerifiedModuleImage),
}
/// Owner-local platform provider. Calls occur outside native scopes.
pub trait ModuleProvider {
    /// Resolves an import relative to its canonical referrer.
    fn resolve(&self, referrer: &ModuleKey, specifier: &JsString)
    -> Result<ModuleKey, ModuleError>;
    /// Loads immutable content for the selected engine without native execution.
    fn load(&self, request: &ModuleLoadRequest) -> Result<ModuleInput, ModuleError>;
}
/// Key prefix of an embedding application's own modules (`host:db`).
///
/// Every host module lives under this one scheme, so none can take a `jetbunny:` builtin's
/// or a package's key. A compiled package records a `host:*` import as external: the host
/// that runs the package must register the module.
pub const HOST_MODULE_PREFIX: &str = "host:";

/// Host-defined native module, registered explicitly for one VM.
#[derive(Clone)]
pub struct BuiltinModule {
    /// Canonical identity used by explicit fallback resolution.
    pub key: ModuleKey,
    /// Native named exports; no JavaScript source wrapper is generated.
    pub exports: Vec<BuiltinExport>,
}
/// Positive graph budgets, independent of native heap and copied-data quotas.
#[derive(Clone, Copy, Debug)]
pub struct ModuleLimits {
    /// Maximum distinct prepared nodes, including builtins.
    pub nodes: usize,
    /// Maximum total static import edges.
    pub edges: usize,
    /// Maximum aggregate original UTF-16 source bytes.
    pub source_bytes: usize,
    /// Maximum code units in each native import specifier.
    pub specifier_units: usize,
}
impl Default for ModuleLimits {
    fn default() -> Self {
        Self {
            nodes: 4096,
            edges: 65536,
            source_bytes: 64 * 1024 * 1024,
            specifier_units: 4096,
        }
    }
}
impl ModuleLimits {
    /// Rejects zero capacities before loading providers or entering native code.
    pub fn validate(&self) -> Result<(), ModuleError> {
        if [
            self.nodes,
            self.edges,
            self.source_bytes,
            self.specifier_units,
        ]
        .contains(&0)
        {
            Err(ModuleError::Limit)
        } else {
            Ok(())
        }
    }
}
/// Platform graph configuration owned by platform admission.
#[derive(Clone)]
pub struct ModuleConfig {
    /// Memory, confined file, package, or application provider.
    pub provider: Rc<dyn ModuleProvider>,
    /// Explicit host modules; duplicate identities are rejected.
    pub builtins: Vec<BuiltinModule>,
    /// Graph resource bounds.
    pub limits: ModuleLimits,
}
/// Detached compile-only graph node, safe to retain after compiler VM teardown.
#[derive(Clone, Debug)]
pub struct CompiledModule {
    /// Canonical graph identity.
    pub key: ModuleKey,
    /// Ordered lossless static bindings.
    pub imports: Vec<(JsString, ModuleKey)>,
    /// Trusted native image with no retained source.
    pub image: CompiledModuleImage,
}
/// Engine-specific graph variant consumed by the package builder.
#[derive(Clone, Debug)]
pub struct CompiledModuleGraph {
    /// Canonical entry identity.
    pub entry: ModuleKey,
    /// Source/native-image nodes; builtins remain external declarations.
    pub modules: Vec<CompiledModule>,
    /// External builtin identities required when mounting the graph.
    pub builtins: Vec<ModuleKey>,
}
/// Rooted native execution handles; no promise is implicitly awaited.
pub struct ModuleRun {
    /// Context-bound entry module handle.
    pub id: ModuleId,
    /// Native live-binding namespace.
    pub namespace: RootedValue,
    /// Native evaluation result or promise.
    pub evaluation: RootedValue,
}

/// Frozen package membership and explicit runtime aliases, independent of engine contexts.
#[derive(Clone, Debug)]
pub struct ModuleResolutionScope {
    /// Stable logical package scope identity within one distribution.
    pub id: String,
    /// Module identities whose relative imports remain within this scope.
    pub modules: Vec<ModuleKey>,
    /// Finite resolved private, self and dependency requests for dynamic imports.
    pub aliases: Vec<(JsString, ModuleKey)>,
    /// Canonical root-relative source path aliases, sharing the target's native instance.
    pub paths: Vec<(String, ModuleKey)>,
}
