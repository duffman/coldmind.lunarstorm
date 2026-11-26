//! General script emission. Every operation preserves JavaScript evaluation order.
use crate::budget::{Budget, Counter};
use qjs_bytecode_image_verifier::{
    image::{self, Capture, Constant, FunctionImage, VarDef},
    snapshot::Position,
};
use qjs_scalar_ssa_ir::js as ir;
use std::collections::{BTreeMap, VecDeque};
#[path = "script_emitter/class_emission.rs"]
mod class_emit;
use class_emit::PrivateStorage;
#[path = "script_emitter/call_emission.rs"]
mod call_emit;
#[path = "script_emitter/class_reference_assignment.rs"]
mod class_references;
#[path = "script_emitter/emitter_bindings_and_jumps.rs"]
mod emitter_bindings_and_jumps;
#[path = "script_emitter/function_body_compilation.rs"]
mod function_body_compilation;
#[path = "script_emitter/blocks_loops_and_patterns.rs"]
mod blocks_loops_and_patterns;
#[path = "script_emitter/statement_emission.rs"]
mod statement_emission;
#[path = "script_emitter/references_and_properties.rs"]
mod references_and_properties;
#[path = "script_emitter/expression_emission.rs"]
mod expression_emission;
#[derive(Clone)]
enum Place {
    Global,
    Local(u32),
    Argument(u32),
    Capture(u32),
}
#[derive(Clone)]
struct Variable {
    binding: ir::Binding,
    place: Place,
    native_const: bool,
    native_lexical: bool,
    native_kind: u8,
}
struct Instruction {
    name: &'static str,
    args: Vec<u8>,
    target: Option<u32>,
    pop: usize,
    push: usize,
    /// (pop, push) applied on the jump edge when it differs from the fall-through effect.
    taken: Option<(usize, usize)>,
    terminal: bool,
    span: ir::Span,
    /// lunarstorm patch: `true` for a `catch` that installs a user catch
    /// clause under debug markers; its target goes into
    /// `FunctionImage::user_catches` (replaces jetbunny's `debug_catch`).
    user_catch: bool,
}
enum Node {
    Label(u32),
    Instruction(Instruction),
}
/// Optional source-level debug markers. Disabled emission preserves the default image.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JsEmitOptions {
    pub debug_markers: bool,
}
#[derive(Clone, Copy)]
struct JumpTarget {
    label: u32,
    blocks: usize,
    handlers: usize,
    unwind: usize,
}
#[derive(Clone, Copy)]
struct Control<'a> {
    name: Option<&'a str>,
    break_target: JumpTarget,
    continue_target: Option<JumpTarget>,
    unlabeled: bool,
}
#[derive(Clone, Copy)]
enum UnwindKind<'a> {
    Finally(&'a [ir::Statement]),
    ForOf,
    ForAwait,
    ForIn,
}
#[derive(Clone, Copy)]
struct Unwind<'a> {
    kind: UnwindKind<'a>,
    blocks: usize,
    handlers: usize,
    controls: usize,
}
#[derive(Clone, Copy)]
enum PreparedReference {
    Property(u32, u32),
    Super(u32, u32, u32),
    Private(u32, u32),
    /// An identifier a with object can shadow: the found object, or undefined, in a temporary.
    With(u32),
}
#[derive(Clone, Copy)]
enum Completion {
    Return(Option<u32>),
    Branch(JumpTarget),
}
struct Emitter<'a> {
    program: &'a ir::Program,
    budget: Budget,
    function: FunctionImage,
    scope: u32,
    env: BTreeMap<u32, Variable>,
    local_scopes: BTreeMap<u32, u32>,
    class_sources: BTreeMap<u32, Variable>,
    auxiliary_declarations: BTreeMap<(u32, Vec<u16>, u8), (u64, Variable)>,
    auxiliary: BTreeMap<u64, Variable>,
    next_auxiliary: std::rc::Rc<std::cell::Cell<u64>>,
    private_names: BTreeMap<u32, PrivateStorage>,
    nodes: Vec<Node>,
    labels: u32,
    self_binding: Option<ir::Binding>,
    arguments_binding: Option<ir::Binding>,
    source_function: Option<&'a ir::Function>,
    parameter_profile: bool,
    parameter_slots: Vec<u32>,
    parameter_anchor: Option<u32>,
    initializing_parameters: bool,
    /// The one local every `o[k] = v` in this function keeps its result in; see
    /// the `AssignMember` arm of `expression`.
    assign_member_slot: Option<u32>,
    /// Temporaries `clear_temporaries` has ended, free for `temporary` to reuse.
    free_temporaries: Vec<u32>,
    parameter_region_locals: std::collections::BTreeSet<u32>,
    pending_body_anchor: Option<u32>,
    new_target_binding: Option<ir::Binding>,
    uses_new_target: bool,
    is_arrow: bool,
    is_class_constructor: bool,
    derived_constructor: bool,
    home_object_binding: Option<ir::Binding>,
    uses_home_object: bool,
    super_constructor_binding: Option<ir::Binding>,
    field_initializer_binding: Option<ir::Binding>,
    class_strict: bool,
    this_binding: Option<ir::Binding>,
    uses_this: bool,
    controls: Vec<Control<'a>>,
    unwind: Vec<Unwind<'a>>,
    handler_depth: usize,
    blocks: Vec<Vec<u32>>,
    root_locals: bool,
    eval_root: bool,
    /// Sloppy eval code whose variable environment is the global object.
    eval_global_var: bool,
    /// Sloppy eval code whose `var`/`function` go to the caller's `<var>` object (auxiliary id).
    eval_var_object: Option<u64>,
    skip_hoisted: bool,
    options: JsEmitOptions,
    markers: u32,
    source_map: std::sync::Arc<qjs_js_parser_frontend::source::SourceLineMap>,
    function_span: ir::Span,
    child_spans: Vec<image::FunctionSpans>,
    /// Suspension kind of the function being emitted.
    kind: ir::FunctionKind,
    /// Operand stack depth at the current emission point; reset at labels.
    depth: usize,
    /// Operand stack depth recorded by the first jump to each label.
    label_depths: BTreeMap<u32, usize>,
    /// Enclosing `with` statements and sloppy-eval var objects: (auxiliary id, scope,
    /// flag, rank). A ranked object is an eval caller's: it applies to captured names
    /// ranked after it in `external_ranks`.
    withs: Vec<(u64, u32, u8, Option<usize>)>,
    /// Eval external symbol -> its rank in the caller's resolution order.
    external_ranks: BTreeMap<u32, usize>,
    /// Stack depth at entry to the innermost statement being emitted.
    statement_base: usize,
    /// Base depths of for-of records opened inside the current expression.
    expr_records: Vec<usize>,
    /// Whether this body emitted `await` (module top-level await).
    awaits: bool,
    /// Module top-level `using` values: first slot of each value/method pair, and `await`.
    usings: Vec<(u32, bool)>,
    /// Handler that disposes the `using` values when the module body throws.
    using_catch: Option<u32>,
}
fn opcode(name: &str) -> Result<&'static qjs_bytecode_image_verifier::Opcode, String> {
    qjs_bytecode_image_verifier::production::FINAL_OPCODES
        .iter()
        .find(|o| o.name == name)
        .ok_or_else(|| format!("unknown production opcode {name}"))
}
/// Compile an ordinary script; supported syntax is checked before emission.
pub fn compile_script(source: &str, filename: &str) -> Result<image::VerifiedImage, String> {
    compile_script_with_options(source, filename, &JsEmitOptions::default())
}
/// Compile a script with opt-in source-level debug markers.
pub fn compile_script_with_options(
    source: &str,
    filename: &str,
    options: &JsEmitOptions,
) -> Result<image::VerifiedImage, String> {
    let program = qjs_js_parser_frontend::compile_program(source, filename, qjs_js_parser_frontend::SourceMode::Script)
        .map_err(|e| format!("{e:?}"))?;
    emit_program_with_options(&program, options)
}
/// Compile lossless UTF-16 Script input with the requested debug-marker policy.
pub fn compile_script_input_with_options(
    input: &qjs_js_parser_frontend::source::SourceInput,
    filename: &str,
    options: &JsEmitOptions,
) -> Result<image::VerifiedImage, String> {
    let program =
        qjs_js_parser_frontend::compile_program_input(input, filename).map_err(|e| format!("{e:?}"))?;
    emit_program_with_options(&program, options)
}
/// Emit an owned general JavaScript program into a verified native image.
pub fn emit_program(program: &ir::Program) -> Result<image::VerifiedImage, String> {
    emit_program_with_options(program, &JsEmitOptions::default())
}
/// Emit an owned script or module using the requested debug-marker policy.
pub fn emit_program_with_options(
    program: &ir::Program,
    options: &JsEmitOptions,
) -> Result<image::VerifiedImage, String> {
    emit_program_with_limits(program, options, &crate::EmissionLimits::default())
}
/// Emit with caller-tightened aggregate compiler admission limits.
pub fn emit_program_with_limits(
    program: &ir::Program,
    options: &JsEmitOptions,
    limits: &crate::EmissionLimits,
) -> Result<image::VerifiedImage, String> {
    emit_program_with_policies(program, options, limits, &image::ImageLimits::default())
}
/// Apply emitter and independent image policies in their respective stages.
pub fn emit_program_with_policies(
    program: &ir::Program,
    options: &JsEmitOptions,
    limits: &crate::EmissionLimits,
    image_limits: &image::ImageLimits,
) -> Result<image::VerifiedImage, String> {
    image_limits.validate()?;
    crate::budget::program(program, limits)?;
    let budget = Budget::new(*limits)?;
    budget.charge(Counter::Work, program.source.len())?;
    let map = std::sync::Arc::new(qjs_js_parser_frontend::source::SourceLineMap::from_utf16(
        &program.source,
    ));
    let mut emitter = Emitter::new(program, None, BTreeMap::new(), map, budget)?;
    emitter.options = *options;
    if let Some(module) = &program.module {
        return emitter.compile_module(module, image_limits);
    }
    let (function, functions) = emitter.compile(&program.body)?;
    image::verify_and_encode_with_limits(
        &function,
        Some(span_metadata(program, functions)),
        image_limits,
    )
}
fn plain_binding(pattern: &ir::Pattern) -> Option<&ir::Binding> {
    if let ir::Pattern::Binding {
        binding,
        initialization_target: None,
    } = pattern
    {
        Some(binding)
    } else {
        None
    }
}
fn parameter_profile(function: &ir::Function) -> bool {
    function.has_parameter_expressions
        || function
            .parameters
            .iter()
            .any(|pattern| plain_binding(pattern).is_none())
        || function
            .rest_parameter
            .as_ref()
            .is_some_and(|pattern| plain_binding(pattern).is_none())
}

