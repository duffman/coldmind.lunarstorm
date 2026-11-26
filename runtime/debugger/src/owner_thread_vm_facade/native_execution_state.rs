//! Native execution state only. Package resolution and admission belong to the platform.
use super::*;
use crate::{modules::*, program::Program};
use std::collections::{HashMap, HashSet};
#[derive(Default)]
pub(super) struct ExecutionModules {
    nodes: HashMap<ModuleKey, LoadedModule>,
    builtins: HashMap<ModuleKey, BuiltinModule>,
    source_bytes: usize,
    edges: usize,
    /// Rejected native records replaced by different content; they stay in the context.
    retired: usize,
}
struct LoadedModule {
    id: ModuleId,
    fingerprint: [u8; 32],
    imports: Vec<JsString>,
    /// Prepared natively but rejected against the Program; never bound or reused.
    rejected: bool,
    /// Bindings installed; they never change because the fingerprint covers edge targets.
    bound: bool,
}
fn invalid() -> FailureReport {
    FailureReport::fault(
        FailureCode::InvalidArtifact,
        "inconsistent Program or replacement of an existing module identity",
    )
}
impl<E: JsEngine> VirtualMachine<E> {
    fn module_command<T: 'static>(
        &mut self,
        mut body: impl FnMut(&dyn JsScope) -> Result<T, FailureReport> + 'static,
    ) -> Result<T, VmError> {
        let result = Rc::new(RefCell::new(None));
        let output = result.clone();
        self.execute(Box::new(move |scope: &dyn JsScope| {
            let value = body(scope);
            if let Err(report) = &value
                && matches!(report.class, FailureClass::Engine | FailureClass::Host)
            {
                return Err(report.clone());
            }
            *output.borrow_mut() = Some(value);
            Ok(())
        }))?;
        result
            .borrow_mut()
            .take()
            .ok_or(VmError::InvalidState)?
            .map_err(VmError::Execution)
    }
    /// Extracts checked requests for the platform. This method performs no resolution or I/O.
    pub fn take_import_requests(&mut self) -> Result<ModuleImportBatch, VmError> {
        if !self.features.dynamic_import {
            self.ready()?;
            return Ok(ModuleImportBatch::default());
        }
        self.module_command(|s| s.modules().ok_or_else(unsupported)?.take_dynamic_imports())
    }
    /// Completes one checked import through the ordinary command/checkpoint path.
    pub fn complete_import(
        &mut self,
        id: ModuleImportId,
        result: Result<ModuleId, FailureReport>,
    ) -> Result<(), VmError> {
        self.module_command(move |s| {
            s.modules()
                .ok_or_else(unsupported)?
                .finish_dynamic_import(id, result.clone())
        })
    }
    /// Returns actual adapter image compatibility metadata without loading any package.
    pub fn module_image_profile(&mut self) -> Result<Option<ImageProfile>, VmError> {
        self.module_command(|s| Ok(s.modules().and_then(|m| m.images()).map(|i| i.profile())))
    }
    /// Instantiates and evaluates an admitted immutable graph. No provider is retained or invoked.
    /// Native handles and once-only module instances belong to this VM context. Returned promises
    /// are rooted but never implicitly awaited. Host implementations are supplied separately.
    pub fn execute_program(
        &mut self,
        program: &Program,
        host: &[BuiltinModule],
    ) -> Result<ModuleRun, VmError> {
        self.ready()?;
        if !self.features.modules {
            return Err(VmError::Unsupported);
        }
        let mut state = std::mem::take(&mut self.modules);
        let result = (|| {
            let mut host_keys = HashSet::new();
            if host.iter().any(|b| !host_keys.insert(b.key.clone())) {
                return Err(VmError::Execution(invalid()));
            }
            let mut source_bytes = state.source_bytes;
            let mut edges = state.edges;
            let mut nodes = state.nodes.len() + state.retired;
            for m in program.modules() {
                // Accepted records are reused only unchanged; rejected ones only by different content.
                match state.nodes.get(m.key()) {
                    None => {}
                    Some(n) if n.rejected && n.fingerprint != m.fingerprint() => {}
                    Some(n)
                        if !n.rejected
                            && n.fingerprint == m.fingerprint()
                            && !state.builtins.contains_key(m.key()) =>
                    {
                        continue;
                    }
                    Some(_) => return Err(VmError::Execution(invalid())),
                }
                nodes = nodes.checked_add(1).ok_or(VmError::Execution(invalid()))?;
                edges = edges
                    .checked_add(m.imports().len())
                    .ok_or(VmError::Execution(invalid()))?;
                if let ModuleInput::Source(source) = m.code() {
                    self.validate_source(source).map_err(VmError::Execution)?;
                    source_bytes = source
                        .text
                        .as_units()
                        .len()
                        .checked_mul(2)
                        .and_then(|n| source_bytes.checked_add(n))
                        .ok_or(VmError::Execution(invalid()))?;
                }
            }
            for key in program.builtins() {
                let binding = host
                    .iter()
                    .find(|b| &b.key == key)
                    .ok_or(VmError::Execution(invalid()))?;
                if let Some(old) = state.builtins.get(key) {
                    if old.exports.len() != binding.exports.len()
                        || old
                            .exports
                            .iter()
                            .zip(&binding.exports)
                            .any(|(a, b)| a.name != b.name || !Rc::ptr_eq(&a.value, &b.value))
                    {
                        return Err(VmError::Execution(invalid()));
                    }
                } else {
                    if state.nodes.contains_key(key) {
                        return Err(VmError::Execution(invalid()));
                    }
                    nodes = nodes.checked_add(1).ok_or(VmError::Execution(invalid()))?;
                }
            }
            let requested = program.limits();
            let configured = self.module_limits;
            let limits = ModuleLimits {
                nodes: requested.nodes.min(configured.nodes),
                edges: requested.edges.min(configured.edges),
                source_bytes: requested.source_bytes.min(configured.source_bytes),
                specifier_units: requested.specifier_units.min(configured.specifier_units),
            };
            if program
                .modules()
                .iter()
                .flat_map(|m| m.imports())
                .any(|e| e.specifier().as_units().len() > limits.specifier_units)
            {
                return Err(VmError::Execution(invalid()));
            }
            if nodes > limits.nodes || edges > limits.edges || source_bytes > limits.source_bytes {
                return Err(VmError::Execution(FailureReport::fault(
                    FailureCode::Capacity,
                    "VM Program session budget exceeded",
                )));
            }
            for key in program.builtins() {
                if state.builtins.contains_key(key) {
                    continue;
                }
                let b = host
                    .iter()
                    .find(|b| &b.key == key)
                    .expect("checked host binding")
                    .clone();
                let identity = key.text().clone();
                let exports = b.exports.clone();
                let id = self.module_command(move |s| {
                    s.modules()
                        .ok_or_else(unsupported)?
                        .define_builtin(&identity, &exports)
                })?;
                state.nodes.insert(
                    key.clone(),
                    LoadedModule {
                        id,
                        fingerprint: [0; 32],
                        imports: vec![],
                        rejected: false,
                        bound: true,
                    },
                );
                state.builtins.insert(key.clone(), b);
            }
            for m in program.modules() {
                let replaces = match state.nodes.get(m.key()) {
                    Some(n) if n.rejected => true,
                    Some(_) => continue,
                    None => false,
                };
                let input = m.clone();
                let identity = m.key().text().clone();
                let allow_source_cache = !self.development;
                let expected = m
                    .imports()
                    .iter()
                    .map(|e| e.specifier().clone())
                    .collect::<HashSet<_>>();
                // The native record exists once prepare succeeds, so it is recorded and charged
                // even when its requests disagree with the Program; retries cannot re-prepare it.
                let (id, imports) = self.module_command(move |s| {
                    let api = s.modules().ok_or_else(unsupported)?;
                    let id = match input.code() {
                        ModuleInput::Source(source) => {
                            let cached = if allow_source_cache && source.source_map.is_none() {
                                api.images().and_then(|images| {
                                    input
                                        .source_cache()
                                        .filter(|cache| {
                                            cache.profile() == &images.profile()
                                                && cache.payload().len()
                                                    <= s.data_limits().max_bytes
                                                && cache.matches_source(&identity, source)
                                        })
                                        .map(|cache| (images, cache))
                                })
                            } else {
                                None
                            };
                            if let Some((images, image)) = cached {
                                images.prepare_source_cache(image, &identity)?
                            } else {
                                api.prepare(source, &identity)?
                            }
                        }
                        ModuleInput::Image(image) => api
                            .images()
                            .ok_or_else(unsupported)?
                            .prepare(image, &identity)?,
                    };
                    match api.imports(&id) {
                        Err(e) if matches!(e.class, FailureClass::Engine | FailureClass::Host) => {
                            Err(e)
                        }
                        imports => Ok((id, imports)),
                    }
                })?;
                state.retired += usize::from(replaces);
                let imports = imports.and_then(|imports| {
                    crate::program::same_imports(&imports, &expected)
                        .then_some(imports)
                        .ok_or_else(invalid)
                });
                if let ModuleInput::Source(source) = m.code() {
                    state.source_bytes += source.text.as_units().len() * 2;
                }
                state.edges += m.imports().len();
                state.nodes.insert(
                    m.key().clone(),
                    LoadedModule {
                        id,
                        imports: imports.clone().unwrap_or_default(),
                        fingerprint: m.fingerprint(),
                        rejected: imports.is_err(),
                        bound: false,
                    },
                );
                imports.map_err(VmError::Execution)?;
            }
            let mut entry_bindings = Vec::new();
            for m in program.modules() {
                let node = &state.nodes[m.key()];
                let id = node.id;
                let targets = m
                    .imports()
                    .iter()
                    .map(|e| (e.specifier(), state.nodes[e.target()].id))
                    .collect::<HashMap<_, _>>();
                let bindings = node
                    .imports
                    .iter()
                    .map(|specifier| ModuleBinding {
                        specifier: specifier.clone(),
                        module: targets[specifier],
                    })
                    .collect::<Vec<_>>();
                if m.key() == program.entry() {
                    entry_bindings = bindings.clone();
                }
                if node.bound {
                    continue;
                }
                self.module_command(move |s| {
                    s.modules().ok_or_else(unsupported)?.bind(&id, &bindings)
                })?;
                if let Some(node) = state.nodes.get_mut(m.key()) {
                    node.bound = true;
                }
            }
            let id = state.nodes[program.entry()].id;
            let bindings = entry_bindings;
            self.module_command(move |s| {
                let m = s.modules().ok_or_else(unsupported)?;
                m.link(&id, &bindings)?;
                let namespace = m.namespace(&id).map_err(|e| s.detach(e))?;
                let namespace = s.retain(namespace)?;
                let evaluation = m.evaluate(&id).map_err(|e| s.detach(e))?;
                let evaluation = s.retain(evaluation)?;
                Ok(ModuleRun {
                    id,
                    namespace,
                    evaluation,
                })
            })
        })();
        self.modules = state;
        result
    }
    /// Observes native module evaluation through the existing command/checkpoint path.
    pub fn module_state(&mut self, id: ModuleId) -> Result<ModuleEvaluationState, VmError> {
        self.module_command(move |s| s.modules().ok_or_else(unsupported)?.evaluation_state(&id))
    }
    /// Reads `id`'s evaluation state and takes queued `import()` requests in one command.
    ///
    /// Every command pumps the turns that became ready before it, each with its checkpoint,
    /// so a turn can settle the entry. One command gives a snapshot that no other command
    /// changes before the caller acts on it.
    pub fn module_snapshot(
        &mut self,
        id: ModuleId,
    ) -> Result<(ModuleEvaluationState, ModuleImportBatch), VmError> {
        let dynamic = self.features.dynamic_import;
        self.module_command(move |s| {
            let m = s.modules().ok_or_else(unsupported)?;
            let state = m.evaluation_state(&id)?;
            let batch = if dynamic {
                m.take_dynamic_imports()?
            } else {
                ModuleImportBatch::default()
            };
            Ok((state, batch))
        })
    }
}
fn unsupported() -> FailureReport {
    FailureReport::fault(
        FailureCode::Unsupported,
        "native module interface unavailable",
    )
}
