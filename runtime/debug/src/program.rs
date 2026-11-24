//! Immutable, context-independent handoff from platform admission to VM execution.
use crate::{engine::*, modules::*};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
};

/// Detached failure before a Program is admitted for execution.
#[derive(Clone, Debug, PartialEq)]
pub enum ProgramError {
    /// A provider rejected resolution or content admission.
    Provider(ModuleError),
    /// Compile-only analysis failed; no application body was executed.
    Analysis(FailureReport),
    /// Entry, identities, bindings or declarations are inconsistent.
    InvalidGraph,
    /// A configured positive graph budget was exceeded.
    Limit,
    /// The selected adapter lacks the required analysis/image capability.
    Unsupported,
}
impl fmt::Display for ProgramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ProgramError {}
impl From<ModuleError> for ProgramError {
    fn from(error: ModuleError) -> Self {
        Self::Provider(error)
    }
}
impl From<FailureReport> for ProgramError {
    fn from(error: FailureReport) -> Self {
        Self::Analysis(error)
    }
}
/// A platform-resolved static edge. No native handle or resolver is retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedImport {
    specifier: JsString,
    target: ModuleKey,
}
impl ResolvedImport {
    /// Records a resolved request. Program admission validates uniqueness and target membership.
    pub fn new(specifier: JsString, target: ModuleKey) -> Self {
        Self { specifier, target }
    }
    /// Borrows the original lossless request.
    pub fn specifier(&self) -> &JsString {
        &self.specifier
    }
    /// Borrows the canonical resolved target.
    pub fn target(&self) -> &ModuleKey {
        &self.target
    }
}
/// Frozen code and resolved bindings for one logical module.
#[derive(Clone, Debug)]
pub struct ProgramModule {
    key: ModuleKey,
    code: Arc<ModuleInput>,
    imports: Arc<[ResolvedImport]>,
    fingerprint: [u8; 32],
    compiled: Option<CompiledModuleImage>,
    cache: Option<SourceModuleCache>,
}
impl ProgramModule {
    /// Takes a code snapshot and explicit bindings; no engine entry occurs.
    pub fn new(key: ModuleKey, code: ModuleInput, imports: Vec<ResolvedImport>) -> Self {
        let mut module = Self {
            key,
            code: Arc::new(code),
            imports: imports.into(),
            fingerprint: [0; 32],
            compiled: None,
            cache: None,
        };
        module.fingerprint = module.compute_fingerprint();
        module
    }
    /// Borrows canonical identity.
    pub fn key(&self) -> &ModuleKey {
        &self.key
    }
    /// Borrows immutable source or authenticated native code.
    pub fn code(&self) -> &ModuleInput {
        &self.code
    }
    /// Borrows complete resolved bindings.
    pub fn imports(&self) -> &[ResolvedImport] {
        &self.imports
    }
    pub(crate) fn with_compiled(mut self, image: Option<CompiledModuleImage>) -> Self {
        self.compiled = image;
        self
    }
    pub(crate) fn with_cache(mut self, cache: Option<SourceModuleCache>) -> Self {
        self.cache = cache;
        self
    }
    pub(crate) fn source_cache(&self) -> Option<&SourceModuleCache> {
        self.cache.as_ref()
    }
    pub(crate) fn has_compiled(&self) -> bool {
        self.compiled.is_some()
    }
    pub(crate) fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
    fn compute_fingerprint(&self) -> [u8; 32] {
        fn bytes(h: &mut Sha256, b: &[u8]) {
            h.update((b.len() as u64).to_le_bytes());
            h.update(b)
        }
        fn text(h: &mut Sha256, t: &JsString) {
            h.update(SourceDigest::of_utf16(t).as_bytes())
        }
        let mut h = Sha256::new();
        text(&mut h, self.key.text());
        match self.code.as_ref() {
            ModuleInput::Source(s) => {
                h.update([0]);
                text(&mut h, &s.text);
                bytes(&mut h, s.origin.as_bytes());
                if let Some(map) = &s.source_map {
                    h.update([1]);
                    bytes(&mut h, &map.json);
                    h.update((map.originals.len() as u64).to_le_bytes());
                    for original in &map.originals {
                        bytes(&mut h, original.origin.as_bytes());
                        if let Some(t) = &original.text {
                            h.update([1]);
                            text(&mut h, t)
                        } else {
                            h.update([0])
                        }
                    }
                    for preceding in &map.preceding_maps {
                        bytes(&mut h, preceding)
                    }
                } else {
                    h.update([0])
                }
            }
            ModuleInput::Image(image) => {
                h.update([1]);
                bytes(
                    &mut h,
                    &serde_json::to_vec(image.profile())
                        .expect("image profile contains only JSON scalar fields"),
                );
                bytes(&mut h, image.payload());
            }
        }
        for edge in self.imports.iter() {
            text(&mut h, edge.specifier());
            text(&mut h, edge.target().text());
        }
        h.finalize().into()
    }
}
// Programs cross threads: a worker VM executes a part of its parent's graph.
const _: () = {
    const fn send_sync<T: Send + Sync>() {}
    send_sync::<Program>();
};
/// One admitted logical execution plan. It contains no provider, native context or live engine value.
#[derive(Clone, Debug)]
pub struct Program {
    entry: ModuleKey,
    modules: Vec<ProgramModule>,
    builtins: Vec<ModuleKey>,
    limits: ModuleLimits,
}
impl Program {
    /// Validates and freezes a complete graph. This does not execute or instantiate code.
    /// Binary frontends must authenticate native payloads before constructing their code inputs.
    pub fn new(
        entry: ModuleKey,
        mut modules: Vec<ProgramModule>,
        mut builtins: Vec<ModuleKey>,
        limits: ModuleLimits,
    ) -> Result<Self, ProgramError> {
        limits.validate()?;
        if modules
            .len()
            .checked_add(builtins.len())
            .is_none_or(|n| n > limits.nodes)
        {
            return Err(ProgramError::Limit);
        }
        let mut keys = HashSet::new();
        for module in &modules {
            if !keys.insert(module.key.clone()) {
                return Err(ProgramError::InvalidGraph);
            }
        }
        for key in &builtins {
            if !keys.insert(key.clone()) {
                return Err(ProgramError::InvalidGraph);
            }
        }
        if !keys.contains(&entry) {
            return Err(ProgramError::InvalidGraph);
        }
        let mut edges = 0usize;
        let mut source_bytes = 0usize;
        for module in &modules {
            if let ModuleInput::Source(source) = module.code() {
                source_bytes = source
                    .text
                    .as_units()
                    .len()
                    .checked_mul(2)
                    .and_then(|n| source_bytes.checked_add(n))
                    .ok_or(ProgramError::Limit)?;
            }
            let mut specifiers = HashSet::new();
            for edge in module.imports.iter() {
                if edge.specifier.as_units().len() > limits.specifier_units {
                    return Err(ProgramError::Limit);
                }
                if !specifiers.insert(edge.specifier.clone()) || !keys.contains(&edge.target) {
                    return Err(ProgramError::InvalidGraph);
                }
                edges = edges.checked_add(1).ok_or(ProgramError::Limit)?;
            }
            if let ModuleInput::Image(image) = module.code()
                && !same_imports(image.imports(), &specifiers)
            {
                return Err(ProgramError::InvalidGraph);
            }
        }
        if edges > limits.edges || source_bytes > limits.source_bytes {
            return Err(ProgramError::Limit);
        }
        modules.sort_by(|a, b| a.key.text().as_units().cmp(b.key.text().as_units()));
        builtins.sort_by(|a, b| a.text().as_units().cmp(b.text().as_units()));
        Ok(Self {
            entry,
            modules,
            builtins,
            limits,
        })
    }
    /// The part of this graph reachable from `entry` through static imports, as its own
    /// Program. Nothing is loaded or analyzed; `entry` must be a module of this graph.
    pub fn with_entry(&self, entry: &ModuleKey) -> Result<Program, ProgramError> {
        let index: HashMap<&ModuleKey, &ProgramModule> =
            self.modules.iter().map(|m| (&m.key, m)).collect();
        let mut seen = HashSet::new();
        let mut pending = vec![entry];
        let mut modules = Vec::new();
        let mut builtins = Vec::new();
        while let Some(key) = pending.pop() {
            if !seen.insert(key) {
                continue;
            }
            if let Some(module) = index.get(key) {
                pending.extend(module.imports.iter().map(|e| &e.target));
                modules.push((*module).clone());
            } else if self.builtins.contains(key) {
                builtins.push(key.clone());
            } else {
                return Err(ProgramError::InvalidGraph);
            }
        }
        Program::new(entry.clone(), modules, builtins, self.limits)
    }
    /// Borrows the resolved entry identity.
    pub fn entry(&self) -> &ModuleKey {
        &self.entry
    }
    /// Borrows admitted code snapshots in canonical order.
    pub fn modules(&self) -> &[ProgramModule] {
        &self.modules
    }
    /// Borrows required host-module identities; host implementations are bound separately.
    pub fn builtins(&self) -> &[ModuleKey] {
        &self.builtins
    }
    /// Returns the graph budgets enforced during admission and execution.
    pub fn limits(&self) -> ModuleLimits {
        self.limits
    }
    /// Exports trusted compile-only images for packaging, independent of compiler context lifetime.
    pub fn compiled_graph(&self) -> Result<CompiledModuleGraph, ProgramError> {
        let modules = self
            .modules
            .iter()
            .map(|m| {
                Ok(CompiledModule {
                    key: m.key.clone(),
                    imports: m
                        .imports
                        .iter()
                        .map(|e| (e.specifier.clone(), e.target.clone()))
                        .collect(),
                    image: m.compiled.clone().ok_or(ProgramError::Unsupported)?,
                })
            })
            .collect::<Result<Vec<_>, ProgramError>>()?;
        Ok(CompiledModuleGraph {
            entry: self.entry.clone(),
            modules,
            builtins: self.builtins.clone(),
        })
    }
}
/// Native requests equal the admitted specifier set exactly, with no duplicate request.
pub(crate) fn same_imports(got: &[JsString], expected: &HashSet<JsString>) -> bool {
    got.len() == expected.len()
        && got.iter().collect::<HashSet<_>>().len() == got.len()
        && got.iter().all(|s| expected.contains(s))
}
#[cfg(test)]
#[path = "../tests/unit/program.rs"]
mod tests;