/// Compile an ordinary module using the module grammar and two-phase entry ABI.
pub fn compile_module(source: &str, filename: &str) -> Result<image::VerifiedImage, String> {
    let program = qjs_js_parser_frontend::compile_program(source, filename, qjs_js_parser_frontend::SourceMode::Module)
        .map_err(|e| format!("{e:?}"))?;
    emit_program(&program)
}

/// Raw selected-frame emission; the caller must obtain the separate eval proof.
// Kept as the default-policy internal entry; worker eval uses the bounded variant.
#[allow(dead_code)]
pub(crate) fn emit_eval_function_with_spans(
    program: &ir::Program,
    external: &[(ir::Binding, Capture)],
) -> Result<(FunctionImage, Vec<image::FunctionSpans>), String> {
    emit_eval_function_with_spans_and_limits(
        program,
        external,
        &[],
        false,
        &[],
        None,
        BTreeMap::new(),
        &crate::EmissionLimits::default(),
    )
}
/// A caller-frame private name visible to eval code; its slice index is the
/// private id the frontend assigned. Each role is a capture of the frame's
/// `#name` (or `#name<set>`) binding.
#[derive(Clone, Debug, Default)]
pub(crate) struct EvalPrivate {
    pub(crate) name: Vec<u16>,
    pub(crate) field: Option<Capture>,
    pub(crate) method: Option<Capture>,
    pub(crate) getter: Option<Capture>,
    pub(crate) setter: Option<Capture>,
}
#[allow(
    clippy::too_many_arguments,
    reason = "the eval entry carries the caller's whole scope description"
)]
pub(crate) fn emit_eval_function_with_spans_and_limits(
    program: &ir::Program,
    external: &[(ir::Binding, Capture)],
    private: &[EvalPrivate],
    global_var: bool,
    objects: &[(Capture, u8, usize)],
    var_object: Option<usize>,
    ranks: BTreeMap<u32, usize>,
    limits: &crate::EmissionLimits,
) -> Result<(FunctionImage, Vec<image::FunctionSpans>), String> {
    crate::budget::program(program, limits)?;
    let budget = Budget::new(*limits)?;
    budget.charge(Counter::Work, program.source.len() + external.len())?;
    if program.module.is_some() {
        return Err("eval requires Script source".into());
    }
    let map = std::sync::Arc::new(qjs_js_parser_frontend::source::SourceLineMap::from_utf16(
        &program.source,
    ));
    let mut emitter = Emitter::new(program, None, BTreeMap::new(), map, budget)?;
    emitter.root_locals = true;
    emitter.eval_root = true;
    emitter.eval_global_var = global_var && !program.strict;
    if external.len() > 65535 {
        return Err("capacity: external captures budget exceeded".into());
    }
    for (binding, capture) in external {
        emitter.budget.string(&binding.name)?;
        emitter.budget.text(capture.name.len())?;
        if !matches!(capture.closure_type, 0..=2)
            || capture.name != binding.name.encode_utf16().collect::<Vec<_>>()
        {
            return Err("invalid external eval binding".into());
        }
        let slot = emitter.function.captures.len() as u32;
        if emitter
            .env
            .insert(
                binding.symbol,
                Variable {
                    binding: binding.clone(),
                    place: Place::Capture(slot),
                    native_const: capture.is_const,
                    native_lexical: capture.is_lexical,
                    native_kind: capture.var_kind,
                },
            )
            .is_some()
        {
            return Err("duplicate external eval symbol".into());
        }
        emitter.function.captures.push(capture.clone());
    }
    emitter.external_ranks = ranks;
    // Outermost first, so `with_tests` (innermost first) walks the caller's chain in order.
    for (index, (capture, flag, rank)) in objects.iter().enumerate().rev() {
        if emitter.function.captures.len() >= 65535 {
            return Err("capacity: external captures budget exceeded".into());
        }
        let slot = emitter.function.captures.len() as u32;
        let aux = emitter.next_auxiliary.get();
        emitter.next_auxiliary.set(
            aux.checked_add(1)
                .ok_or("capacity: auxiliary identifier overflow")?,
        );
        emitter.auxiliary.insert(
            aux,
            Variable {
                binding: ir::Binding {
                    symbol: u32::MAX,
                    scope: program.scope,
                    name: String::from_utf16_lossy(&capture.name),
                    kind: ir::DeclarationKind::Const,
                    span: ir::Span { start: 0, end: 0 },
                },
                place: Place::Capture(slot),
                native_const: capture.is_const,
                native_lexical: capture.is_lexical,
                native_kind: capture.var_kind,
            },
        );
        emitter.function.captures.push(capture.clone());
        emitter.withs.push((aux, program.scope, *flag, Some(*rank)));
        if var_object == Some(index) {
            emitter.eval_var_object = Some(aux);
        }
    }
    for (id, outer) in private.iter().enumerate() {
        let mut role = |capture: &Option<Capture>| -> Result<Option<u64>, String> {
            let Some(capture) = capture else {
                return Ok(None);
            };
            emitter.budget.text(capture.name.len())?;
            if emitter.function.captures.len() >= 65535 {
                return Err("capacity: external captures budget exceeded".into());
            }
            let slot = emitter.function.captures.len() as u32;
            let aux = emitter.next_auxiliary.get();
            emitter.next_auxiliary.set(aux + 1);
            emitter.auxiliary.insert(
                aux,
                Variable {
                    binding: ir::Binding {
                        symbol: u32::MAX,
                        scope: program.scope,
                        name: String::from_utf16_lossy(&capture.name),
                        kind: ir::DeclarationKind::Const,
                        span: ir::Span { start: 0, end: 0 },
                    },
                    place: Place::Capture(slot),
                    native_const: capture.is_const,
                    native_lexical: capture.is_lexical,
                    native_kind: capture.var_kind,
                },
            );
            emitter.function.captures.push(capture.clone());
            Ok(Some(aux))
        };
        let storage = PrivateStorage {
            name: outer.name.clone(),
            field: role(&outer.field)?,
            method: role(&outer.method)?,
            getter: role(&outer.getter)?,
            setter: role(&outer.setter)?,
        };
        emitter.private_names.insert(id as u32, storage);
    }
    emitter.compile(&program.body)
}

impl Emitter<'_> {
    fn compile_module(
        mut self,
        module: &ir::Module,
        image_limits: &image::ImageLimits,
    ) -> Result<image::VerifiedImage, String> {
        self.budget.charge(Counter::Work, 1)?;
        self.budget.charge(
            Counter::Work,
            module
                .imports
                .len()
                .saturating_add(module.exports.len())
                .saturating_add(module.requests.len()),
        )?;
        for request in &module.requests {
            self.budget.text(request.len())?;
        }
        for (k, v) in module.request_attributes.iter().flatten() {
            self.budget.text(k.len() + v.len())?;
        }
        for import in &module.imports {
            self.budget.string(&import.binding.name)?;
            self.budget.text(import.name.len())?;
            self.env.insert(
                import.binding.symbol,
                Variable {
                    binding: import.binding.clone(),
                    place: Place::Global,
                    native_const: true,
                    native_lexical: true,
                    native_kind: 0,
                },
            );
        }
        if (self.options.debug_markers
            || self.uses_this
            || self.is_class_constructor
            || self.uses_home_object)
            && let Some(binding) = self.this_binding.clone()
        {
            // Native frame locals begin as undefined. Keeping this private
            // module receiver cell preserves the module phase-entry protocol.
            self.declare_binding(&binding, true)?;
        }
        self.collect(&self.program.body)?;
        let mut declaration = 0;
        for variable in self.env.values_mut() {
            self.budget.charge(Counter::Work, module.imports.len())?;
            if !matches!(variable.place, Place::Global)
                || variable.binding.scope != self.program.scope
            {
                continue;
            }
            let imported = module
                .imports
                .iter()
                .enumerate()
                .find(|(_, i)| i.binding.symbol == variable.binding.symbol);
            let (kind, index) = if let Some((at, import)) = imported {
                (if import.name == [b'*' as u16] { 6 } else { 7 }, at as u32)
            } else {
                let index = declaration;
                declaration += 1;
                (6, index)
            };
            if self.function.captures.len() >= 65535 {
                return Err("capacity: emission captures budget exceeded".into());
            }
            self.budget.string(&variable.binding.name)?;
            let slot = self.function.captures.len() as u32;
            self.function
                .captures
                .push(Self::capture(&variable.binding, index, kind));
            variable.place = Place::Capture(slot);
        }
        let span = ir::Span {
            start: self.function_span.start,
            end: self.function_span.start,
        };
        self.function.strict = true;
        self.function.flags.kind = 2;
        self.simple("push_this", 0, 1, span);
        let evaluation = self.label();
        self.jump("if_false", evaluation, span);
        for statement in &self.program.body {
            if let ir::StatementKind::Function { binding, function } = &statement.kind {
                self.function_value(function, binding.span)?;
                self.store(
                    &ir::Reference {
                        symbol: Some(binding.symbol),
                        name: binding.name.clone(),
                    },
                    true,
                    binding.span,
                )?;
            }
        }
        self.simple("return_undef", 0, 0, span);
        self.mark(evaluation);
        self.simple("undefined", 0, 1, span);
        self.slot("put_loc", 0, 1, 0, span);
        self.skip_hoisted = true;
        self.block(&self.program.body)?;
        self.dispose_module_usings(span)?;
        self.simple("undefined", 0, 1, span);
        self.simple("return_async", 1, 0, span);
        let capture_for = |symbol: u32| -> Result<u32, String> {
            match self.env.get(&symbol).map(|v| &v.place) {
                Some(Place::Capture(n)) => Ok(*n),
                _ => Err("module binding has no capture slot".into()),
            }
        };
        let imports = module
            .imports
            .iter()
            .map(|i| {
                Ok(image::ModuleImport {
                    capture: capture_for(i.binding.symbol)?,
                    name: i.name.clone(),
                    module: i.request,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut exports = Vec::new();
        let mut stars = Vec::new();
        for export in &module.exports {
            match export {
                ir::Export::Local { name, .. } => self.budget.text(name.len())?,
                ir::Export::Indirect { import, name, .. } => {
                    self.budget.text(import.len())?;
                    self.budget.text(name.len())?;
                }
                ir::Export::Star { .. } => self.budget.charge(Counter::Work, stars.len())?,
            }
            match export {
                ir::Export::Local { symbol, name } => exports.push(image::ModuleExport::Local {
                    capture: capture_for(*symbol)?,
                    name: name.clone(),
                }),
                ir::Export::Indirect {
                    request,
                    import,
                    name,
                } => exports.push(image::ModuleExport::Indirect {
                    module: *request,
                    import: import.clone(),
                    name: name.clone(),
                }),
                ir::Export::Star { request } => {
                    if !stars.contains(request) {
                        stars.push(*request);
                    }
                }
            }
        }
        let filename = self.program.filename.encode_utf16().collect();
        let program = self.program;
        let awaits = self.awaits;
        let (function, functions) = self.finish()?;
        let image = image::ModuleImage {
            name: filename,
            required_modules: module.requests.clone(),
            // lunarstorm patch: import attributes reach the engine's loader;
            // an all-empty table is written as no table.
            required_attributes: if module.request_attributes.iter().all(Vec::is_empty) {
                Vec::new()
            } else {
                module.request_attributes.clone()
            },
            exports,
            star_exports: stars,
            imports,
            has_top_level_await: awaits,
            function,
        };
        image::verify_and_encode_module_with_limits(
            &image,
            Some(span_metadata(program, functions)),
            image_limits,
        )
    }
}

fn span_metadata(
    program: &ir::Program,
    functions: Vec<image::FunctionSpans>,
) -> image::SpanMetadata {
    image::SpanMetadata {
        source_unit: image::SourceUnit::Utf16,
        source_len: program.source.len() as u32,
        source_digest: image::source_digest_utf16(&program.source),
        functions,
    }
}
