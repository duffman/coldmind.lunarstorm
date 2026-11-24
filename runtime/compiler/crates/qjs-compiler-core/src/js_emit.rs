//! General script emission. Every operation preserves JavaScript evaluation order.
use crate::budget::{Budget, Counter};
use qjs_bytecode::{
    image::{self, Capture, Constant, FunctionImage, VarDef},
    snapshot::Position,
};
use qjs_ir::js as ir;
use std::collections::{BTreeMap, VecDeque};
#[path = "class_emit.rs"]
mod class_emit;
use class_emit::PrivateStorage;
#[path = "call_emit.rs"]
mod call_emit;
#[path = "class_references.rs"]
mod class_references;
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
    source_map: std::sync::Arc<qjs_frontend::source::SourceLineMap>,
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
fn opcode(name: &str) -> Result<&'static qjs_bytecode::Opcode, String> {
    qjs_bytecode::production::FINAL_OPCODES
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
    let program = qjs_frontend::compile_program(source, filename, qjs_frontend::SourceMode::Script)
        .map_err(|e| format!("{e:?}"))?;
    emit_program_with_options(&program, options)
}
/// Compile lossless UTF-16 Script input with the requested debug-marker policy.
pub fn compile_script_input_with_options(
    input: &qjs_frontend::source::SourceInput,
    filename: &str,
    options: &JsEmitOptions,
) -> Result<image::VerifiedImage, String> {
    let program =
        qjs_frontend::compile_program_input(input, filename).map_err(|e| format!("{e:?}"))?;
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
    let map = std::sync::Arc::new(qjs_frontend::source::SourceLineMap::from_utf16(
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
impl<'a> Emitter<'a> {
    fn new(
        program: &'a ir::Program,
        function: Option<&'a ir::Function>,
        outer: BTreeMap<u32, Variable>,
        source_map: std::sync::Arc<qjs_frontend::source::SourceLineMap>,
        budget: Budget,
    ) -> Result<Self, String> {
        Self::new_region(program, function, outer, source_map, budget, None)
    }
    fn new_region(
        program: &'a ir::Program,
        function: Option<&'a ir::Function>,
        outer: BTreeMap<u32, Variable>,
        source_map: std::sync::Arc<qjs_frontend::source::SourceLineMap>,
        budget: Budget,
        region: Option<ir::Span>,
    ) -> Result<Self, String> {
        budget.charge(Counter::Functions, 1)?;
        budget.slots(function.map_or(0, |f| f.parameters.len()) + 1)?;
        budget.string(&program.filename)?;
        budget.string(function.map_or("<script>", |f| f.name.as_deref().unwrap_or("")))?;
        let region = region.or_else(|| function.map(|function| function.span));
        if region
            .is_some_and(|span| span.start > span.end || span.end as usize > program.source.len())
        {
            return Err("function source region outside original input".into());
        }
        let source_length = region.map_or(program.source.len(), |span| {
            (span.end - span.start) as usize
        });
        budget.charge(Counter::Work, source_length)?;
        if let Some(f) = function {
            for binding in [
                &f.self_binding,
                &f.arguments_binding,
                &f.this_binding,
                &f.new_target_binding,
                &f.home_object_binding,
                &f.super_constructor_binding,
            ]
            .into_iter()
            .flatten()
            {
                budget.string(&binding.name)?;
            }
        }
        if function.is_none()
            && let Some(binding) = &program.this_binding
        {
            budget.string(&binding.name)?;
        }
        let parameter_profile = function.is_some_and(parameter_profile);
        let scope = function.map_or(program.scope, |f| f.scope);
        let mut native = FunctionImage {
            filename: program.filename.clone(),
            function_name: function
                .map_or_else(|| "<script>".into(), |f| f.name.clone().unwrap_or_default()),
            strict: function.map_or(program.strict, |f| f.strict),
            ..Default::default()
        };
        native.flags.has_prototype = function.is_some_and(|f| {
            !f.is_method
                && !f.is_arrow
                && !f.is_class_constructor
                && f.kind == ir::FunctionKind::Normal
        });
        native.flags.kind = function.map_or(0, |f| f.kind.native());
        native.flags.arguments_allowed = function.is_some_and(|f| f.arguments_allowed);
        native.flags.simple_parameters = function.is_none_or(|f| {
            f.rest_parameter.is_none()
                && f.parameters
                    .iter()
                    .all(|pattern| plain_binding(pattern).is_some())
        });
        native.flags.new_target =
            function.is_some_and(|f| !f.is_arrow || f.new_target_binding.is_some());
        native.flags.derived_constructor = function.is_some_and(|f| f.derived_constructor);
        native.flags.super_call = function.is_some_and(|f| f.super_call_allowed);
        native.flags.super_allowed = function.is_some_and(|f| f.home_object_binding.is_some());
        native.flags.need_home_object = function.is_some_and(|f| !f.is_arrow && f.uses_home_object);
        native.source = Some(if let Some(region) = region {
            program.source[region.start as usize..region.end as usize].to_vec()
        } else {
            program.source.clone()
        });
        let mut env = BTreeMap::new();
        budget.charge(Counter::Work, outer.len())?;
        // Innermost first: the parent's own slots (later slots are nested
        // deeper), then its captures, which are already innermost first. Frame
        // inspection resolves a name to its first capture, so shadowing holds.
        let mut outer: Vec<_> = outer.into_iter().collect();
        outer.sort_by_key(|(_, var)| match var.place {
            Place::Local(i) | Place::Argument(i) => (0, std::cmp::Reverse(i), 0),
            Place::Capture(i) => (1, std::cmp::Reverse(0), i),
            Place::Global => (2, std::cmp::Reverse(0), 0),
        });
        for (symbol, var) in outer {
            // Atom units are charged for the captures kept after pruning.
            budget.charge(Counter::Work, var.binding.name.len() + 1)?;
            if !matches!(var.place, Place::Global) && native.captures.len() >= 65535 {
                return Err("capacity: emission captures budget exceeded".into());
            }
            let place = match var.place {
                Place::Global => Place::Global,
                Place::Local(idx) => {
                    let n = native.captures.len() as u32;
                    native.captures.push(Self::capture_variable(&var, idx, 0));
                    Place::Capture(n)
                }
                Place::Argument(idx) => {
                    let n = native.captures.len() as u32;
                    native.captures.push(Self::capture_variable(&var, idx, 1));
                    Place::Capture(n)
                }
                Place::Capture(idx) => {
                    let n = native.captures.len() as u32;
                    native.captures.push(Self::capture_variable(&var, idx, 2));
                    Place::Capture(n)
                }
            };
            env.insert(symbol, Variable { place, ..var });
        }
        if let Some(f) = function {
            native.arg_count = f.parameters.len() as u32;
            native.defined_arg_count = f.defined_arg_count;
            budget.charge(Counter::Work, f.parameters.len())?;
            for (i, pattern) in f.parameters.iter().enumerate() {
                if parameter_profile {
                    native.locals.push(VarDef::default());
                    continue;
                }
                let binding =
                    plain_binding(pattern).ok_or("simple parameter must be an identifier")?;
                budget.string(&binding.name)?;
                let mut definition = Self::vardef(binding, i as u32, None);
                budget.charge(Counter::Work, f.parameters[i + 1..].len())?;
                let mut shadowed = false;
                for later in &f.parameters[i + 1..] {
                    let later =
                        plain_binding(later).ok_or("simple parameter must be an identifier")?;
                    budget.charge(Counter::Work, later.name.len().min(binding.name.len()))?;
                    shadowed |= later.name == binding.name;
                }
                if shadowed {
                    definition.name.clear();
                }
                native.locals.push(definition);
                env.insert(
                    binding.symbol,
                    Variable {
                        binding: binding.clone(),
                        place: Place::Argument(i as u32),
                        native_const: false,
                        native_lexical: false,
                        native_kind: 0,
                    },
                );
            }
        }
        // Script completion is an ordinary synthetic local, never a source binding.
        native.local_count = 1;
        native.locals.push(VarDef {
            name: Vec::new(),
            ..Default::default()
        });
        native.var_ref_count = native.arg_count;
        Ok(Self {
            program,
            budget,
            function: native,
            scope,
            env,
            local_scopes: BTreeMap::new(),
            class_sources: BTreeMap::new(),
            auxiliary_declarations: BTreeMap::new(),
            auxiliary: BTreeMap::new(),
            next_auxiliary: std::rc::Rc::new(std::cell::Cell::new(0)),
            private_names: BTreeMap::new(),
            nodes: Vec::new(),
            labels: 0,
            self_binding: function.and_then(|f| f.self_binding.clone()),
            arguments_binding: function.and_then(|f| f.arguments_binding.clone()),
            source_function: function,
            parameter_profile,
            parameter_slots: Vec::new(),
            parameter_anchor: None,
            initializing_parameters: false,
            parameter_region_locals: Default::default(),
            pending_body_anchor: None,
            this_binding: function
                .map_or_else(|| program.this_binding.clone(), |f| f.this_binding.clone()),
            uses_this: function.map_or(program.uses_this, |f| f.uses_this),
            new_target_binding: function.and_then(|f| f.new_target_binding.clone()),
            uses_new_target: function.is_some_and(|f| f.uses_new_target),
            is_arrow: function.is_some_and(|f| f.is_arrow),
            is_class_constructor: function.is_some_and(|f| f.is_class_constructor),
            derived_constructor: function.is_some_and(|f| f.derived_constructor),
            home_object_binding: function.and_then(|f| f.home_object_binding.clone()),
            uses_home_object: function.is_some_and(|f| f.uses_home_object),
            super_constructor_binding: function.and_then(|f| f.super_constructor_binding.clone()),
            field_initializer_binding: function.and_then(|f| f.field_initializer_binding.clone()),
            class_strict: false,
            controls: Vec::new(),
            unwind: Vec::new(),
            handler_depth: 0,
            blocks: Vec::new(),
            root_locals: false,
            eval_root: false,
            eval_global_var: false,
            eval_var_object: None,
            skip_hoisted: false,
            options: JsEmitOptions::default(),
            markers: 0,
            source_map,
            function_span: region.unwrap_or(ir::Span {
                start: 0,
                end: program.source.len() as u32,
            }),
            child_spans: Vec::new(),
            kind: function.map_or(ir::FunctionKind::Normal, |f| f.kind),
            depth: 0,
            label_depths: BTreeMap::new(),
            withs: Vec::new(),
            external_ranks: BTreeMap::new(),
            statement_base: 0,
            expr_records: Vec::new(),
            awaits: false,
            usings: Vec::new(),
            using_catch: None,
        })
    }
    /// `using x = v` at module top level. The first one opens a handler that lasts to the end
    /// of the module body; each value and its dispose method wait in two adjacent temporaries,
    /// the pair `using_dispose` reads.
    fn using_declaration(
        &mut self,
        declaration: &'a ir::Declarator,
        is_await: bool,
        span: ir::Span,
    ) -> Result<(), String> {
        let ir::Pattern::Binding { binding, .. } = &declaration.pattern else {
            return Err("using declarations bind identifiers".into());
        };
        if self.program.module.is_none() || binding.scope != self.program.scope {
            return Err("using declarations are supported at module top level only".into());
        }
        let initializer = declaration
            .initializer
            .as_ref()
            .ok_or("using declaration without initializer")?;
        if self.using_catch.is_none() {
            let caught = self.label();
            self.handler(caught, span, false)?;
            self.handler_depth += 1;
            self.using_catch = Some(caught);
        }
        let value = self.temporary()?;
        let method = self.temporary()?;
        if method != value + 1 || value > u32::from(u16::MAX) - 1 {
            return Err("capacity: using slots".into());
        }
        self.expression(initializer)?;
        self.infer_pattern_name(&declaration.pattern, initializer)?;
        self.instruction("using_check", vec![u8::from(is_await)], 1, 2, span);
        self.slot("put_loc", method, 1, 0, span);
        self.simple("dup", 1, 2, span);
        self.slot("put_loc", value, 1, 0, span);
        self.pattern(&declaration.pattern, span)?;
        self.usings.push((value, is_await));
        Ok(())
    }
    /// Ends the module body's `using` region: disposes in reverse order on normal completion
    /// and on a throw. A disposal error becomes the error state, merged into a SuppressedError
    /// with any earlier one, and is thrown once every value has been disposed.
    fn dispose_module_usings(&mut self, span: ir::Span) -> Result<(), String> {
        let Some(caught) = self.using_catch.take() else {
            return Ok(());
        };
        let usings = std::mem::take(&mut self.usings);
        let cleanup = self.scope_slots(self.program.scope)?;
        let end = self.label();
        self.handler_depth -= 1;
        self.simple("drop", 1, 0, span);
        self.simple("using_dispose_init", 0, 1, span);
        self.dispose_usings(&usings, span)?;
        self.simple("using_dispose_end", 1, 0, span);
        self.jump("goto", end, span);
        self.mark(caught);
        // Native unwinding leaves block scopes entered inside the region open.
        for slot in cleanup {
            self.slot("close_loc", slot, 0, 0, span);
        }
        self.caught_error(span)?;
        self.dispose_usings(&usings, span)?;
        self.simple("throw", 1, 0, span);
        self.mark(end);
        Ok(())
    }
    /// A handler entry stores the thrown value in an anonymous temporary before using it.
    fn caught_error(&mut self, span: ir::Span) -> Result<(), String> {
        let error = self.temporary()?;
        self.slot("put_loc", error, 1, 0, span);
        self.slot("get_loc", error, 0, 1, span);
        Ok(())
    }
    fn dispose_usings(&mut self, usings: &[(u32, bool)], span: ir::Span) -> Result<(), String> {
        for &(slot, is_await) in usings.iter().rev() {
            let slot = (slot as u16).to_le_bytes().to_vec();
            if !is_await {
                self.instruction("using_dispose", slot, 1, 1, span);
                continue;
            }
            // [error] -> [error, catch, result] -> awaited; a throw merges into the error.
            let failed = self.label();
            let done = self.label();
            self.handler(failed, span, false)?;
            self.instruction("using_dispose_async", slot, 0, 1, span);
            self.simple("await", 1, 1, span);
            self.simple("drop", 1, 0, span);
            self.simple("drop", 1, 0, span);
            self.jump("goto", done, span);
            self.mark(failed);
            self.caught_error(span)?;
            self.simple("using_dispose_merge", 2, 1, span);
            self.mark(done);
        }
        Ok(())
    }
    fn capture_variable(variable: &Variable, idx: u32, kind: u8) -> Capture {
        let mut capture = Self::capture(&variable.binding, idx, kind);
        capture.is_const = variable.native_const;
        capture.is_lexical = variable.native_lexical;
        capture.var_kind = variable.native_kind;
        capture
    }
    fn capture(b: &ir::Binding, idx: u32, kind: u8) -> Capture {
        Capture {
            name: b.name.encode_utf16().collect(),
            var_idx: idx,
            closure_type: kind,
            is_const: matches!(
                b.kind,
                ir::DeclarationKind::Const | ir::DeclarationKind::Import
            ),
            is_lexical: matches!(
                b.kind,
                ir::DeclarationKind::Let | ir::DeclarationKind::Const | ir::DeclarationKind::Import
            ),
            var_kind: if b.kind == ir::DeclarationKind::Catch {
                3
            } else {
                0
            },
        }
    }
    fn vardef(b: &ir::Binding, var_ref_idx: u32, next: Option<u32>) -> VarDef {
        VarDef {
            name: b.name.encode_utf16().collect(),
            scope_level: if matches!(
                b.kind,
                ir::DeclarationKind::Let
                    | ir::DeclarationKind::Const
                    | ir::DeclarationKind::Import
                    | ir::DeclarationKind::Catch
            ) {
                b.scope + 2
            } else {
                0
            },
            scope_next: next.into(),
            is_const: matches!(
                b.kind,
                ir::DeclarationKind::Const | ir::DeclarationKind::Import
            ),
            is_lexical: matches!(
                b.kind,
                ir::DeclarationKind::Let | ir::DeclarationKind::Const | ir::DeclarationKind::Import
            ),
            is_captured: true,
            var_ref_idx,
            var_kind: if b.kind == ir::DeclarationKind::Catch {
                3
            } else {
                0
            },
        }
    }
    fn atom(&mut self, name: &str) -> u32 {
        if self.budget.charge(Counter::Work, name.len()).is_err() {
            return 0;
        }
        if name.len() > self.budget.limits.max_atom_units.saturating_mul(3) {
            let _ = self.budget.text(name.len());
            return 0;
        }
        self.atom_units(&name.encode_utf16().collect::<Vec<_>>())
    }
    fn atom_units(&mut self, units: &[u16]) -> u32 {
        if self
            .budget
            .charge(
                Counter::Work,
                self.function.atoms.len().saturating_add(units.len()),
            )
            .is_err()
        {
            return 0;
        }
        let comparisons = self
            .function
            .atoms
            .iter()
            .filter(|a| a.len() == units.len())
            .count()
            .checked_mul(units.len());
        let comparisons = match comparisons {
            Some(n) => n,
            None => {
                let _ = self.budget.charge(Counter::Work, usize::MAX);
                return 0;
            }
        };
        if self.budget.charge(Counter::Work, comparisons).is_err() {
            return 0;
        }
        if let Some(n) = self.function.atoms.iter().position(|a| a == units) {
            n as u32
        } else {
            if self
                .budget
                .charge(Counter::Atoms, 1)
                .and_then(|_| self.budget.text(units.len()))
                .is_err()
            {
                return 0;
            }
            let n = self.function.atoms.len() as u32;
            self.function.atoms.push(units.to_vec());
            n
        }
    }
    fn instruction(
        &mut self,
        name: &'static str,
        args: Vec<u8>,
        pop: usize,
        push: usize,
        span: ir::Span,
    ) {
        if self
            .budget
            .charge(Counter::Instructions, 1)
            .and_then(|_| self.budget.charge(Counter::Code, 1 + args.len()))
            .and_then(|_| self.budget.charge(Counter::Positions, 1))
            .is_err()
        {
            return;
        }
        self.depth = self.depth.saturating_sub(pop) + push;
        self.awaits |= name == "await";
        self.nodes.push(Node::Instruction(Instruction {
            name,
            args,
            target: None,
            pop,
            push,
            taken: None,
            terminal: matches!(
                name,
                "return" | "return_undef" | "return_async" | "throw_error" | "throw"
            ),
            span,
            user_catch: false,
        }));
    }
    fn effective_strict(&self) -> bool {
        self.function.strict || self.class_strict
    }
    fn simple(&mut self, name: &'static str, pop: usize, push: usize, span: ir::Span) {
        if self.class_strict && !self.function.strict {
            let selector = match name {
                "put_array_el" => Some(0),
                "delete" => Some(1),
                "put_super_value" => Some(2),
                _ => None,
            };
            if let Some(selector) = selector {
                let mut arguments = self.atom_units(&[]).to_ne_bytes().to_vec();
                arguments.push(selector);
                self.instruction("strict_op", arguments, pop, push, span);
                return;
            }
        }
        self.instruction(name, vec![], pop, push, span)
    }
    fn atom_op(
        &mut self,
        name: &'static str,
        value: &str,
        pop: usize,
        push: usize,
        flags: Option<u8>,
        span: ir::Span,
    ) {
        let mut args = self.atom(value).to_ne_bytes().to_vec();
        if let Some(flag) = flags {
            args.push(flag);
        }
        if name == "put_var" && self.class_strict && !self.function.strict {
            args.push(3);
            self.instruction("strict_op", args, pop, push, span)
        } else {
            self.instruction(name, args, pop, push, span)
        }
    }
    fn slot(&mut self, name: &'static str, slot: u32, pop: usize, push: usize, span: ir::Span) {
        self.instruction(name, (slot as u16).to_ne_bytes().to_vec(), pop, push, span)
    }
    fn label(&mut self) -> u32 {
        if self.budget.charge(Counter::Work, 1).is_err() {
            return 0;
        }
        let id = self.labels;
        self.labels += 1;
        id
    }
    fn mark(&mut self, id: u32) {
        if let Some(&depth) = self.label_depths.get(&id) {
            self.depth = depth;
        }
        if self.budget.charge(Counter::Work, 1).is_ok() {
            self.nodes.push(Node::Label(id));
        }
    }
    fn jump(&mut self, name: &'static str, label: u32, span: ir::Span) {
        if self
            .budget
            .charge(Counter::Instructions, 1)
            .and_then(|_| self.budget.charge(Counter::Code, 5))
            .and_then(|_| self.budget.charge(Counter::Positions, 1))
            .is_err()
        {
            return;
        }
        let pop = usize::from(!matches!(name, "goto" | "catch"));
        self.depth = self.depth.saturating_sub(pop) + usize::from(name == "catch");
        self.label_depths.entry(label).or_insert(self.depth);
        self.nodes.push(Node::Instruction(Instruction {
            name,
            args: vec![0; 4],
            target: Some(label),
            pop: usize::from(!matches!(name, "goto" | "catch")),
            push: usize::from(name == "catch"),
            taken: None,
            terminal: false,
            span,
            user_catch: false,
        }));
    }
    /// Emit a handler `catch` and, under debug markers, mark whether it
    /// installs a user catch clause (`true`) or a finally/cleanup handler.
    ///
    /// lunarstorm patch: the mark becomes a `FunctionImage::user_catches`
    /// entry in [`Self::finish`] instead of jetbunny's `debug_catch` opcode;
    /// the instruction is always a plain 5-byte `catch`.
    ///
    /// # Arguments
    /// * `label` - handler label the `catch` targets.
    /// * `span` - source span of the protected statement.
    /// * `user_catch` - whether the handler is a user `catch` clause.
    ///
    /// # Errors
    /// None today; the `Result` keeps jetbunny's call sites unchanged.
    fn handler(&mut self, label: u32, span: ir::Span, user_catch: bool) -> Result<(), String> {
        self.jump("catch", label, span);
        if self.options.debug_markers
            && let Some(Node::Instruction(op)) = self.nodes.last_mut()
            && op.name == "catch"
        {
            op.user_catch = user_catch;
        }
        Ok(())
    }
    /// `with_*` lookup: atom + label + flag; falls through with (pop, push), jumps with `taken`.
    #[allow(
        clippy::too_many_arguments,
        reason = "one instruction's operands and both of its stack effects"
    )]
    fn with_jump(
        &mut self,
        name: &'static str,
        units: &[u16],
        flag: u8,
        label: u32,
        pop: usize,
        push: usize,
        taken: (usize, usize),
        span: ir::Span,
    ) {
        if self
            .budget
            .charge(Counter::Instructions, 1)
            .and_then(|_| self.budget.charge(Counter::Code, 10))
            .and_then(|_| self.budget.charge(Counter::Positions, 1))
            .is_err()
        {
            return;
        }
        let atom = self.atom_units(units);
        let mut args = atom.to_ne_bytes().to_vec();
        args.extend([0; 4]);
        args.push(flag);
        let depth_before = self.depth;
        self.depth = self.depth.saturating_sub(pop) + push;
        self.label_depths
            .entry(label)
            .or_insert(depth_before.saturating_sub(taken.0) + taken.1);
        self.nodes.push(Node::Instruction(Instruction {
            name,
            args,
            target: Some(label),
            pop,
            push,
            taken: Some(taken),
            terminal: false,
            span,
            user_catch: false,
        }));
    }
    fn constant(&mut self, make: impl FnOnce() -> Constant, span: ir::Span) {
        if self.budget.charge(Counter::Constants, 1).is_err() {
            return;
        }
        let n = self.function.constants.len() as u32;
        self.function.constants.push(make());
        self.instruction("push_const", n.to_ne_bytes().to_vec(), 0, 1, span)
    }
    fn collect(&mut self, body: &'a [ir::Statement]) -> Result<(), String> {
        self.budget.charge(Counter::Work, 1)?;
        for statement in body {
            match &statement.kind {
                ir::StatementKind::Declaration(ds) => {
                    for declaration in ds {
                        self.declare_pattern(&declaration.pattern)?;
                    }
                }
                ir::StatementKind::Class { binding, .. } => self.declare(binding)?,
                ir::StatementKind::Function { binding, .. } => {
                    self.declare(binding)?;
                }
                ir::StatementKind::ForOf { left, body, .. }
                | ir::StatementKind::ForIn { left, body, .. } => {
                    if let ir::ForTarget::Declaration(declaration) = left {
                        self.declare_pattern(&declaration.pattern)?;
                    }
                    self.collect(std::slice::from_ref(body))?;
                }
                ir::StatementKind::TryFinally {
                    body, finalizer, ..
                } => {
                    self.collect(body)?;
                    self.collect(finalizer)?;
                }
                ir::StatementKind::Label { body, .. } => {
                    self.collect(std::slice::from_ref(body))?
                }
                ir::StatementKind::Switch { cases, .. } => {
                    for case in cases {
                        self.collect(&case.consequent)?;
                    }
                }
                ir::StatementKind::TryCatch {
                    body,
                    binding,
                    handler,
                    ..
                } => {
                    self.collect(body)?;
                    if let Some(pattern) = binding {
                        self.declare_pattern(pattern)?;
                    }
                    self.collect(handler)?;
                }
                ir::StatementKind::Block(body) => self.collect(body)?,
                ir::StatementKind::For { init, body, .. } => {
                    if let Some(init) = init {
                        self.collect(std::slice::from_ref(init))?;
                    }
                    self.collect(std::slice::from_ref(body))?;
                }
                ir::StatementKind::While { body, .. } | ir::StatementKind::DoWhile { body, .. } => {
                    self.collect(std::slice::from_ref(body))?
                }
                ir::StatementKind::If {
                    consequent,
                    alternate,
                    ..
                } => {
                    self.collect(std::slice::from_ref(consequent))?;
                    if let Some(s) = alternate {
                        self.collect(std::slice::from_ref(s))?;
                    }
                }
                ir::StatementKind::With { body, .. } => {
                    self.collect(std::slice::from_ref(body))?;
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn pattern_bindings(
        &mut self,
        pattern: &'a ir::Pattern,
    ) -> Result<Vec<&'a ir::Binding>, String> {
        self.budget.charge(Counter::Work, 1)?;
        let mut pending = vec![pattern];
        let mut bindings = Vec::new();
        let mut new_locals = std::collections::BTreeSet::new();
        while let Some(pattern) = pending.pop() {
            self.budget.charge(Counter::Work, 1)?;
            match pattern {
                ir::Pattern::Binding { binding, .. } => {
                    let local = self.root_locals
                        || binding.scope != self.program.scope
                        || self.parameter_profile && binding.kind == ir::DeclarationKind::Parameter;
                    if local && !self.env.contains_key(&binding.symbol) {
                        self.budget
                            .charge(Counter::Work, (new_locals.len() + 1).ilog2() as usize + 1)?;
                        if !new_locals.contains(&binding.symbol) {
                            self.budget.slots(
                                self.function.arg_count as usize
                                    + self.function.local_count as usize
                                    + new_locals.len()
                                    + 1,
                            )?;
                            new_locals.insert(binding.symbol);
                        }
                    }
                    bindings.push(binding);
                }
                ir::Pattern::Default { pattern, .. } => pending.push(pattern),
                ir::Pattern::Array { elements, rest } => {
                    self.budget
                        .charge(Counter::Work, elements.len() + usize::from(rest.is_some()))?;
                    if let Some(rest) = rest {
                        pending.push(rest);
                    }
                    pending.extend(elements.iter().rev().flatten());
                }
                ir::Pattern::Object { properties, rest } => {
                    self.budget.charge(
                        Counter::Work,
                        properties.len() + usize::from(rest.is_some()),
                    )?;
                    if let Some(rest) = rest {
                        pending.push(rest);
                    }
                    pending.extend(properties.iter().rev().map(|property| &property.pattern));
                }
                ir::Pattern::Target(_)
                | ir::Pattern::Member { .. }
                | ir::Pattern::SuperMember { .. }
                | ir::Pattern::PrivateMember { .. } => {}
            }
        }
        Ok(bindings)
    }
    fn declare_pattern(&mut self, pattern: &'a ir::Pattern) -> Result<(), String> {
        for binding in self.pattern_bindings(pattern)? {
            self.declare(binding)?;
        }
        Ok(())
    }
    fn declare(&mut self, b: &ir::Binding) -> Result<(), String> {
        self.declare_binding(b, false)
    }
    fn declare_binding(&mut self, b: &ir::Binding, private: bool) -> Result<(), String> {
        self.budget.charge(Counter::Work, 1)?;
        if self.env.contains_key(&b.symbol) {
            return Ok(());
        }
        let eval_global = (self.eval_global_var || self.eval_var_object.is_some())
            && b.scope == self.program.scope
            && matches!(
                b.kind,
                ir::DeclarationKind::Var | ir::DeclarationKind::Function
            );
        if self.eval_root
            && !eval_global
            && !self.program.strict
            && matches!(
                b.kind,
                ir::DeclarationKind::Var | ir::DeclarationKind::Function
            )
        {
            return Err("sloppy eval var/function environments not implemented".into());
        }
        self.budget.string(&b.name)?;
        let place =
            if !private && b.scope == self.program.scope && (!self.root_locals || eval_global) {
                Place::Global
            } else {
                let n = self.function.local_count;
                self.budget.slots(
                    self.function.arg_count as usize + self.function.local_count as usize + 1,
                )?;
                self.function.local_count += 1;
                if self.function.local_count > 65535 {
                    return Err("capacity: local count budget".into());
                }
                let previous = None;
                let reference = self.function.var_ref_count;
                self.function.var_ref_count += 1;
                self.function
                    .locals
                    .push(Self::vardef(b, reference, previous));
                self.local_scopes.insert(n, b.scope);
                if self.initializing_parameters {
                    self.parameter_region_locals.insert(n);
                }
                Place::Local(n)
            };
        self.env.insert(
            b.symbol,
            Variable {
                binding: b.clone(),
                place,
                native_const: matches!(
                    b.kind,
                    ir::DeclarationKind::Const | ir::DeclarationKind::Import
                ) || b.kind == ir::DeclarationKind::FunctionName
                    && self.function.strict,
                native_lexical: matches!(
                    b.kind,
                    ir::DeclarationKind::Let
                        | ir::DeclarationKind::Const
                        | ir::DeclarationKind::Import
                ),
                native_kind: if b.kind == ir::DeclarationKind::FunctionName {
                    4
                } else if b.kind == ir::DeclarationKind::Catch {
                    3
                } else {
                    0
                },
            },
        );
        Ok(())
    }
    /// Hidden `<var>` object of a sloppy function that calls `eval` directly: eval's
    /// `var`/`function` declarations live in it, so they stay local to this call.
    fn var_object_local(&mut self, span: ir::Span) -> Result<u64, String> {
        let id = self.next_auxiliary.get();
        self.next_auxiliary.set(
            id.checked_add(1)
                .ok_or("capacity: auxiliary identifier overflow")?,
        );
        let slot = self.temporary()?;
        let reference = self.function.var_ref_count;
        self.function.var_ref_count += 1;
        self.function.locals[(self.function.arg_count + slot) as usize] = VarDef {
            name: "<var>".encode_utf16().collect(),
            is_captured: true,
            var_ref_idx: reference,
            ..Default::default()
        };
        self.local_scopes.insert(slot, self.scope);
        self.auxiliary.insert(
            id,
            Variable {
                binding: ir::Binding {
                    symbol: 0,
                    scope: self.scope,
                    name: "<var>".into(),
                    kind: ir::DeclarationKind::Var,
                    span,
                },
                place: Place::Local(slot),
                native_const: false,
                native_lexical: false,
                native_kind: 0,
            },
        );
        self.instruction("special_object", vec![5], 0, 1, span);
        self.slot("put_loc", slot, 1, 0, span);
        Ok(id)
    }
    /// Eval `var name` in a `<var>` object: defines it as undefined unless already present.
    fn define_var_object(&mut self, object: u64, name: &str, span: ir::Span) -> Result<(), String> {
        let done = self.label();
        self.atom_op("push_atom_value", name, 0, 1, None, span);
        self.read_auxiliary(object, span)?;
        self.simple("in", 2, 1, span);
        self.jump("if_true", done, span);
        self.read_auxiliary(object, span)?;
        self.simple("undefined", 0, 1, span);
        self.atom_op("define_field", name, 2, 1, None, span);
        self.simple("drop", 1, 0, span);
        self.mark(done);
        Ok(())
    }
    fn compile(
        mut self,
        body: &'a [ir::Statement],
    ) -> Result<(FunctionImage, Vec<image::FunctionSpans>), String> {
        self.budget.charge(Counter::Work, 1)?;
        if let Some(binding) = self.self_binding.clone() {
            self.declare(&binding)?;
            if let Place::Local(slot) = self.env[&binding.symbol].place {
                let definition =
                    &mut self.function.locals[(self.function.arg_count + slot) as usize];
                definition.var_kind = 4;
                definition.is_const = self.function.strict;
            }
        }
        if let Some(binding) = self.arguments_binding.clone() {
            self.declare_binding(&binding, true)?;
            let variable = self
                .env
                .get_mut(&binding.symbol)
                .expect("declared arguments binding");
            variable.native_kind = image::VAR_KIND_ARGUMENTS;
            if let Place::Local(slot) = variable.place {
                self.function.locals[(self.function.arg_count + slot) as usize].var_kind =
                    image::VAR_KIND_ARGUMENTS;
            }
        }
        if !self.is_arrow
            && (self.options.debug_markers
                || self.uses_this
                || self.is_class_constructor
                || self.uses_home_object)
            && let Some(binding) = self.this_binding.clone()
        {
            self.declare_binding(&binding, true)?;
        }
        if !self.is_arrow
            && (self.options.debug_markers || self.uses_new_target || self.derived_constructor)
            && let Some(binding) = self.new_target_binding.clone()
        {
            self.declare_binding(&binding, true)?;
        }
        if let Some(function) = self.source_function {
            if self.parameter_profile {
                for pattern in function
                    .parameters
                    .iter()
                    .chain(function.rest_parameter.iter())
                {
                    for binding in self.pattern_bindings(pattern)? {
                        self.declare_binding(binding, true)?;
                        let variable = self
                            .env
                            .get_mut(&binding.symbol)
                            .ok_or("missing parameter local")?;
                        let Place::Local(slot) = variable.place else {
                            return Err("parameter needs a private local".into());
                        };
                        variable.native_lexical = true;
                        let definition =
                            &mut self.function.locals[(self.function.arg_count + slot) as usize];
                        definition.is_lexical = true;
                        definition.scope_level = 1;
                        self.parameter_slots.push(slot);
                    }
                }
            } else if let Some(rest) = &function.rest_parameter {
                self.declare_pattern(rest)?;
            }
            if self.parameter_profile && self.parameter_slots.is_empty() {
                let slot = self.temporary()?;
                let definition =
                    &mut self.function.locals[(self.function.arg_count + slot) as usize];
                definition.is_lexical = true;
                definition.is_captured = true;
                definition.scope_level = 1;
                definition.var_ref_idx = self.function.var_ref_count;
                self.function.var_ref_count += 1;
                self.parameter_slots.push(slot);
                self.parameter_anchor = Some(slot);
            }
            for copy in &function.body_parameter_copies {
                self.declare(&copy.binding)?;
            }
        }
        if !self.is_arrow {
            if (self.options.debug_markers || self.uses_home_object)
                && let Some(binding) = self.home_object_binding.clone()
            {
                self.declare_binding(&binding, true)?;
                self.function.flags.need_home_object = true;
            }
            if let Some(binding) = self.super_constructor_binding.clone() {
                self.declare_binding(&binding, true)?;
            }
        }
        if self.derived_constructor
            && let Some(binding) = &self.this_binding
        {
            let variable = self
                .env
                .get_mut(&binding.symbol)
                .ok_or("derived constructor lacks this cell")?;
            let Place::Local(slot) = variable.place else {
                return Err("derived this needs an own local".into());
            };
            variable.native_lexical = true;
            self.function.locals[(self.function.arg_count + slot) as usize].is_lexical = true;
        }
        self.collect(body)?;
        let span = ir::Span {
            start: self.function_span.start,
            end: self.function_span.start,
        };
        if self.is_class_constructor {
            self.simple("check_ctor", 0, 0, span);
        }
        self.simple("undefined", 0, 1, span);
        self.slot("put_loc", 0, 1, 0, span);
        if let Some(binding) = self.arguments_binding.clone() {
            self.instruction(
                "special_object",
                vec![
                    if self.function.strict || !self.function.flags.simple_parameters {
                        0
                    } else {
                        1
                    },
                ],
                0,
                1,
                binding.span,
            );
            self.store(
                &ir::Reference {
                    symbol: Some(binding.symbol),
                    name: binding.name.clone(),
                },
                true,
                binding.span,
            )?;
        }
        if !self.is_arrow
            && (self.options.debug_markers
                || self.uses_this
                || self.is_class_constructor
                || self.uses_home_object)
            && let Some(binding) = self.this_binding.clone()
        {
            if self.derived_constructor {
                let Place::Local(slot) = self.env[&binding.symbol].place else {
                    return Err("derived this needs a local".into());
                };
                self.slot("set_loc_uninitialized", slot, 0, 0, binding.span);
            } else {
                self.simple("push_this", 0, 1, binding.span);
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
        if !self.is_arrow
            && (self.options.debug_markers || self.uses_new_target || self.derived_constructor)
            && let Some(binding) = self.new_target_binding.clone()
        {
            self.instruction("special_object", vec![3], 0, 1, binding.span);
            self.store(
                &ir::Reference {
                    symbol: Some(binding.symbol),
                    name: binding.name.clone(),
                },
                true,
                binding.span,
            )?;
        }
        if let Some(binding) = self.self_binding.clone() {
            self.instruction("special_object", vec![2], 0, 1, binding.span);
            self.store(
                &ir::Reference {
                    symbol: Some(binding.symbol),
                    name: binding.name.clone(),
                },
                true,
                binding.span,
            )?;
        }
        if !self.is_arrow {
            if self.function.flags.need_home_object
                && let Some(binding) = self.home_object_binding.clone()
            {
                self.instruction("special_object", vec![4], 0, 1, binding.span);
                self.store(
                    &ir::Reference {
                        symbol: Some(binding.symbol),
                        name: binding.name.clone(),
                    },
                    true,
                    binding.span,
                )?;
            }
            if let Some(binding) = self.super_constructor_binding.clone() {
                self.instruction("special_object", vec![2], 0, 1, binding.span);
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
        if self.is_class_constructor && !self.derived_constructor {
            self.initialize_instance_fields(span)?;
        }
        if let Some(function) = self.source_function {
            if self.parameter_profile {
                self.initializing_parameters = true;
                self.budget
                    .charge(Counter::Work, self.parameter_slots.len())?;
                self.blocks.push(self.parameter_slots.clone());
                for index in 0..self.parameter_slots.len() {
                    self.slot(
                        "set_loc_uninitialized",
                        self.parameter_slots[index],
                        0,
                        0,
                        span,
                    );
                }
                if let Some(slot) = self.parameter_anchor {
                    self.simple("undefined", 0, 1, span);
                    self.slot("put_loc_check_init", slot, 1, 0, span);
                }
                self.marker(span)?;
                for (index, pattern) in function.parameters.iter().enumerate() {
                    self.slot("get_arg", index as u32, 0, 1, span);
                    self.pattern(pattern, span)?;
                }
            }
            if let Some(rest) = &function.rest_parameter {
                self.instruction(
                    "rest",
                    (self.function.arg_count as u16).to_ne_bytes().to_vec(),
                    0,
                    1,
                    span,
                );
                self.pattern(rest, span)?;
            }
            self.initializing_parameters = false;
            for copy in &function.body_parameter_copies {
                self.read(&copy.parameter, false, copy.binding.span)?;
                self.store(
                    &ir::Reference {
                        symbol: Some(copy.binding.symbol),
                        name: copy.binding.name.clone(),
                    },
                    false,
                    copy.binding.span,
                )?;
            }
        }
        // After the arguments prefix the verifier accepts; before any body code.
        if let Some(function) = self.source_function
            && function.has_direct_eval
            && !self.eval_root
            && !self.function.strict
        {
            let id = self.var_object_local(function.span)?;
            self.withs.push((id, self.scope, 0, None));
        }
        let globals: Vec<_> = self
            .env
            .values()
            .filter(|v| matches!(v.place, Place::Global) && v.binding.scope == self.scope)
            .cloned()
            .collect();
        // Direct-eval declarations are configurable (JS_PROP_CONFIGURABLE).
        let configurable = u8::from(self.eval_global_var);
        if let Some(object) = self.eval_var_object {
            for v in &globals {
                if v.binding.kind != ir::DeclarationKind::Function {
                    self.define_var_object(object, &v.binding.name, v.binding.span)?;
                }
            }
        }
        let plain: &[_] = if self.eval_var_object.is_some() {
            &[]
        } else {
            &globals
        };
        for v in plain {
            let flags = Self::global_flags(&v.binding);
            self.atom_op(
                "check_define_var",
                &v.binding.name,
                0,
                0,
                Some(flags),
                v.binding.span,
            );
        }
        for v in plain {
            if v.binding.kind == ir::DeclarationKind::Function {
                continue;
            }
            self.atom_op(
                "define_var",
                &v.binding.name,
                0,
                0,
                Some(Self::global_flags(&v.binding) | configurable),
                v.binding.span,
            );
        }
        if matches!(
            self.kind,
            ir::FunctionKind::Generator | ir::FunctionKind::AsyncGenerator
        ) {
            self.simple("initial_yield", 0, 0, span);
        }
        if self.parameter_profile && self.options.debug_markers {
            let shadowed_arguments = self.function.locals.iter().any(|v| {
                v.scope_level == 0
                    && v.var_kind == 0
                    && !v.is_lexical
                    && v.name == "arguments".encode_utf16().collect::<Vec<_>>()
            });
            let slot = self.temporary()?;
            let definition = &mut self.function.locals[(self.function.arg_count + slot) as usize];
            definition.is_lexical = true;
            definition.is_captured = true;
            definition.scope_level = 2;
            definition.var_ref_idx = self.function.var_ref_count;
            self.function.var_ref_count += 1;
            definition.var_kind = if shadowed_arguments {
                image::VAR_KIND_BODY_ARGUMENTS_ANCHOR
            } else {
                image::VAR_KIND_BODY_ANCHOR
            };
            self.local_scopes.insert(slot, self.scope);
            self.pending_body_anchor = Some(slot);
        }
        self.block(body)?;
        if self.parameter_profile {
            self.end_block(span);
        }
        if self.scope == self.program.scope {
            self.slot("get_loc", 0, 0, 1, span);
            self.simple("return", 1, 0, span);
        } else {
            self.return_value(false, span)?;
        }
        self.finish()
    }
    fn global_flags(b: &ir::Binding) -> u8 {
        match b.kind {
            ir::DeclarationKind::Let => 128 | 2,
            ir::DeclarationKind::Const => 128,
            ir::DeclarationKind::Function => 64,
            _ => 0,
        }
    }
    fn begin_block(&mut self, body: &[&'a ir::Statement]) -> Result<(), String> {
        self.budget.charge(Counter::Work, body.len() + 1)?;
        let mut scoped = Vec::new();
        for &statement in body {
            if let ir::StatementKind::Declaration(declarations) = &statement.kind {
                for declaration in declarations {
                    for binding in self.pattern_bindings(&declaration.pattern)? {
                        if matches!(
                            binding.kind,
                            ir::DeclarationKind::Let
                                | ir::DeclarationKind::Const
                                | ir::DeclarationKind::Import
                        ) && let Place::Local(slot) = self.env[&binding.symbol].place
                        {
                            scoped.push((slot, binding.span));
                        }
                    }
                }
            }
        }
        for &statement in body {
            if let ir::StatementKind::Class { binding, .. } = &statement.kind
                && let Place::Local(slot) = self.env[&binding.symbol].place
            {
                scoped.push((slot, binding.span));
            }
        }
        if let Some(anchor) = self.pending_body_anchor.take() {
            scoped.push((anchor, self.function_span));
        }
        self.budget.charge(Counter::Work, scoped.len())?;
        // Block-scoped function declarations are closed with the block (fresh
        // per loop iteration) but initialized below, never left uninitialized.
        let mut closed: Vec<u32> = scoped.iter().map(|&(slot, _)| slot).collect();
        for &statement in body {
            if let ir::StatementKind::Function { binding, .. } = &statement.kind
                && binding.scope != self.scope
                && let Place::Local(slot) = self.env[&binding.symbol].place
            {
                closed.push(slot);
            }
        }
        self.push_block(closed);
        for (slot, span) in scoped {
            self.slot("set_loc_uninitialized", slot, 0, 0, span);
        }
        // Only the module body's own declarations were already hoisted.
        let skip_hoisted = std::mem::take(&mut self.skip_hoisted);
        for &statement in body {
            if let ir::StatementKind::Function { binding, function } = &statement.kind {
                if skip_hoisted {
                    continue;
                }
                if let Some(object) = self.eval_var_object
                    && matches!(self.env[&binding.symbol].place, Place::Global)
                {
                    self.read_auxiliary(object, binding.span)?;
                    self.function_value(function, binding.span)?;
                    self.atom_op("define_field", &binding.name, 2, 1, None, binding.span);
                    self.simple("drop", 1, 0, binding.span);
                    continue;
                }
                self.function_value(function, binding.span)?;
                let reference = ir::Reference {
                    symbol: Some(binding.symbol),
                    name: binding.name.clone(),
                };
                if matches!(self.env[&binding.symbol].place, Place::Global) {
                    let flags = u8::from(self.eval_global_var);
                    self.atom_op(
                        "define_func",
                        &binding.name,
                        1,
                        0,
                        Some(flags),
                        binding.span,
                    );
                } else {
                    self.store(&reference, false, binding.span)?;
                }
            }
        }
        Ok(())
    }
    /// Annex B.3.2: evaluating a sloppy block-level function declaration copies
    /// the block binding into the same-named var-scope binding. oxc already
    /// moves the symbol to the var scope when that scope had no such binding.
    fn annex_b_copy(&mut self, binding: &'a ir::Binding) -> Result<(), String> {
        if self.function.strict || binding.scope == self.scope {
            return Ok(());
        }
        let Some(own) = self.env.get(&binding.symbol) else {
            return Ok(());
        };
        if own.binding.scope == self.scope {
            return Ok(());
        }
        self.budget
            .charge(Counter::Work, self.env.len() + self.program.scopes.len())?;
        // Scopes strictly between the block and the var scope.
        let mut between = Vec::new();
        let mut cursor = self
            .program
            .scopes
            .iter()
            .find(|s| s.id == binding.scope)
            .and_then(|s| s.parent);
        while let Some(id) = cursor
            && id != self.scope
        {
            between.push(id);
            cursor = self
                .program
                .scopes
                .iter()
                .find(|s| s.id == id)
                .and_then(|s| s.parent);
        }
        let mut target = None;
        for variable in self.env.values() {
            let b = &variable.binding;
            if b.name != binding.name {
                continue;
            }
            if between.contains(&b.scope) && b.kind != ir::DeclarationKind::Function {
                return Ok(()); // a lexical binding in between: `var F` would be an early error
            }
            if b.scope == self.scope
                && matches!(
                    b.kind,
                    ir::DeclarationKind::Var | ir::DeclarationKind::Function
                )
            {
                target = Some(b.symbol);
            }
        }
        let Some(target) = target else {
            return Ok(());
        };
        let span = binding.span;
        self.read(
            &ir::Reference {
                symbol: Some(binding.symbol),
                name: binding.name.clone(),
            },
            false,
            span,
        )?;
        self.store(
            &ir::Reference {
                symbol: Some(target),
                name: binding.name.clone(),
            },
            false,
            span,
        )
    }
    fn scope_level(&self, slot: u32) -> u32 {
        self.function.locals[(self.function.arg_count + slot) as usize].scope_level
    }
    /// The innermost lexical slot: the head of the scope chain that frame
    /// inspection (debug markers, direct eval) walks to find visible bindings.
    fn scope_head(&self) -> Option<u32> {
        self.blocks
            .iter()
            .rev()
            // Scope links are built in slot order by finish(). Class bindings
            // can be appended to a block after later declarations, so block
            // insertion order does not identify the head of that chain.
            .find_map(|slots| {
                slots
                    .iter()
                    .copied()
                    .filter(|&slot| self.scope_level(slot) > 0)
                    .max()
            })
    }
    fn push_block(&mut self, slots: Vec<u32>) {
        self.blocks.push(slots);
    }
    /// Drop captures that neither this function's code nor its closures use,
    /// renumbering the rest. Functions with direct eval, and debug builds,
    /// keep every capture: the frame is inspected by name at run time.
    fn prune_captures(&mut self) -> Result<(), String> {
        const REFS: [&str; 5] = [
            "get_var_ref",
            "get_var_ref_check",
            "put_var_ref",
            "put_var_ref_check",
            "put_var_ref_check_init",
        ];
        if self.source_function.is_none()
            || self.options.debug_markers
            || self.function.captures.is_empty()
        {
            return Ok(());
        }
        self.budget.charge(
            Counter::Work,
            self.nodes.len() + self.function.captures.len(),
        )?;
        let mut used = vec![false; self.function.captures.len()];
        for node in &self.nodes {
            if let Node::Instruction(op) = node {
                if matches!(op.name, "eval" | "apply_eval") {
                    return Ok(());
                }
                if REFS.contains(&op.name) {
                    let index = u16::from_ne_bytes([op.args[0], op.args[1]]) as usize;
                    *used.get_mut(index).ok_or("capture index out of range")? = true;
                }
            }
        }
        for constant in &self.function.constants {
            if let Constant::Function(child) = constant {
                for capture in child.captures.iter().filter(|c| c.closure_type == 2) {
                    *used
                        .get_mut(capture.var_idx as usize)
                        .ok_or("child capture index out of range")? = true;
                }
            }
        }
        let mut remap = vec![u16::MAX; used.len()];
        let mut kept = Vec::new();
        for (index, capture) in std::mem::take(&mut self.function.captures)
            .into_iter()
            .enumerate()
        {
            if used[index] {
                remap[index] = kept.len() as u16;
                kept.push(capture);
            }
        }
        self.function.captures = kept;
        for node in &mut self.nodes {
            if let Node::Instruction(op) = node
                && REFS.contains(&op.name)
            {
                let index = u16::from_ne_bytes([op.args[0], op.args[1]]) as usize;
                op.args[..2].copy_from_slice(&remap[index].to_ne_bytes());
            }
        }
        for constant in &mut self.function.constants {
            if let Constant::Function(child) = constant {
                for capture in child.captures.iter_mut().filter(|c| c.closure_type == 2) {
                    capture.var_idx = u32::from(remap[capture.var_idx as usize]);
                }
            }
        }
        Ok(())
    }
    fn end_block(&mut self, span: ir::Span) {
        for slot in self.blocks.pop().expect("block entry") {
            self.slot("close_loc", slot, 0, 0, span);
        }
    }
    fn block(&mut self, body: &'a [ir::Statement]) -> Result<(), String> {
        self.budget.charge(Counter::Work, body.len())?;
        let statements: Vec<_> = body.iter().collect();
        self.begin_block(&statements)?;
        for statement in body {
            self.statement(statement)?;
        }
        self.end_block(
            body.last()
                .map_or(ir::Span { start: 0, end: 0 }, |s| s.span),
        );
        Ok(())
    }
    fn marker(&mut self, span: ir::Span) -> Result<(), String> {
        self.budget.charge(Counter::Work, 1)?;
        if !self.options.debug_markers {
            return Ok(());
        }
        if self.markers >= 0x7fff {
            return Err("capacity: debug marker count budget".into());
        }
        let low = self.scope_head().map_or(0, |slot| slot + 1);
        if low >= 0xffff {
            return Err("capacity: debug marker scope-head budget".into());
        }
        let operand = (self.markers << 16) | low;
        self.markers += 1;
        self.instruction("debug_stmt", operand.to_ne_bytes().to_vec(), 0, 0, span);
        Ok(())
    }
    fn clone_iteration_slots(&mut self, slots: &[u32], span: ir::Span) -> Result<(), String> {
        self.budget.charge(Counter::Work, slots.len() + 1)?;
        for &slot in slots {
            let temporary = self.temporary()?;
            self.slot("get_loc_check", slot, 0, 1, span);
            self.slot("put_loc", temporary, 1, 0, span);
            self.slot("close_loc", slot, 0, 0, span);
            self.slot("set_loc_uninitialized", slot, 0, 0, span);
            self.slot("get_loc", temporary, 0, 1, span);
            self.slot("put_loc_check_init", slot, 1, 0, span);
            self.clear_temporaries(&[temporary], span);
        }
        Ok(())
    }
    fn close_blocks(&mut self, depth: usize, span: ir::Span) -> Result<(), String> {
        let count = self.blocks[depth..].iter().map(Vec::len).sum();
        self.budget.charge(Counter::Work, count)?;
        let slots: Vec<_> = self.blocks[depth..]
            .iter()
            .rev()
            .flat_map(|slots| slots.iter().rev().copied())
            .collect();
        for slot in slots {
            self.slot("close_loc", slot, 0, 0, span);
        }
        Ok(())
    }
    fn scope_slots(&mut self, scope: u32) -> Result<Vec<u32>, String> {
        self.budget.charge(
            Counter::Work,
            self.program
                .scopes
                .len()
                .saturating_add(self.local_scopes.len()),
        )?;
        let parents: BTreeMap<_, _> = self
            .program
            .scopes
            .iter()
            .map(|scope| (scope.id, scope.parent))
            .collect();
        let mut slots = Vec::new();
        for (&slot, &source_scope) in &self.local_scopes {
            let definition = &self.function.locals[(self.function.arg_count + slot) as usize];
            if !(definition.is_lexical || definition.var_kind == 3) {
                continue;
            }
            let mut ancestor = Some(source_scope);
            while let Some(current) = ancestor {
                self.budget.charge(Counter::Work, 1)?;
                if current == scope {
                    slots.push(slot);
                    break;
                }
                ancestor = parents.get(&current).copied().flatten();
            }
        }
        Ok(slots)
    }
    fn jump_target(&self, label: u32) -> JumpTarget {
        JumpTarget {
            label,
            blocks: self.blocks.len(),
            handlers: self.handler_depth,
            unwind: self.unwind.len(),
        }
    }
    fn enter_loop(&mut self, next: u32, end: u32, labels: &[usize]) -> Result<(), String> {
        self.budget.charge(Counter::Work, labels.len() + 1)?;
        let continue_target = self.jump_target(next);
        for &label in labels {
            self.controls[label].continue_target = Some(continue_target);
        }
        self.controls.push(Control {
            name: None,
            break_target: self.jump_target(end),
            continue_target: Some(continue_target),
            unlabeled: true,
        });
        Ok(())
    }
    fn cleanup_to(&mut self, blocks: usize, handlers: usize, span: ir::Span) -> Result<(), String> {
        self.close_blocks(blocks, span)?;
        self.blocks.truncate(blocks);
        for _ in handlers..self.handler_depth {
            self.simple("drop", 1, 0, span);
        }
        self.handler_depth = handlers;
        Ok(())
    }
    fn completion(&mut self, completion: Completion, span: ir::Span) -> Result<(), String> {
        let finalizer_depth = match completion {
            Completion::Return(_) => 0,
            Completion::Branch(target) => target.unwind,
        };
        let state_cells = self
            .blocks
            .iter()
            .map(Vec::len)
            .sum::<usize>()
            .saturating_add(self.blocks.len())
            .saturating_add(self.controls.len())
            .saturating_add(self.unwind.len());
        self.budget.charge(Counter::Work, state_cells)?;
        let original_blocks = self.blocks.clone();
        let original_controls = self.controls.clone();
        let original_unwind = self.unwind.clone();
        let original_handlers = self.handler_depth;
        for index in (finalizer_depth..original_unwind.len()).rev() {
            let finalizer = original_unwind[index];
            // Close only the protected descendants before executing finally;
            // enclosing cells remain live for reads, writes and new closures.
            self.cleanup_to(finalizer.blocks, finalizer.handlers, span)?;
            self.controls.truncate(finalizer.controls);
            self.unwind.truncate(index);
            match finalizer.kind {
                UnwindKind::Finally(body) => {
                    let previous_value = self.temporary()?;
                    self.slot("get_loc", 0, 0, 1, span);
                    self.slot("put_loc", previous_value, 1, 0, span);
                    self.block(body)?;
                    self.slot("get_loc", previous_value, 0, 1, span);
                    self.slot("put_loc", 0, 1, 0, span);
                }
                UnwindKind::ForOf => self.simple("iterator_close", 3, 0, span),
                UnwindKind::ForAwait => self.async_iterator_close(span),
                UnwindKind::ForIn => self.simple("drop", 1, 0, span),
            }
        }
        match completion {
            Completion::Return(saved) => {
                self.cleanup_to(0, 0, span)?;
                if let Some(slot) = saved {
                    self.slot("get_loc", slot, 0, 1, span);
                    self.return_value(true, span)?;
                } else {
                    self.return_value(false, span)?;
                }
            }
            Completion::Branch(target) => {
                self.cleanup_to(target.blocks, target.handlers, span)?;
                self.jump("goto", target.label, span);
            }
        }
        self.blocks = original_blocks;
        self.controls = original_controls;
        self.unwind = original_unwind;
        self.handler_depth = original_handlers;
        Ok(())
    }
    fn finalizer_cleanup(
        &mut self,
        body: &[ir::Statement],
        scope: Option<u32>,
    ) -> Result<Vec<u32>, String> {
        if let Some(scope) = scope {
            return self.scope_slots(scope);
        }
        let [
            ir::Statement {
                kind:
                    ir::StatementKind::TryCatch {
                        body_scope,
                        catch_scope,
                        handler_scope,
                        ..
                    },
                ..
            },
        ] = body
        else {
            return Err("synthetic finally body must contain its owned try/catch".into());
        };
        let mut slots = self.scope_slots(*body_scope)?;
        slots.extend(self.scope_slots(*catch_scope)?);
        slots.extend(self.scope_slots(*handler_scope)?);
        self.budget
            .charge(Counter::Work, slots.len().saturating_mul(2))?;
        slots.sort_unstable();
        slots.dedup();
        Ok(slots)
    }
    fn switch(
        &mut self,
        discriminant: &'a ir::Expression,
        cases: &'a [ir::SwitchCase],
        span: ir::Span,
    ) -> Result<(), String> {
        let count = cases
            .iter()
            .map(|case| case.consequent.len())
            .sum::<usize>();
        self.budget
            .charge(Counter::Work, count.saturating_add(cases.len()))?;
        let statements: Vec<_> = cases
            .iter()
            .flat_map(|case| case.consequent.iter())
            .collect();
        let saved = self.temporary()?;
        let end = self.label();
        self.simple("undefined", 0, 1, span);
        self.slot("put_loc", 0, 1, 0, span);
        self.expression(discriminant)?;
        self.slot("put_loc", saved, 1, 0, span);
        self.begin_block(&statements)?;
        let mut entries = Vec::with_capacity(cases.len());
        for _ in cases {
            entries.push(self.label());
        }
        let mut default = end;
        for (case, &entry) in cases.iter().zip(&entries) {
            if let Some(test) = &case.test {
                self.marker(test.span)?;
                self.slot("get_loc", saved, 0, 1, test.span);
                self.expression(test)?;
                self.simple("strict_eq", 2, 1, test.span);
                self.jump("if_true", entry, test.span);
            } else {
                default = entry;
            }
        }
        self.jump("goto", default, span);
        self.controls.push(Control {
            name: None,
            break_target: self.jump_target(end),
            continue_target: None,
            unlabeled: true,
        });
        for (case, entry) in cases.iter().zip(entries) {
            self.mark(entry);
            for statement in &case.consequent {
                self.statement(statement)?;
            }
        }
        self.controls.pop();
        self.mark(end);
        self.end_block(span);
        self.clear_temporaries(&[saved], span);
        Ok(())
    }
    fn this_value(&mut self, span: ir::Span) -> Result<(), String> {
        let binding = self
            .this_binding
            .clone()
            .ok_or("missing lexical this owner")?;
        self.read(
            &ir::Reference {
                symbol: Some(binding.symbol),
                name: binding.name.clone(),
            },
            false,
            span,
        )
    }
    fn return_value(&mut self, has_value: bool, span: ir::Span) -> Result<(), String> {
        if self.kind != ir::FunctionKind::Normal {
            if !has_value {
                self.simple("undefined", 0, 1, span);
            }
            self.simple("return_async", 1, 0, span);
            return Ok(());
        }
        if !self.derived_constructor {
            self.simple(
                if has_value { "return" } else { "return_undef" },
                usize::from(has_value),
                0,
                span,
            );
            return Ok(());
        }
        if has_value {
            let object = self.label();
            self.simple("check_ctor_return", 1, 2, span);
            self.jump("if_false", object, span);
            self.simple("drop", 1, 0, span);
            self.this_value(span)?;
            self.simple("return", 1, 0, span);
            self.mark(object);
            self.simple("return", 1, 0, span);
        } else {
            self.this_value(span)?;
            self.simple("return", 1, 0, span);
        }
        Ok(())
    }
    /// AsyncIteratorClose of the record on top of the stack: dissolve the
    /// record, call and await `return()` when the iterator is still live.
    fn async_iterator_close(&mut self, span: ir::Span) {
        let no_return = self.label();
        let done = self.label();
        self.simple("drop", 1, 0, span);
        self.simple("drop", 1, 0, span);
        self.simple("dup", 1, 2, span);
        self.simple("is_undefined", 1, 1, span);
        self.jump("if_true", done, span);
        self.atom_op("get_field2", "return", 1, 2, None, span);
        self.simple("dup", 1, 2, span);
        self.simple("is_undefined_or_null", 1, 1, span);
        self.jump("if_true", no_return, span);
        self.instruction("call_method", 0u16.to_ne_bytes().to_vec(), 2, 1, span);
        self.simple("await", 1, 1, span);
        self.simple("check_object", 1, 1, span);
        self.jump("goto", done, span);
        self.mark(no_return);
        self.simple("drop", 1, 0, span);
        self.mark(done);
        self.simple("drop", 1, 0, span);
    }
    /// A generator resumed with a return completion: the value is on the
    /// stack above the current statement's expression temporaries.
    fn yield_return(&mut self, span: ir::Span) -> Result<(), String> {
        if self.kind == ir::FunctionKind::AsyncGenerator {
            self.simple("await", 1, 1, span);
        }
        let slot = self.temporary()?;
        self.slot("put_loc", slot, 1, 0, span);
        let records = self.expr_records.clone();
        let mut open = records.len();
        while self.depth > self.statement_base {
            if open > 0 && records[open - 1] + 3 == self.depth {
                open -= 1;
                self.simple("iterator_close", 3, 0, span);
            } else {
                self.simple("drop", 1, 0, span);
            }
        }
        self.completion(Completion::Return(Some(slot)), span)
    }
    /// `yield*` with the iterable on the stack; leaves the delegate's result.
    fn yield_star(&mut self, span: ir::Span) -> Result<(), String> {
        let is_async = self.kind == ir::FunctionKind::AsyncGenerator;
        let looping = self.label();
        let yielding = self.label();
        let finished = self.label();
        let returning = self.label();
        let returned = self.label();
        let throwing = self.label();
        let no_throw = self.label();
        let closed = self.label();
        self.simple(
            if is_async {
                "for_await_of_start"
            } else {
                "for_of_start"
            },
            1,
            3,
            span,
        );
        // Delegation closes the iterator itself: dissolve the private record.
        self.simple("drop", 1, 0, span);
        self.simple("undefined", 0, 1, span);
        self.simple("undefined", 0, 1, span);
        self.mark(looping);
        self.simple("iterator_next", 4, 4, span);
        if is_async {
            self.simple("await", 1, 1, span);
        }
        self.simple("check_object", 1, 1, span);
        self.atom_op("get_field2", "done", 1, 2, None, span);
        self.jump("if_true", finished, span);
        self.mark(yielding);
        if is_async {
            self.atom_op("get_field", "value", 1, 1, None, span);
            self.simple("async_yield_star", 1, 2, span);
        } else {
            self.simple("yield_star", 1, 2, span);
        }
        self.simple("dup", 1, 2, span);
        self.jump("if_true", returning, span);
        self.simple("drop", 1, 0, span);
        self.jump("goto", looping, span);
        self.mark(returning);
        self.instruction("push_i32", 2i32.to_ne_bytes().to_vec(), 0, 1, span);
        self.simple("strict_eq", 2, 1, span);
        self.jump("if_true", throwing, span);
        if is_async {
            self.simple("await", 1, 1, span);
        }
        self.instruction("iterator_call", vec![0], 4, 5, span);
        self.jump("if_true", returned, span);
        if is_async {
            self.simple("await", 1, 1, span);
        }
        self.simple("check_object", 1, 1, span);
        self.atom_op("get_field2", "done", 1, 2, None, span);
        self.jump("if_false", yielding, span);
        self.atom_op("get_field", "value", 1, 1, None, span);
        self.mark(returned);
        for _ in 0..3 {
            self.simple("nip", 2, 1, span);
        }
        self.yield_return(span)?;
        self.mark(throwing);
        self.instruction("iterator_call", vec![1], 4, 5, span);
        self.jump("if_true", no_throw, span);
        if is_async {
            self.simple("await", 1, 1, span);
        }
        self.simple("check_object", 1, 1, span);
        self.atom_op("get_field2", "done", 1, 2, None, span);
        self.jump("if_false", yielding, span);
        self.jump("goto", finished, span);
        self.mark(no_throw);
        self.instruction("iterator_call", vec![2], 4, 5, span);
        self.jump("if_true", closed, span);
        if is_async {
            self.simple("await", 1, 1, span);
        }
        self.mark(closed);
        // JS_THROW_ERROR_ITERATOR_THROW: the delegate has no throw() method.
        self.atom_op("throw_error", "", 0, 0, Some(4), span);
        self.mark(finished);
        self.atom_op("get_field", "value", 1, 1, None, span);
        for _ in 0..3 {
            self.simple("nip", 2, 1, span);
        }
        Ok(())
    }
    fn initialize_instance_fields(&mut self, span: ir::Span) -> Result<(), String> {
        let Some(binding) = self.field_initializer_binding.clone() else {
            return Ok(());
        };
        let absent = self.label();
        self.read(
            &ir::Reference {
                symbol: Some(binding.symbol),
                name: binding.name.clone(),
            },
            false,
            span,
        )?;
        self.simple("dup", 1, 2, span);
        self.jump("if_false", absent, span);
        self.this_value(span)?;
        self.simple("swap", 2, 2, span);
        self.instruction("call_method", 0u16.to_ne_bytes().to_vec(), 2, 1, span);
        self.mark(absent);
        self.simple("drop", 1, 0, span);
        Ok(())
    }
    fn infer_pattern_name(
        &mut self,
        pattern: &ir::Pattern,
        value: &ir::Expression,
    ) -> Result<(), String> {
        if matches!(&value.kind, ir::ExpressionKind::Function(function) if function.name.is_none())
        {
            let name = match pattern {
                ir::Pattern::Binding { binding, .. } => Some(binding.name.as_str()),
                ir::Pattern::Target(reference) => Some(reference.name.as_str()),
                _ => None,
            };
            if let Some(name) = name {
                self.atom_op("set_name", name, 1, 1, None, value.span);
            }
        }
        Ok(())
    }
    fn prepare_pattern_reference(
        &mut self,
        pattern: &'a ir::Pattern,
        span: ir::Span,
    ) -> Result<Option<PreparedReference>, String> {
        match pattern {
            ir::Pattern::Default { pattern, .. } => self.prepare_pattern_reference(pattern, span),
            ir::Pattern::Member { object, key } => {
                let base = self.temporary()?;
                let property = self.temporary()?;
                self.expression(object)?;
                self.slot("put_loc", base, 1, 0, span);
                self.expression(key)?;
                self.slot("put_loc", property, 1, 0, span);
                Ok(Some(PreparedReference::Property(base, property)))
            }
            ir::Pattern::SuperMember { key } => {
                let (receiver, base, key) = self.super_reference(key, false, span)?;
                Ok(Some(PreparedReference::Super(receiver, base, key)))
            }
            ir::Pattern::PrivateMember { object, id } => {
                let base = self.private_reference(object, span)?;
                Ok(Some(PreparedReference::Private(base, *id)))
            }
            ir::Pattern::Binding {
                binding,
                initialization_target,
            } => {
                let reference = ir::Reference {
                    symbol: Some(binding.symbol),
                    name: binding.name.clone(),
                };
                let initializing = !matches!(
                    binding.kind,
                    ir::DeclarationKind::Var | ir::DeclarationKind::Function
                );
                self.prepare_with_reference(
                    initialization_target.as_ref().unwrap_or(&reference),
                    initializing,
                    span,
                )
            }
            ir::Pattern::Target(reference) => self.prepare_with_reference(reference, false, span),
            _ => Ok(None),
        }
    }
    /// ResolveBinding precedes reading the value: a target a with object can shadow is looked
    /// up now, and its object (or undefined, for the plain binding) waits in a temporary.
    fn prepare_with_reference(
        &mut self,
        r: &ir::Reference,
        init: bool,
        span: ir::Span,
    ) -> Result<Option<PreparedReference>, String> {
        let lexical_init = init
            && r.symbol
                .and_then(|id| self.env.get(&id))
                .is_some_and(|v| v.native_lexical);
        if lexical_init || !self.with_applies(r) {
            return Ok(None);
        }
        let base = self.temporary()?;
        let done = self
            .with_tests(r, "with_make_ref", 1, 0, (1, 2), span)?
            .ok_or("with reference without a with test")?;
        self.simple("undefined", 0, 1, span);
        self.atom_op("push_atom_value", &r.name, 0, 1, None, span);
        self.mark(done);
        self.simple("drop", 1, 0, span);
        self.slot("put_loc", base, 1, 0, span);
        Ok(Some(PreparedReference::With(base)))
    }
    /// Stores the value on the stack through a prepared with reference: PutValue on the found
    /// object checks that the binding still exists; otherwise the plain binding is stored.
    fn store_prepared_with(
        &mut self,
        r: &ir::Reference,
        init: bool,
        base: u32,
        span: ir::Span,
    ) -> Result<(), String> {
        let plain = self.label();
        let end = self.label();
        self.slot("get_loc", base, 0, 1, span);
        self.simple("dup", 1, 2, span);
        self.simple("is_undefined", 1, 1, span);
        self.jump("if_true", plain, span);
        self.simple("swap", 2, 2, span);
        self.atom_op("push_atom_value", &r.name, 0, 1, None, span);
        self.simple("swap", 2, 2, span);
        self.simple("put_ref_value", 3, 0, span);
        self.jump("goto", end, span);
        self.mark(plain);
        self.simple("drop", 1, 0, span);
        self.store_plain(r, init, span)?;
        self.mark(end);
        self.clear_temporaries(&[base], span);
        Ok(())
    }
    fn pattern(&mut self, pattern: &'a ir::Pattern, span: ir::Span) -> Result<(), String> {
        self.pattern_prepared(pattern, span, None)
    }
    fn pattern_prepared(
        &mut self,
        pattern: &'a ir::Pattern,
        span: ir::Span,
        prepared: Option<PreparedReference>,
    ) -> Result<(), String> {
        self.budget.charge(Counter::Work, 1)?;
        match pattern {
            ir::Pattern::Binding {
                binding,
                initialization_target,
            } => {
                let reference = ir::Reference {
                    symbol: Some(binding.symbol),
                    name: binding.name.clone(),
                };
                let initializing = !matches!(
                    binding.kind,
                    ir::DeclarationKind::Var | ir::DeclarationKind::Function
                );
                let target = initialization_target.as_ref().unwrap_or(&reference);
                if let Some(PreparedReference::With(base)) = prepared {
                    self.store_prepared_with(target, initializing, base, binding.span)?;
                } else {
                    self.store(target, initializing, binding.span)?;
                }
            }
            ir::Pattern::Target(reference) => {
                if let Some(PreparedReference::With(base)) = prepared {
                    self.store_prepared_with(reference, false, base, span)?;
                } else {
                    self.store(reference, false, span)?;
                }
            }
            ir::Pattern::Member { object, key } => {
                let value = self.temporary()?;
                self.slot("put_loc", value, 1, 0, span);
                if let Some(PreparedReference::Property(base, property)) = prepared {
                    self.slot("get_loc", base, 0, 1, span);
                    self.slot("get_loc", property, 0, 1, span);
                    self.slot("get_loc", value, 0, 1, span);
                    self.simple("put_array_el", 3, 0, span);
                    self.clear_temporaries(&[base, property], span);
                } else {
                    self.expression(object)?;
                    self.expression(key)?;
                    self.slot("get_loc", value, 0, 1, span);
                    self.simple("put_array_el", 3, 0, span);
                }
                self.clear_temporaries(&[value], span);
            }
            ir::Pattern::SuperMember { key } => {
                let reference =
                    if let Some(PreparedReference::Super(receiver, base, property)) = prepared {
                        (receiver, base, property)
                    } else {
                        self.super_reference(key, false, span)?
                    };
                self.super_put(reference, span)?;
                self.simple("drop", 1, 0, span);
                self.clear_temporaries(&[reference.0, reference.1, reference.2], span);
            }
            ir::Pattern::PrivateMember { object, id } => {
                let base = if let Some(PreparedReference::Private(base, prepared_id)) = prepared {
                    if prepared_id != *id {
                        return Err("private reference identity mismatch".into());
                    }
                    base
                } else {
                    self.private_reference(object, span)?
                };
                self.private_put(base, *id, span)?;
                self.simple("drop", 1, 0, span);
                self.clear_temporaries(&[base], span);
            }
            ir::Pattern::Default { pattern, value } => {
                let initialized = self.label();
                self.simple("dup", 1, 2, span);
                self.simple("is_undefined", 1, 1, span);
                self.jump("if_false", initialized, span);
                self.simple("drop", 1, 0, span);
                self.expression(value)?;
                self.infer_pattern_name(pattern, value)?;
                self.mark(initialized);
                self.pattern_prepared(pattern, span, prepared)?;
            }
            ir::Pattern::Array { elements, rest } => {
                self.simple("for_of_start", 1, 3, span);
                self.expr_records.push(self.depth - 3);
                for element in elements {
                    let Some(element) = element.as_ref() else {
                        self.instruction("for_of_skip", vec![0], 3, 4, span);
                        self.simple("drop", 1, 0, span);
                        continue;
                    };
                    let reference = self.prepare_pattern_reference(element, span)?;
                    self.instruction("for_of_next", vec![0], 3, 5, span);
                    self.simple("drop", 1, 0, span);
                    self.pattern_prepared(element, span, reference)?;
                }
                if let Some(rest) = rest {
                    let reference = self.prepare_pattern_reference(rest, span)?;
                    let array = self.temporary()?;
                    let index = self.temporary()?;
                    let value = self.temporary()?;
                    self.instruction("array_from", 0u16.to_ne_bytes().to_vec(), 0, 1, span);
                    self.slot("put_loc", array, 1, 0, span);
                    self.constant(|| Constant::Number(0.0), span);
                    self.slot("put_loc", index, 1, 0, span);
                    let next = self.label();
                    let done = self.label();
                    self.mark(next);
                    self.instruction("for_of_next", vec![0], 3, 5, span);
                    self.jump("if_true", done, span);
                    self.slot("put_loc", value, 1, 0, span);
                    self.slot("get_loc", array, 0, 1, span);
                    self.slot("get_loc", index, 0, 1, span);
                    self.slot("get_loc", value, 0, 1, span);
                    self.simple("define_array_el", 3, 2, span);
                    self.simple("drop", 1, 0, span);
                    self.simple("drop", 1, 0, span);
                    self.slot("get_loc", index, 0, 1, span);
                    self.simple("inc", 1, 1, span);
                    self.slot("put_loc", index, 1, 0, span);
                    self.jump("goto", next, span);
                    self.mark(done);
                    self.simple("drop", 1, 0, span);
                    self.simple("iterator_close", 3, 0, span);
                    self.slot("get_loc", array, 0, 1, span);
                    self.pattern_prepared(rest, span, reference)?;
                    self.clear_temporaries(&[array, index, value], span);
                } else {
                    self.simple("iterator_close", 3, 0, span);
                }
                self.expr_records.pop();
            }
            ir::Pattern::Object { properties, rest } => {
                let source = self.temporary()?;
                self.simple("dup", 1, 2, span);
                self.simple("to_object", 1, 1, span);
                self.simple("drop", 1, 0, span);
                self.slot("put_loc", source, 1, 0, span);
                let excluded = if rest.is_some() {
                    let excluded = self.temporary()?;
                    self.simple("object", 0, 1, span);
                    self.simple("null", 0, 1, span);
                    self.simple("set_proto", 2, 1, span);
                    self.slot("put_loc", excluded, 1, 0, span);
                    Some(excluded)
                } else {
                    None
                };
                for property in properties {
                    let key = self.temporary()?;
                    match &property.key {
                        ir::ObjectKey::Static(name) => {
                            self.budget.charge(Counter::Work, name.len())?;
                            self.constant(|| Constant::String(name.clone()), span);
                        }
                        ir::ObjectKey::Computed(expression) => {
                            self.expression(expression)?;
                            self.simple("to_propkey", 1, 1, expression.span);
                        }
                    }
                    self.slot("put_loc", key, 1, 0, span);
                    let reference = self.prepare_pattern_reference(&property.pattern, span)?;
                    if let Some(excluded) = excluded {
                        self.slot("get_loc", excluded, 0, 1, span);
                        self.slot("get_loc", key, 0, 1, span);
                        self.simple("undefined", 0, 1, span);
                        self.simple("define_array_el", 3, 2, span);
                        self.simple("drop", 1, 0, span);
                        self.simple("drop", 1, 0, span);
                    }
                    self.slot("get_loc", source, 0, 1, span);
                    self.slot("get_loc", key, 0, 1, span);
                    self.simple("get_array_el", 2, 1, span);
                    self.pattern_prepared(&property.pattern, span, reference)?;
                    self.clear_temporaries(&[key], span);
                }
                if let Some(rest) = rest {
                    let reference = self.prepare_pattern_reference(rest, span)?;
                    self.simple("object", 0, 1, span);
                    self.slot("get_loc", source, 0, 1, span);
                    self.simple("to_object", 1, 1, span);
                    self.slot(
                        "get_loc",
                        excluded.ok_or("missing rest exclusion record")?,
                        0,
                        1,
                        span,
                    );
                    self.instruction("copy_data_properties", vec![6], 3, 3, span);
                    self.simple("drop", 1, 0, span);
                    self.simple("drop", 1, 0, span);
                    self.pattern_prepared(rest, span, reference)?;
                }
                self.clear_temporaries(&[source], span);
                if let Some(excluded) = excluded {
                    self.clear_temporaries(&[excluded], span);
                }
            }
        }
        Ok(())
    }
    fn iteration_store(&mut self, left: &'a ir::ForTarget, span: ir::Span) -> Result<(), String> {
        let pattern = match left {
            ir::ForTarget::Declaration(declaration) => &declaration.pattern,
            ir::ForTarget::Assignment(pattern) => pattern,
        };
        self.pattern(pattern, span)
    }
    #[allow(clippy::too_many_arguments)]
    fn iteration(
        &mut self,
        left: &'a ir::ForTarget,
        right: &'a ir::Expression,
        body: &'a ir::Statement,
        labels: &[usize],
        of: bool,
        is_await: bool,
        span: ir::Span,
    ) -> Result<(), String> {
        let next = self.label();
        let done = self.label();
        let end = self.label();
        let outside = self.jump_target(end);
        let outer_controls = self.controls.len();
        let mut slots = Vec::new();
        if let ir::ForTarget::Declaration(declaration) = left {
            for binding in self.pattern_bindings(&declaration.pattern)? {
                if matches!(
                    binding.kind,
                    ir::DeclarationKind::Let | ir::DeclarationKind::Const
                ) {
                    let Place::Local(slot) = self.env[&binding.symbol].place else {
                        return Err("iteration lexical binding must occupy a local".into());
                    };
                    slots.push(slot);
                    self.slot("set_loc_uninitialized", slot, 0, 0, binding.span);
                }
            }
        }
        self.push_block(slots);
        self.simple("undefined", 0, 1, span);
        self.slot("put_loc", 0, 1, 0, span);
        if let ir::ForTarget::Declaration(declaration) = left
            && let Some(initializer) = &declaration.initializer
        {
            self.expression(initializer)?;
            self.infer_pattern_name(&declaration.pattern, initializer)?;
            self.iteration_store(left, initializer.span)?;
        }
        self.marker(right.span)?;
        self.expression(right)?;
        self.simple(
            match (of, is_await) {
                (true, true) => "for_await_of_start",
                (true, false) => "for_of_start",
                _ => "for_in_start",
            },
            1,
            if of { 3 } else { 1 },
            right.span,
        );
        self.budget.charge(Counter::Work, 1)?;
        self.unwind.push(Unwind {
            kind: match (of, is_await) {
                (true, true) => UnwindKind::ForAwait,
                (true, false) => UnwindKind::ForOf,
                _ => UnwindKind::ForIn,
            },
            blocks: outside.blocks,
            handlers: outside.handlers,
            controls: outer_controls,
        });
        let continue_target = self.jump_target(next);
        self.budget.charge(Counter::Work, labels.len() + 1)?;
        for &label in labels {
            self.controls[label].continue_target = Some(continue_target);
        }
        self.controls.push(Control {
            name: None,
            break_target: outside,
            continue_target: Some(continue_target),
            unlabeled: true,
        });
        self.mark(next);
        // Every successful step receives a new lexical cell. Closing before
        // stepping also detaches closures created while evaluating the RHS.
        self.budget.charge(
            Counter::Work,
            self.blocks.last().expect("iteration scope").len(),
        )?;
        let scoped = self.blocks.last().expect("iteration scope").clone();
        for slot in scoped {
            self.slot("close_loc", slot, 0, 0, span);
            self.slot("set_loc_uninitialized", slot, 0, 0, span);
        }
        if is_await {
            // The pinned VM recognises this exact step (await before
            // iterator_get_value_done) when unwinding a throwing next().
            self.simple("dup3", 3, 6, span);
            self.simple("drop", 1, 0, span);
            self.instruction("call_method", 0u16.to_ne_bytes().to_vec(), 2, 1, span);
            self.simple("await", 1, 1, span);
            self.simple("iterator_get_value_done", 1, 2, span);
        } else if of {
            self.instruction("for_of_next", vec![0], 3, 5, span);
        } else {
            self.simple("for_in_next", 1, 3, span);
        }
        self.jump("if_true", done, span);
        self.iteration_store(left, span)?;
        self.statement(body)?;
        self.jump("goto", next, span);
        self.mark(done);
        self.simple("drop", 1, 0, span);
        if is_await {
            // iterator_get_value_done cleared the iterator on exhaustion, so
            // this close calls no return() method.
            self.async_iterator_close(span);
        } else if of {
            // Native Next has marked exhaustion with an undefined iterator;
            // close now only frees the private record and calls no user return.
            self.simple("iterator_close", 3, 0, span);
        } else {
            self.simple("drop", 1, 0, span);
        }
        self.controls.pop();
        self.unwind.pop();
        self.end_block(span);
        self.mark(end);
        Ok(())
    }
    fn statement(&mut self, s: &'a ir::Statement) -> Result<(), String> {
        self.statement_with_labels(s, &[])
    }
    fn statement_with_labels(
        &mut self,
        s: &'a ir::Statement,
        labels: &[usize],
    ) -> Result<(), String> {
        let base = std::mem::replace(&mut self.statement_base, self.depth);
        let result = self.statement_at_base(s, labels);
        self.statement_base = base;
        result
    }
    fn statement_at_base(&mut self, s: &'a ir::Statement, labels: &[usize]) -> Result<(), String> {
        self.budget.charge(Counter::Work, 1)?;
        self.marker(s.span)?;
        match &s.kind {
            ir::StatementKind::Empty => {}
            ir::StatementKind::Function { binding, .. } => self.annex_b_copy(binding)?,
            ir::StatementKind::Debugger => {
                // Like jetbunny-debugger-v1 in C: bit 31 of this statement's
                // marker makes it an unconditional breakpoint.
                if self.options.debug_markers
                    && let Some(Node::Instruction(marker)) = self.nodes.last_mut()
                    && marker.name == "debug_stmt"
                {
                    marker.args[3] |= 0x80;
                }
            }
            ir::StatementKind::Expression(e) => {
                self.expression(e)?;
                self.slot("put_loc", 0, 1, 0, s.span);
            }
            ir::StatementKind::Declaration(declarations) => {
                for declaration in declarations {
                    if let Some(is_await) = declaration.using {
                        self.using_declaration(declaration, is_await, s.span)?;
                    } else if let Some(initializer) = &declaration.initializer {
                        self.expression(initializer)?;
                        self.infer_pattern_name(&declaration.pattern, initializer)?;
                        self.pattern(&declaration.pattern, initializer.span)?;
                    } else if !self
                        .pattern_bindings(&declaration.pattern)?
                        .iter()
                        .all(|binding| binding.kind == ir::DeclarationKind::Var)
                    {
                        self.simple("undefined", 0, 1, s.span);
                        self.pattern(&declaration.pattern, s.span)?;
                    }
                }
            }
            ir::StatementKind::Class { binding, class } => {
                self.class_value(class, None)?;
                self.store(
                    &ir::Reference {
                        symbol: Some(binding.symbol),
                        name: binding.name.clone(),
                    },
                    true,
                    binding.span,
                )?;
            }
            ir::StatementKind::Block(body) => self.block(body)?,
            ir::StatementKind::With {
                object,
                body,
                scope,
            } => {
                self.expression(object)?;
                self.simple("to_object", 1, 1, s.span);
                self.push_block(Vec::new());
                let name: Vec<u16> = "<with>".encode_utf16().collect();
                let id = self.auxiliary_local(&name, *scope, 0, s.span)?;
                self.initialize_auxiliary(id, s.span)?;
                self.withs.push((id, *scope, 1, None));
                self.statement(body)?;
                self.withs.pop();
                self.end_block(s.span);
            }
            ir::StatementKind::If {
                test,
                consequent,
                alternate,
            } => {
                self.simple("undefined", 0, 1, s.span);
                self.slot("put_loc", 0, 1, 0, s.span);
                let otherwise = self.label();
                let done = self.label();
                self.expression(test)?;
                self.jump("if_false", otherwise, test.span);
                self.statement(consequent)?;
                self.jump("goto", done, s.span);
                self.mark(otherwise);
                if let Some(s) = alternate {
                    self.statement(s)?;
                }
                self.mark(done);
            }
            ir::StatementKind::While { test, body } => {
                self.simple("undefined", 0, 1, s.span);
                self.slot("put_loc", 0, 1, 0, s.span);
                let begin = self.label();
                let end = self.label();
                self.mark(begin);
                self.marker(test.span)?;
                self.expression(test)?;
                self.jump("if_false", end, test.span);
                self.enter_loop(begin, end, labels)?;
                self.statement(body)?;
                self.controls.pop();
                self.jump("goto", begin, s.span);
                self.mark(end);
            }
            ir::StatementKind::For {
                init,
                test,
                update,
                body,
            } => {
                self.simple("undefined", 0, 1, s.span);
                self.slot("put_loc", 0, 1, 0, s.span);
                let mut slots = Vec::new();
                if let Some(init) = init {
                    if let ir::StatementKind::Declaration(declarations) = &init.kind {
                        for declaration in declarations {
                            for binding in self.pattern_bindings(&declaration.pattern)? {
                                if matches!(
                                    binding.kind,
                                    ir::DeclarationKind::Let
                                        | ir::DeclarationKind::Const
                                        | ir::DeclarationKind::Import
                                ) && let Place::Local(slot) = self.env[&binding.symbol].place
                                {
                                    self.slot("set_loc_uninitialized", slot, 0, 0, binding.span);
                                    slots.push(slot);
                                }
                            }
                        }
                    }
                    self.push_block(slots.clone());
                    self.statement(init)?;
                    // The init expression does not contribute to loop completion.
                    self.simple("undefined", 0, 1, s.span);
                    self.slot("put_loc", 0, 1, 0, s.span);
                }
                if init.is_none() {
                    self.push_block(slots.clone());
                }
                self.clone_iteration_slots(&slots, s.span)?;
                let begin = self.label();
                let next = self.label();
                let end = self.label();
                self.mark(begin);
                if let Some(test) = test {
                    self.marker(test.span)?;
                    self.expression(test)?;
                    self.jump("if_false", end, test.span);
                }
                self.enter_loop(next, end, labels)?;
                self.statement(body)?;
                self.controls.pop();
                self.mark(next);
                self.clone_iteration_slots(&slots, s.span)?;
                if let Some(update) = update {
                    self.marker(update.span)?;
                    self.expression(update)?;
                    self.simple("drop", 1, 0, update.span);
                }
                self.jump("goto", begin, s.span);
                self.mark(end);
                for slot in self.blocks.pop().expect("for block") {
                    self.slot("close_loc", slot, 0, 0, s.span);
                }
            }
            ir::StatementKind::DoWhile { test, body } => {
                self.simple("undefined", 0, 1, s.span);
                self.slot("put_loc", 0, 1, 0, s.span);
                let begin = self.label();
                let condition = self.label();
                let end = self.label();
                self.mark(begin);
                self.enter_loop(condition, end, labels)?;
                self.statement(body)?;
                self.controls.pop();
                self.mark(condition);
                self.marker(test.span)?;
                self.expression(test)?;
                self.jump("if_true", begin, test.span);
                self.mark(end);
            }
            ir::StatementKind::Break
            | ir::StatementKind::Continue
            | ir::StatementKind::BreakLabel(_)
            | ir::StatementKind::ContinueLabel(_) => {
                self.budget.charge(Counter::Work, self.controls.len())?;
                let (name, continuing) = match &s.kind {
                    ir::StatementKind::BreakLabel(name) => (Some(name.as_str()), false),
                    ir::StatementKind::ContinueLabel(name) => (Some(name.as_str()), true),
                    ir::StatementKind::Continue => (None, true),
                    _ => (None, false),
                };
                if let Some(name) = name {
                    let comparisons = self
                        .controls
                        .iter()
                        .filter_map(|control| control.name)
                        .filter(|candidate| candidate.len() == name.len())
                        .fold(0usize, |work, candidate| {
                            work.saturating_add(candidate.len())
                        });
                    self.budget.charge(Counter::Work, comparisons)?;
                }
                let control = self
                    .controls
                    .iter()
                    .rev()
                    .find(|control| {
                        if let Some(name) = name {
                            control.name == Some(name)
                        } else {
                            control.unlabeled && (!continuing || control.continue_target.is_some())
                        }
                    })
                    .ok_or("control transfer has no matching target")?;
                let target = if continuing {
                    control
                        .continue_target
                        .ok_or("continue label does not target a loop")?
                } else {
                    control.break_target
                };
                self.completion(Completion::Branch(target), s.span)?;
            }
            ir::StatementKind::Label { label, body } => {
                let end = self.label();
                let index = self.controls.len();
                self.budget.charge(Counter::Work, labels.len() + 1)?;
                self.controls.push(Control {
                    name: Some(label.as_str()),
                    break_target: self.jump_target(end),
                    continue_target: None,
                    unlabeled: false,
                });
                let mut nested = labels.to_vec();
                nested.push(index);
                self.statement_with_labels(body, &nested)?;
                self.controls.pop();
                self.mark(end);
            }
            ir::StatementKind::ForOf {
                left,
                right,
                body,
                is_await,
                ..
            } => {
                self.iteration(left, right, body, labels, true, *is_await, s.span)?;
            }
            ir::StatementKind::ForIn {
                left, right, body, ..
            } => {
                self.iteration(left, right, body, labels, false, false, s.span)?;
            }
            ir::StatementKind::Switch {
                discriminant,
                cases,
                ..
            } => {
                self.switch(discriminant, cases, s.span)?;
            }
            ir::StatementKind::Throw(value) => {
                self.expression(value)?;
                self.simple("throw", 1, 0, s.span);
            }
            ir::StatementKind::TryFinally {
                body,
                finalizer,
                body_scope,
                ..
            } => {
                let thrown = self.temporary()?;
                let exception_state = if self.options.debug_markers {
                    Some(self.temporary()?)
                } else {
                    None
                };
                let previous_value = self.temporary()?;
                let caught = self.label();
                let end = self.label();
                self.simple("undefined", 0, 1, s.span);
                self.slot("put_loc", 0, 1, 0, s.span);
                self.handler(caught, s.span, false)?;
                self.unwind.push(Unwind {
                    kind: UnwindKind::Finally(finalizer),
                    blocks: self.blocks.len(),
                    handlers: self.handler_depth,
                    controls: self.controls.len(),
                });
                self.handler_depth += 1;
                self.block(body)?;
                let cleanup = self.finalizer_cleanup(body, *body_scope)?;
                self.handler_depth -= 1;
                self.unwind.pop();
                self.simple("drop", 1, 0, s.span);
                self.slot("get_loc", 0, 0, 1, s.span);
                self.slot("put_loc", previous_value, 1, 0, s.span);
                self.block(finalizer)?;
                self.slot("get_loc", previous_value, 0, 1, s.span);
                self.slot("put_loc", 0, 1, 0, s.span);
                self.jump("goto", end, s.span);
                self.mark(caught);
                for slot in cleanup {
                    self.slot("close_loc", slot, 0, 0, s.span);
                }
                self.slot("put_loc", thrown, 1, 0, s.span);
                if let Some(state) = exception_state {
                    let mut args = self.atom_units(&[]).to_ne_bytes().to_vec();
                    args.push(6);
                    self.instruction("strict_op", args, 0, 1, s.span);
                    self.slot("put_loc", state, 1, 0, s.span);
                }
                self.block(finalizer)?;
                self.slot("get_loc", thrown, 0, 1, s.span);
                if let Some(state) = exception_state {
                    // A compiler-generated finally continuation propagates the
                    // existing exception; it is not another user throw event.
                    let mut args = self.atom_units(&[]).to_ne_bytes().to_vec();
                    args.push(5);
                    self.slot("get_loc", state, 0, 1, s.span);
                    self.instruction("strict_op", args, 2, 0, s.span);
                    if let Some(Node::Instruction(op)) = self.nodes.last_mut() {
                        op.terminal = true;
                    }
                } else {
                    self.simple("throw", 1, 0, s.span);
                }
                self.mark(end);
            }
            ir::StatementKind::TryCatch {
                body,
                binding,
                handler,
                body_scope,
                ..
            } => {
                let caught = self.label();
                let end = self.label();
                self.simple("undefined", 0, 1, s.span);
                self.slot("put_loc", 0, 1, 0, s.span);
                self.handler(caught, s.span, true)?;
                self.handler_depth += 1;
                self.block(body)?;
                let cleanup = self.scope_slots(*body_scope)?;
                self.handler_depth -= 1;
                self.simple("drop", 1, 0, s.span);
                self.jump("goto", end, s.span);
                self.mark(caught);
                // Native unwinding removes stack values, not lexical references.
                // These reserved descendant slots may have been entered before
                // any implicit or explicit exception in the protected block.
                for slot in cleanup {
                    self.slot("close_loc", slot, 0, 0, s.span);
                }
                let mut catch_slots = Vec::new();
                if let Some(pattern) = binding {
                    if let ir::Pattern::Binding {
                        binding,
                        initialization_target: None,
                    } = pattern
                        && binding.kind == ir::DeclarationKind::Catch
                    {
                        let Place::Local(slot) = self.env[&binding.symbol].place else {
                            return Err("catch binding must occupy a scoped local".into());
                        };
                        self.slot("put_loc", slot, 1, 0, binding.span);
                        catch_slots.push(slot);
                        self.push_block(catch_slots);
                    } else {
                        let exception = self.temporary()?;
                        self.slot("put_loc", exception, 1, 0, s.span);
                        for binding in self.pattern_bindings(pattern)? {
                            let Place::Local(slot) = self.env[&binding.symbol].place else {
                                return Err("catch binding must occupy a scoped local".into());
                            };
                            catch_slots.push(slot);
                            self.slot("set_loc_uninitialized", slot, 0, 0, binding.span);
                        }
                        self.push_block(catch_slots);
                        self.slot("get_loc", exception, 0, 1, s.span);
                        self.pattern(pattern, s.span)?;
                        self.clear_temporaries(&[exception], s.span);
                    }
                } else {
                    self.simple("drop", 1, 0, s.span);
                    self.push_block(catch_slots);
                }
                self.simple("undefined", 0, 1, s.span);
                self.slot("put_loc", 0, 1, 0, s.span);
                self.block(handler)?;
                for slot in self.blocks.pop().expect("catch scope") {
                    self.slot("close_loc", slot, 0, 0, s.span);
                }
                self.mark(end);
            }
            ir::StatementKind::Return(value) => {
                let saved = if let Some(value) = value {
                    let slot = self.temporary()?;
                    self.expression(value)?;
                    if self.kind == ir::FunctionKind::AsyncGenerator {
                        // Await before any finally block runs, so a rejection
                        // is thrown inside the try statement.
                        self.simple("await", 1, 1, s.span);
                    }
                    self.slot("put_loc", slot, 1, 0, s.span);
                    Some(slot)
                } else {
                    None
                };
                self.completion(Completion::Return(saved), s.span)?;
            }
        }
        Ok(())
    }
    /// Whether `scope` is `ancestor` or nested inside it.
    fn scope_inside(&self, scope: u32, ancestor: u32) -> bool {
        let mut cursor = Some(scope);
        while let Some(id) = cursor {
            if id == ancestor {
                return true;
            }
            cursor = self
                .program
                .scopes
                .iter()
                .find(|s| s.id == id)
                .and_then(|s| s.parent);
        }
        false
    }
    /// Whether `r` binds inside scope object `index`, so that object cannot shadow it.
    fn with_skips(&self, r: &ir::Reference, index: usize) -> bool {
        let (_, scope, _, rank) = self.withs[index];
        let Some(id) = r.symbol else {
            return false;
        };
        let Some(v) = self.env.get(&id) else {
            return false;
        };
        if matches!(v.place, Place::Global) {
            return false;
        }
        match (rank, self.external_ranks.get(&id)) {
            (Some(object), Some(&binding)) => binding < object,
            _ => self.scope_inside(v.binding.scope, scope),
        }
    }
    /// Whether any enclosing with/var object can shadow `r`.
    fn with_applies(&self, r: &ir::Reference) -> bool {
        (0..self.withs.len()).any(|index| !self.with_skips(r, index))
    }
    /// Emits the with/var-object tests for `r`; returns the join label if any test was emitted.
    fn with_tests(
        &mut self,
        r: &ir::Reference,
        op: &'static str,
        pop: usize,
        push: usize,
        taken: (usize, usize),
        span: ir::Span,
    ) -> Result<Option<u32>, String> {
        let mut join = None;
        for index in (0..self.withs.len()).rev() {
            let (aux, _, flag, _) = self.withs[index];
            if self.with_skips(r, index) {
                continue;
            }
            self.read_auxiliary(aux, span)?;
            let label = match join {
                Some(label) => label,
                None => {
                    let label = self.label();
                    join = Some(label);
                    label
                }
            };
            let units: Vec<u16> = r.name.encode_utf16().collect();
            self.with_jump(op, &units, flag, label, pop, push, taken, span);
        }
        Ok(join)
    }
    fn read(&mut self, r: &ir::Reference, typeof_mode: bool, span: ir::Span) -> Result<(), String> {
        let done = self.with_tests(r, "with_get_var", 1, 0, (1, 1), span)?;
        self.read_plain(r, typeof_mode, span)?;
        if let Some(label) = done {
            self.mark(label);
        }
        Ok(())
    }
    fn read_plain(
        &mut self,
        r: &ir::Reference,
        typeof_mode: bool,
        span: ir::Span,
    ) -> Result<(), String> {
        self.budget.charge(Counter::Work, 1)?;
        if let Some(v) = r.symbol.and_then(|id| self.env.get(&id)) {
            self.budget.charge(Counter::Work, v.binding.name.len())?;
        }
        let Some(v) = r.symbol.and_then(|id| self.env.get(&id)).cloned() else {
            if r.name == "arguments"
                && !self.is_arrow
                && (self.eval_root || self.scope != self.program.scope)
            {
                return Err("arguments object emission not implemented".into());
            }
            self.atom_op(
                if typeof_mode {
                    "get_var_undef"
                } else {
                    "get_var"
                },
                &r.name,
                0,
                1,
                None,
                span,
            );
            return Ok(());
        };
        let checked = v.native_lexical;
        match v.place {
            Place::Global => self.atom_op("get_var", &v.binding.name, 0, 1, None, span),
            Place::Local(n) => self.slot(
                if checked { "get_loc_check" } else { "get_loc" },
                n,
                0,
                1,
                span,
            ),
            Place::Argument(n) => self.slot("get_arg", n, 0, 1, span),
            Place::Capture(n) => self.slot(
                if checked {
                    "get_var_ref_check"
                } else {
                    "get_var_ref"
                },
                n,
                0,
                1,
                span,
            ),
        }
        Ok(())
    }
    fn store(&mut self, r: &ir::Reference, init: bool, span: ir::Span) -> Result<(), String> {
        let lexical_init = init
            && r.symbol
                .and_then(|id| self.env.get(&id))
                .is_some_and(|v| v.native_lexical);
        let done = if lexical_init {
            None
        } else {
            self.with_tests(r, "with_put_var", 2, 1, (2, 0), span)?
        };
        self.store_plain(r, init, span)?;
        if let Some(label) = done {
            self.mark(label);
        }
        Ok(())
    }
    fn store_plain(&mut self, r: &ir::Reference, init: bool, span: ir::Span) -> Result<(), String> {
        self.budget.charge(Counter::Work, 1)?;
        if let Some(v) = r.symbol.and_then(|id| self.env.get(&id)) {
            self.budget.charge(Counter::Work, v.binding.name.len())?;
        }
        let Some(v) = r.symbol.and_then(|id| self.env.get(&id)).cloned() else {
            self.atom_op("put_var", &r.name, 1, 0, None, span);
            return Ok(());
        };
        if !init && v.binding.kind == ir::DeclarationKind::FunctionName {
            if self.effective_strict() {
                self.atom_op("throw_error", &v.binding.name, 0, 0, Some(0), span);
            } else {
                self.simple("drop", 1, 0, span);
            }
            return Ok(());
        }
        let lexical = v.native_lexical;
        if !init
            && matches!(
                v.binding.kind,
                ir::DeclarationKind::Const | ir::DeclarationKind::Import
            )
            && !matches!(v.place, Place::Global)
        {
            self.atom_op("throw_error", &v.binding.name, 0, 0, Some(0), span);
            return Ok(());
        }
        match v.place {
            Place::Global => self.atom_op(
                if init && lexical {
                    "put_var_init"
                } else {
                    "put_var"
                },
                &v.binding.name,
                1,
                0,
                None,
                span,
            ),
            Place::Local(n) => self.slot(
                if lexical {
                    if init {
                        "put_loc_check_init"
                    } else {
                        "put_loc_check"
                    }
                } else {
                    "put_loc"
                },
                n,
                1,
                0,
                span,
            ),
            Place::Argument(n) => self.slot("put_arg", n, 1, 0, span),
            Place::Capture(n) => self.slot(
                if lexical {
                    if init {
                        "put_var_ref_check_init"
                    } else {
                        "put_var_ref_check"
                    }
                } else {
                    "put_var_ref"
                },
                n,
                1,
                0,
                span,
            ),
        }
        Ok(())
    }
    fn function_constant(&mut self, f: &'a ir::Function, force_home: bool) -> Result<u32, String> {
        self.budget.charge(Counter::Work, 1)?;
        self.budget.charge(Counter::Constants, 1)?;
        self.budget.charge(Counter::Work, self.env.len())?;
        for value in self.env.values() {
            self.budget
                .charge(Counter::Work, value.binding.name.len())?;
        }
        let outer = if self.initializing_parameters {
            let active: std::collections::BTreeSet<_> =
                self.blocks.iter().flatten().copied().collect();
            self.env
                .iter()
                .filter(|(_, variable)| match variable.place {
                    Place::Local(slot) => {
                        let definition =
                            &self.function.locals[(self.function.arg_count + slot) as usize];
                        if definition.scope_level > 0 {
                            active.contains(&slot)
                        } else {
                            matches!(variable.native_kind, 4 | image::VAR_KIND_ARGUMENTS)
                                || matches!(
                                    variable.binding.name.as_str(),
                                    "this"
                                        | "new.target"
                                        | "<home_object>"
                                        | "this.active_func"
                                        | "<arg_var>"
                                )
                        }
                    }
                    Place::Argument(_) => false,
                    Place::Capture(_) | Place::Global => true,
                })
                .map(|(symbol, variable)| (*symbol, variable.clone()))
                .collect()
        } else {
            self.env.clone()
        };
        let mut emitter = Self::new(
            self.program,
            Some(f),
            outer,
            self.source_map.clone(),
            self.budget.clone(),
        )?;
        emitter.options = self.options;
        self.inherit_auxiliary(&mut emitter)?;
        if force_home {
            emitter.uses_home_object = true;
            emitter.function.flags.need_home_object = true;
        }
        let (image, mut spans) = emitter.compile(&f.body)?;
        let n = self.function.constants.len() as u32;
        for span in &mut spans {
            span.path.insert(0, n);
        }

        self.budget
            .charge(Counter::Work, spans.iter().map(|s| s.path.len() + 1).sum())?;
        self.child_spans.extend(spans);
        self.function
            .constants
            .push(Constant::Function(Box::new(image)));
        Ok(n)
    }
    fn function_value(&mut self, function: &'a ir::Function, span: ir::Span) -> Result<(), String> {
        let index = self.function_constant(function, false)?;
        self.instruction("fclosure", index.to_ne_bytes().to_vec(), 0, 1, span);
        Ok(())
    }
    fn temporary(&mut self) -> Result<u32, String> {
        self.budget.charge(Counter::Work, 1)?;
        self.budget
            .slots(self.function.arg_count as usize + self.function.local_count as usize + 1)?;
        let slot = self.function.local_count;
        self.function.local_count += 1;
        self.function.locals.push(VarDef::default());
        if self.initializing_parameters {
            self.parameter_region_locals.insert(slot);
        }
        Ok(slot)
    }
    fn clear_temporaries(&mut self, slots: &[u32], span: ir::Span) {
        for &slot in slots {
            self.simple("undefined", 0, 1, span);
            self.slot("put_loc", slot, 1, 0, span);
        }
    }
    // Compound/update/logical references normalize exactly once before GetValue.
    // Simple assignment intentionally keeps native late coercion after its RHS.
    fn property_reference(
        &mut self,
        object: &'a ir::Expression,
        key: &'a ir::Expression,
        span: ir::Span,
    ) -> Result<(u32, u32), String> {
        let base = self.temporary()?;
        let property = self.temporary()?;
        self.expression(object)?;
        self.slot("put_loc", base, 1, 0, span);
        self.expression(key)?;
        self.simple("to_propkey", 1, 1, key.span);
        self.slot("put_loc", property, 1, 0, span);
        Ok((base, property))
    }
    fn property_value(&mut self, base: u32, key: u32, span: ir::Span) {
        self.slot("get_loc", base, 0, 1, span);
        self.slot("get_loc", key, 0, 1, span);
        self.simple("get_array_el", 2, 1, span);
    }
    // Preserve the assigned value independently of setter return values.
    fn property_store(&mut self, base: u32, key: u32, span: ir::Span) -> Result<(), String> {
        let value = self.temporary()?;
        self.simple("dup", 1, 2, span);
        self.slot("put_loc", value, 1, 0, span);
        self.slot("get_loc", base, 0, 1, span);
        self.slot("get_loc", key, 0, 1, span);
        self.slot("get_loc", value, 0, 1, span);
        self.simple("put_array_el", 3, 0, span);
        self.clear_temporaries(&[value], span);
        Ok(())
    }
    fn logical_skip(&mut self, op: ir::Logical, end: u32, span: ir::Span) {
        self.simple("dup", 1, 2, span);
        let branch = match op {
            ir::Logical::And => "if_false",
            ir::Logical::Or => "if_true",
            ir::Logical::Nullish => {
                self.simple("is_undefined_or_null", 1, 1, span);
                "if_false"
            }
        };
        self.jump(branch, end, span);
    }
    // Return the receiver and value separately, including across a parenthesized
    // chain boundary. Calls consume the reference; ordinary expressions do not.
    fn reference_value(&mut self, expression: &'a ir::Expression) -> Result<(), String> {
        self.budget.charge(Counter::Work, 1)?;
        match &expression.kind {
            ir::ExpressionKind::Member { object, key } => {
                self.expression(object)?;
                self.expression(key)?;
                self.simple("get_array_el2", 2, 2, expression.span);
            }
            ir::ExpressionKind::SuperMember { key } => {
                let reference = self.super_reference(key, false, expression.span)?;
                self.slot("get_loc", reference.0, 0, 1, expression.span);
                self.super_get(reference, expression.span)?;
                self.clear_temporaries(&[reference.0, reference.1, reference.2], expression.span);
            }
            ir::ExpressionKind::PrivateMember { object, id } => {
                let base = self.private_reference(object, expression.span)?;
                self.slot("get_loc", base, 0, 1, expression.span);
                self.private_get(base, *id, expression.span)?;
                self.clear_temporaries(&[base], expression.span);
            }
            ir::ExpressionKind::Chain { base, segments } => {
                self.chain(base, segments, false, true, expression.span)?;
            }
            _ => {
                self.simple("undefined", 0, 1, expression.span);
                self.expression(expression)?;
            }
        }
        Ok(())
    }
    fn chain(
        &mut self,
        base: &'a ir::Expression,
        segments: &'a [ir::ChainSegment],
        delete: bool,
        reference: bool,
        span: ir::Span,
    ) -> Result<(), String> {
        let receiver = self.temporary()?;
        let value = self.temporary()?;
        let short = self.label();
        let end = self.label();
        self.reference_value(base)?;
        self.slot("put_loc", value, 1, 0, span);
        self.slot("put_loc", receiver, 1, 0, span);
        for (index, segment) in segments.iter().enumerate() {
            self.budget.charge(Counter::Work, 1)?;
            let (optional, site) = match segment {
                ir::ChainSegment::Member { optional, span, .. }
                | ir::ChainSegment::PrivateMember { optional, span, .. }
                | ir::ChainSegment::Call { optional, span, .. } => (*optional, *span),
            };
            if optional {
                self.slot("get_loc", value, 0, 1, site);
                self.simple("is_undefined_or_null", 1, 1, site);
                self.jump("if_true", short, site);
            }
            match segment {
                ir::ChainSegment::Member { key, .. } => {
                    self.slot("get_loc", value, 0, 1, site);
                    self.expression(key)?;
                    if delete && index + 1 == segments.len() {
                        self.simple("delete", 2, 1, site);
                        self.slot("put_loc", value, 1, 0, site);
                    } else {
                        self.simple("get_array_el2", 2, 2, site);
                        self.slot("put_loc", value, 1, 0, site);
                        self.slot("put_loc", receiver, 1, 0, site);
                    }
                }
                ir::ChainSegment::PrivateMember { id, .. } => {
                    if delete && index + 1 == segments.len() {
                        return Err("private fields cannot be deleted".into());
                    }
                    self.slot("get_loc", value, 0, 1, site);
                    self.slot("put_loc", receiver, 1, 0, site);
                    self.private_get(value, *id, site)?;
                    self.slot("put_loc", value, 1, 0, site);
                }
                ir::ChainSegment::Call { arguments, .. } => {
                    self.slot("get_loc", receiver, 0, 1, site);
                    self.slot("get_loc", value, 0, 1, site);
                    self.emit_call_arguments(arguments, false, true, site)?;
                    self.slot("put_loc", value, 1, 0, site);
                    self.simple("undefined", 0, 1, site);
                    self.slot("put_loc", receiver, 1, 0, site);
                }
            }
        }
        if delete && !matches!(segments.last(), Some(ir::ChainSegment::Member { .. })) {
            self.simple("push_true", 0, 1, span);
            self.slot("put_loc", value, 1, 0, span);
        }
        self.jump("goto", end, span);
        self.mark(short);
        self.simple(if delete { "push_true" } else { "undefined" }, 0, 1, span);
        self.slot("put_loc", value, 1, 0, span);
        self.simple("undefined", 0, 1, span);
        self.slot("put_loc", receiver, 1, 0, span);
        self.mark(end);
        if reference {
            self.slot("get_loc", receiver, 0, 1, span);
        }
        self.slot("get_loc", value, 0, 1, span);
        self.clear_temporaries(&[receiver, value], span);
        Ok(())
    }
    fn binary_opcode(op: ir::Binary) -> &'static str {
        match op {
            ir::Binary::Add => "add",
            ir::Binary::Subtract => "sub",
            ir::Binary::Multiply => "mul",
            ir::Binary::Divide => "div",
            ir::Binary::Remainder => "mod",
            ir::Binary::Exponent => "pow",
            ir::Binary::Equal => "eq",
            ir::Binary::NotEqual => "neq",
            ir::Binary::StrictEqual => "strict_eq",
            ir::Binary::StrictNotEqual => "strict_neq",
            ir::Binary::Less => "lt",
            ir::Binary::LessEqual => "lte",
            ir::Binary::Greater => "gt",
            ir::Binary::GreaterEqual => "gte",
            ir::Binary::BitAnd => "and",
            ir::Binary::BitOr => "or",
            ir::Binary::BitXor => "xor",
            ir::Binary::ShiftLeft => "shl",
            ir::Binary::ShiftRight => "sar",
            ir::Binary::ShiftUnsigned => "shr",
            ir::Binary::In => "in",
            ir::Binary::Instanceof => "instanceof",
        }
    }
    fn object_property(
        &mut self,
        key: &'a ir::ObjectKey,
        value: &'a ir::Expression,
        method: Option<u8>,
        infer_name: bool,
        span: ir::Span,
    ) -> Result<(), String> {
        // Computed property names become keys before the value is evaluated.
        if let ir::ObjectKey::Computed(key) = key {
            self.expression(key)?;
            self.simple("to_propkey", 1, 1, key.span);
        }
        let inferred_name = if infer_name && method.is_none() {
            let slot = self.temporary()?;
            match key {
                ir::ObjectKey::Static(name) => {
                    self.budget.charge(Counter::Work, name.len())?;
                    self.constant(|| Constant::String(name.clone()), value.span);
                }
                ir::ObjectKey::Computed(_) => self.simple("dup", 1, 2, value.span),
            }
            self.slot("put_loc", slot, 1, 0, value.span);
            Some(slot)
        } else {
            None
        };
        self.expression_named(value, inferred_name)?;
        if let Some(slot) = inferred_name {
            self.clear_temporaries(&[slot], value.span);
        }
        match (key, method) {
            (ir::ObjectKey::Static(name), Some(flags)) => {
                let atom = self.atom_units(name);
                let mut operands = atom.to_ne_bytes().to_vec();
                operands.push(flags);
                self.instruction("define_method", operands, 2, 1, span);
            }
            (ir::ObjectKey::Computed(_), Some(flags)) => {
                self.instruction("define_method_computed", vec![flags], 3, 1, span);
            }
            (ir::ObjectKey::Static(name), None) => {
                let atom = self.atom_units(name);
                self.instruction("define_field", atom.to_ne_bytes().to_vec(), 2, 1, span);
            }
            (ir::ObjectKey::Computed(_), None) => {
                self.simple("define_array_el", 3, 2, span);
                self.simple("drop", 1, 0, span);
            }
        }
        Ok(())
    }
    fn expression(&mut self, e: &'a ir::Expression) -> Result<(), String> {
        self.budget.charge(Counter::Work, 1)?;
        match &e.kind {
            ir::ExpressionKind::Template {
                parts,
                substitutions,
            } => {
                if parts.len() != substitutions.len() + 1 {
                    return Err("template part arity".into());
                }
                self.budget
                    .charge(Counter::Work, parts.iter().map(|p| p.cooked.len()).sum())?;
                self.constant(|| Constant::String(parts[0].cooked.clone()), e.span);
                for (i, substitution) in substitutions.iter().enumerate() {
                    self.expression(substitution)?;
                    self.simple("to_propkey", 1, 1, substitution.span);
                    self.simple("add", 2, 1, e.span);
                    self.constant(|| Constant::String(parts[i + 1].cooked.clone()), e.span);
                    self.simple("add", 2, 1, e.span);
                }
            }
            ir::ExpressionKind::Sequence(expressions) => {
                for (i, expression) in expressions.iter().enumerate() {
                    self.expression(expression)?;
                    if i + 1 < expressions.len() {
                        self.simple("drop", 1, 0, expression.span);
                    }
                }
            }
            ir::ExpressionKind::Conditional {
                test,
                consequent,
                alternate,
            } => {
                let otherwise = self.label();
                let done = self.label();
                self.expression(test)?;
                self.jump("if_false", otherwise, test.span);
                self.expression(consequent)?;
                self.jump("goto", done, e.span);
                self.mark(otherwise);
                self.expression(alternate)?;
                self.mark(done);
            }
            ir::ExpressionKind::Object(properties) => {
                self.simple("object", 0, 1, e.span);
                for property in properties {
                    self.budget.charge(Counter::Work, 1)?;
                    match property {
                        ir::ObjectEntry::Data {
                            key,
                            value,
                            method,
                            infer_name,
                        } => {
                            self.object_property(
                                key,
                                value,
                                (*method).then_some(4),
                                *infer_name,
                                e.span,
                            )?;
                        }
                        ir::ObjectEntry::Getter { key, value } => {
                            self.object_property(key, value, Some(5), false, e.span)?;
                        }
                        ir::ObjectEntry::Setter { key, value } => {
                            self.object_property(key, value, Some(6), false, e.span)?;
                        }
                        ir::ObjectEntry::Prototype { value } => {
                            self.expression(value)?;
                            self.simple("set_proto", 2, 1, value.span);
                        }
                        ir::ObjectEntry::Spread { value } => {
                            self.expression(value)?;
                            let nullable = self.label();
                            self.simple("dup", 1, 2, value.span);
                            self.simple("is_undefined_or_null", 1, 1, value.span);
                            self.jump("if_true", nullable, value.span);
                            self.simple("to_object", 1, 1, value.span);
                            self.mark(nullable);
                            self.simple("null", 0, 1, value.span);
                            self.instruction("copy_data_properties", vec![6], 3, 3, value.span);
                            self.simple("drop", 1, 0, value.span);
                            self.simple("drop", 1, 0, value.span);
                        }
                    }
                }
            }
            ir::ExpressionKind::Array(elements) => {
                for element in elements {
                    self.expression(element)?;
                }
                let n = u16::try_from(elements.len()).map_err(|_| "capacity: array size budget")?;
                self.instruction(
                    "array_from",
                    n.to_ne_bytes().to_vec(),
                    elements.len(),
                    1,
                    e.span,
                );
            }
            ir::ExpressionKind::Chain { base, segments } => {
                self.chain(base, segments, false, false, e.span)?;
            }
            ir::ExpressionKind::Member { object, key } => {
                self.expression(object)?;
                self.expression(key)?;
                self.simple("get_array_el", 2, 1, e.span);
            }
            ir::ExpressionKind::Literal(v) => match v {
                ir::Literal::BigInt(bytes) => {
                    self.budget.charge(Counter::Work, bytes.len())?;
                    self.constant(|| Constant::BigInt(bytes.clone()), e.span)
                }
                ir::Literal::Number(v) => self.constant(|| Constant::Number(*v), e.span),
                ir::Literal::String(v) => {
                    self.budget.charge(Counter::Work, v.len())?;
                    self.constant(|| Constant::String(v.clone()), e.span)
                }
                ir::Literal::Bool(v) => {
                    self.simple(if *v { "push_true" } else { "push_false" }, 0, 1, e.span)
                }
                ir::Literal::Null => self.simple("null", 0, 1, e.span),
                ir::Literal::Undefined => self.simple("undefined", 0, 1, e.span),
            },
            ir::ExpressionKind::Read(r) => self.read(r, false, e.span)?,
            ir::ExpressionKind::InvalidCallTarget(call) => {
                self.expression(call)?;
                self.simple("drop", 1, 0, e.span);
                // JS_THROW_ERROR_INVALID_LVALUE; the unreachable value keeps the stack shape.
                self.atom_op("throw_error", "", 0, 0, Some(5), e.span);
                self.simple("undefined", 0, 1, e.span);
            }
            ir::ExpressionKind::DestructureAssign { pattern, value } => {
                let saved = self.temporary()?;
                self.expression(value)?;
                self.simple("dup", 1, 2, e.span);
                self.slot("put_loc", saved, 1, 0, e.span);
                self.pattern(pattern, e.span)?;
                self.slot("get_loc", saved, 0, 1, e.span);
                self.clear_temporaries(&[saved], e.span);
            }
            ir::ExpressionKind::SuperMember { key } => {
                let reference = self.super_reference(key, false, e.span)?;
                self.super_get(reference, e.span)?;
                self.clear_temporaries(&[reference.0, reference.1, reference.2], e.span);
            }
            ir::ExpressionKind::AssignSuper { key, value, op } => {
                self.emit_super_assign(key, op, value, e.span)?
            }
            ir::ExpressionKind::UpdateSuper {
                key,
                increment,
                prefix,
            } => self.emit_super_update(key, *increment, *prefix, e.span)?,
            ir::ExpressionKind::PrivateMember { object, id } => {
                let base = self.private_reference(object, e.span)?;
                self.private_get(base, *id, e.span)?;
                self.clear_temporaries(&[base], e.span);
            }
            ir::ExpressionKind::AssignPrivate {
                object,
                id,
                value,
                op,
            } => self.emit_private_assign(object, *id, op, value, e.span)?,
            ir::ExpressionKind::UpdatePrivate {
                object,
                id,
                increment,
                prefix,
            } => self.emit_private_update(object, *id, *increment, *prefix, e.span)?,
            ir::ExpressionKind::PrivateIn { object, id } => self.private_in(object, *id, e.span)?,
            ir::ExpressionKind::NamedEvaluation { name, value } => {
                self.named_evaluation(name, value, e.span)?
            }
            ir::ExpressionKind::SuperCall { arguments } => {
                let constructor = self
                    .super_constructor_binding
                    .clone()
                    .ok_or("super call lacks a lexical constructor owner")?;
                let new_target = self
                    .new_target_binding
                    .clone()
                    .ok_or("super call lacks lexical new.target")?;
                self.read(
                    &ir::Reference {
                        symbol: Some(constructor.symbol),
                        name: constructor.name.clone(),
                    },
                    false,
                    e.span,
                )?;
                self.simple("get_super", 1, 1, e.span);
                self.read(
                    &ir::Reference {
                        symbol: Some(new_target.symbol),
                        name: new_target.name.clone(),
                    },
                    false,
                    e.span,
                )?;
                self.emit_call_arguments(arguments, true, false, e.span)?;
                self.simple("dup", 1, 2, e.span);
                let this = self
                    .this_binding
                    .clone()
                    .ok_or("super call lacks lexical this")?;
                self.store(
                    &ir::Reference {
                        symbol: Some(this.symbol),
                        name: this.name.clone(),
                    },
                    true,
                    e.span,
                )?;
                self.initialize_instance_fields(e.span)?;
            }
            ir::ExpressionKind::Class(class) => self.class_value(class, None)?,
            ir::ExpressionKind::NewTarget => {
                if self.eval_root {
                    return Err("eval new.target requires a proven materialized binding".into());
                }
                let binding = self
                    .new_target_binding
                    .clone()
                    .ok_or("new.target has no lexical owner")?;
                self.read(
                    &ir::Reference {
                        symbol: Some(binding.symbol),
                        name: binding.name.clone(),
                    },
                    false,
                    e.span,
                )?;
            }
            ir::ExpressionKind::This => {
                if self.eval_root {
                    return Err("eval this requires a proven materialized binding".into());
                }
                if let Some(binding) = self.this_binding.clone() {
                    self.read(
                        &ir::Reference {
                            symbol: Some(binding.symbol),
                            name: binding.name.clone(),
                        },
                        false,
                        e.span,
                    )?;
                } else {
                    self.simple("push_this", 0, 1, e.span)
                }
            }
            ir::ExpressionKind::Function(f) => {
                self.function_value(f, e.span)?;
            }
            ir::ExpressionKind::SpreadArray(elements) => {
                self.spread_array(elements, e.span)?;
            }
            ir::ExpressionKind::RegExp { pattern, flags } => {
                self.constant(|| Constant::String(pattern.clone()), e.span);
                self.constant(|| Constant::String(flags.clone()), e.span);
                // Selector 4: libregexp compiles the literal when it is evaluated.
                let mut arguments = self.atom_units(&[]).to_ne_bytes().to_vec();
                arguments.push(4);
                self.instruction("strict_op", arguments, 2, 1, e.span);
            }
            ir::ExpressionKind::TemplateObject { cooked, raw } => {
                // One constant per site, so every evaluation sees the same object.
                self.constant(
                    || Constant::TemplateObject {
                        cooked: cooked.clone(),
                        raw: raw.clone(),
                    },
                    e.span,
                );
            }
            ir::ExpressionKind::DirectEval { callee, arguments } => {
                self.direct_eval(callee, arguments, e.span)?;
            }
            ir::ExpressionKind::ImportMeta => {
                // OP_SPECIAL_OBJECT_IMPORT_META; throws outside a module.
                self.instruction("special_object", vec![6], 0, 1, e.span);
            }
            ir::ExpressionKind::ImportCall { specifier, options } => {
                self.expression(specifier)?;
                match options {
                    Some(options) => self.expression(options)?,
                    None => self.simple("undefined", 0, 1, e.span),
                }
                self.simple("import", 2, 1, e.span);
            }
            ir::ExpressionKind::Await(argument) => {
                self.expression(argument)?;
                self.simple("await", 1, 1, e.span);
            }
            ir::ExpressionKind::Yield {
                argument,
                delegate: false,
            } => {
                match argument {
                    Some(argument) => self.expression(argument)?,
                    None => self.simple("undefined", 0, 1, e.span),
                }
                if self.kind == ir::FunctionKind::AsyncGenerator {
                    self.simple("await", 1, 1, e.span);
                }
                self.simple("yield", 1, 2, e.span);
                let resumed = self.label();
                self.jump("if_false", resumed, e.span);
                self.yield_return(e.span)?;
                self.mark(resumed);
            }
            ir::ExpressionKind::Yield {
                argument,
                delegate: true,
            } => {
                let argument = argument.as_ref().ok_or("yield* requires an operand")?;
                self.expression(argument)?;
                self.yield_star(e.span)?;
            }
            ir::ExpressionKind::Update {
                target,
                increment,
                prefix,
            } => {
                self.read(target, false, e.span)?;
                self.simple(
                    match (*increment, *prefix) {
                        (true, true) => "inc",
                        (false, true) => "dec",
                        (true, false) => "post_inc",
                        (false, false) => "post_dec",
                    },
                    1,
                    if *prefix { 1 } else { 2 },
                    e.span,
                );
                if *prefix {
                    self.simple("dup", 1, 2, e.span);
                }
                self.store(target, false, e.span)?;
            }
            ir::ExpressionKind::UpdateMember {
                object,
                key,
                increment,
                prefix,
            } => {
                let (base, key) = self.property_reference(object, key, e.span)?;
                self.property_value(base, key, e.span);
                self.simple(
                    match (*increment, *prefix) {
                        (true, true) => "inc",
                        (false, true) => "dec",
                        (true, false) => "post_inc",
                        (false, false) => "post_dec",
                    },
                    1,
                    if *prefix { 1 } else { 2 },
                    e.span,
                );
                self.property_store(base, key, e.span)?;
                if !prefix {
                    self.simple("drop", 1, 0, e.span);
                }
                self.clear_temporaries(&[base, key], e.span);
            }
            ir::ExpressionKind::CompoundAssignMember {
                object,
                key,
                op,
                value,
            } => {
                let (base, key) = self.property_reference(object, key, e.span)?;
                self.property_value(base, key, e.span);
                self.expression(value)?;
                self.simple(Self::binary_opcode(*op), 2, 1, e.span);
                self.property_store(base, key, e.span)?;
                self.clear_temporaries(&[base, key], e.span);
            }
            ir::ExpressionKind::LogicalAssign { target, op, value } => {
                let end = self.label();
                self.read(target, false, e.span)?;
                self.logical_skip(*op, end, e.span);
                self.simple("drop", 1, 0, e.span);
                self.expression(value)?;
                if matches!(&value.kind,ir::ExpressionKind::Function(f)if f.name.is_none()) {
                    self.atom_op("set_name", &target.name, 1, 1, None, value.span);
                }
                self.simple("dup", 1, 2, e.span);
                self.store(target, false, e.span)?;
                self.mark(end);
            }
            ir::ExpressionKind::LogicalAssignMember {
                object,
                key,
                op,
                value,
            } => {
                let (base, key) = self.property_reference(object, key, e.span)?;
                let end = self.label();
                self.property_value(base, key, e.span);
                self.logical_skip(*op, end, e.span);
                self.simple("drop", 1, 0, e.span);
                self.expression(value)?;
                self.property_store(base, key, e.span)?;
                self.mark(end);
                self.clear_temporaries(&[base, key], e.span);
            }
            ir::ExpressionKind::AssignMember { object, key, value } => {
                self.expression(object)?;
                self.expression(key)?;
                self.expression(value)?;
                let slot = self.function.local_count;
                self.budget.slots(
                    self.function.arg_count as usize + self.function.local_count as usize + 1,
                )?;
                self.function.local_count += 1;
                if self.function.local_count > 65535 {
                    return Err("capacity: local count budget".into());
                }
                self.function.locals.push(VarDef::default());
                self.simple("dup", 1, 2, e.span);
                self.slot("put_loc", slot, 1, 0, e.span);
                self.simple("put_array_el", 3, 0, e.span);
                self.slot("get_loc", slot, 0, 1, e.span);
            }
            ir::ExpressionKind::Assign { target, value }
                if let ir::ExpressionKind::Binary { op, left, right } = &value.kind
                    && matches!(&left.kind, ir::ExpressionKind::Read(r) if r.name == target.name && r.symbol == target.symbol)
                    && self.with_applies(target) =>
            {
                // `x op= v` inside `with`: resolve the reference once, before `v`.
                // Stack: [base, old] where base is the with object or undefined.
                let done = self
                    .with_tests(target, "with_get_ref", 1, 0, (1, 2), e.span)?
                    .ok_or("with reference without a with test")?;
                self.simple("undefined", 0, 1, e.span);
                self.read_plain(target, false, left.span)?;
                self.mark(done);
                self.expression(right)?;
                self.simple(Self::binary_opcode(*op), 2, 1, e.span);
                let plain = self.label();
                let end = self.label();
                self.simple("swap", 2, 2, e.span);
                self.simple("dup", 1, 2, e.span);
                self.simple("is_undefined", 1, 1, e.span);
                self.jump("if_true", plain, e.span);
                // PutValue checks that the binding still exists: [new, base, name, new].
                self.simple("swap", 2, 2, e.span);
                self.simple("insert2", 2, 3, e.span);
                self.atom_op("push_atom_value", &target.name, 0, 1, None, e.span);
                self.simple("swap", 2, 2, e.span);
                self.simple("put_ref_value", 3, 0, e.span);
                self.jump("goto", end, e.span);
                self.mark(plain);
                self.simple("drop", 1, 0, e.span);
                self.simple("dup", 1, 2, e.span);
                self.store_plain(target, false, e.span)?;
                self.mark(end);
            }
            ir::ExpressionKind::Assign { target, value } => {
                // Inside `with`, ResolveBinding precedes `v`.
                let prepared = self.prepare_with_reference(target, false, e.span)?;
                self.expression(value)?;
                if matches!(&value.kind,ir::ExpressionKind::Function(f) if f.name.is_none()) {
                    self.atom_op("set_name", &target.name, 1, 1, None, value.span);
                }
                self.simple("dup", 1, 2, e.span);
                if let Some(PreparedReference::With(base)) = prepared {
                    self.store_prepared_with(target, false, base, e.span)?;
                } else {
                    self.store(target, false, e.span)?;
                }
            }
            ir::ExpressionKind::Unary {
                op: ir::Unary::Delete,
                argument,
            } => match &argument.kind {
                ir::ExpressionKind::Read(reference) => {
                    let done =
                        self.with_tests(reference, "with_delete_var", 1, 0, (1, 1), e.span)?;
                    let fixed_binding = reference
                        .symbol
                        .and_then(|symbol| self.env.get(&symbol))
                        .is_some_and(|variable| !matches!(variable.place, Place::Global));
                    if fixed_binding {
                        self.simple("push_false", 0, 1, e.span)
                    } else {
                        // A global var can reuse a configurable property created by
                        // an earlier script; the runtime owns its delete attributes.
                        self.atom_op("delete_var", &reference.name, 0, 1, None, e.span)
                    }
                    if let Some(label) = done {
                        self.mark(label);
                    }
                }
                ir::ExpressionKind::Chain { base, segments } => {
                    self.chain(base, segments, true, false, e.span)?;
                }
                ir::ExpressionKind::SuperMember { key } => {
                    let reference = self.super_reference(key, false, e.span)?;
                    self.clear_temporaries(&[reference.0, reference.1, reference.2], e.span);
                    self.atom_op("throw_error", "", 0, 0, Some(3), e.span);
                }
                ir::ExpressionKind::PrivateMember { .. } => {
                    return Err("private fields cannot be deleted".into());
                }
                ir::ExpressionKind::Member { object, key } => {
                    self.expression(object)?;
                    self.expression(key)?;
                    self.simple("delete", 2, 1, e.span);
                }
                _ => {
                    self.expression(argument)?;
                    self.simple("drop", 1, 0, e.span);
                    self.simple("push_true", 0, 1, e.span);
                }
            },
            ir::ExpressionKind::Unary { op, argument } => {
                if *op == ir::Unary::Typeof
                    && let ir::ExpressionKind::Read(r) = &argument.kind
                {
                    self.read(r, true, argument.span)?;
                } else {
                    self.expression(argument)?;
                }
                match op {
                    ir::Unary::Void => {
                        self.simple("drop", 1, 0, e.span);
                        self.simple("undefined", 0, 1, e.span);
                    }
                    _ => self.simple(
                        match op {
                            ir::Unary::Negate => "neg",
                            ir::Unary::Plus => "plus",
                            ir::Unary::Not => "lnot",
                            ir::Unary::BitNot => "not",
                            ir::Unary::Typeof => "typeof",
                            ir::Unary::Void | ir::Unary::Delete => unreachable!(),
                        },
                        1,
                        1,
                        e.span,
                    ),
                }
            }
            ir::ExpressionKind::Binary { op, left, right } => {
                self.expression(left)?;
                self.expression(right)?;
                self.simple(Self::binary_opcode(*op), 2, 1, e.span);
            }
            ir::ExpressionKind::Logical { op, left, right } => {
                let end = self.label();
                self.expression(left)?;
                self.simple("dup", 1, 2, e.span);
                let branch = match op {
                    ir::Logical::And => "if_false",
                    ir::Logical::Or => "if_true",
                    ir::Logical::Nullish => {
                        self.simple("is_undefined_or_null", 1, 1, e.span);
                        "if_false"
                    }
                };
                self.jump(branch, end, e.span);
                self.simple("drop", 1, 0, e.span);
                self.expression(right)?;
                self.mark(end);
            }
            ir::ExpressionKind::Construct { callee, arguments } => {
                self.expression(callee)?;
                self.simple("dup", 1, 2, callee.span);
                self.emit_call_arguments(arguments, true, false, e.span)?;
            }
            ir::ExpressionKind::Call { callee, arguments } => {
                let with_call = match &callee.kind {
                    ir::ExpressionKind::Read(r) => self
                        .with_tests(r, "with_get_ref", 1, 0, (1, 2), callee.span)?
                        .map(|done| (r, done)),
                    _ => None,
                };
                if let Some((r, done)) = with_call {
                    // A with-object found for the callee becomes `this`.
                    self.simple("undefined", 0, 1, e.span);
                    self.read_plain(r, false, e.span)?;
                    self.mark(done);
                    self.emit_call_arguments(arguments, false, true, e.span)?;
                } else {
                    let method = matches!(
                        &callee.kind,
                        ir::ExpressionKind::Member { .. }
                            | ir::ExpressionKind::Chain { .. }
                            | ir::ExpressionKind::PrivateMember { .. }
                            | ir::ExpressionKind::SuperMember { .. }
                    );
                    if method {
                        self.reference_value(callee)?;
                    } else {
                        self.expression(callee)?;
                    }
                    self.emit_call_arguments(arguments, false, method, e.span)?;
                }
            }
        }
        Ok(())
    }
    fn finish(mut self) -> Result<(FunctionImage, Vec<image::FunctionSpans>), String> {
        self.budget.charge(Counter::Work, 1)?;
        self.prune_captures()?;
        for capture in &self.function.captures {
            self.budget.text(capture.name.len())?;
        }
        // Native scope links use local-slot indexes, excluding the argument prefix.
        let arg_count = self.function.arg_count as usize;
        self.budget
            .charge(Counter::Work, self.program.scopes.len())?;
        self.budget.charge(
            Counter::Work,
            self.env.len().saturating_add(self.function.locals.len()),
        )?;
        let parents: BTreeMap<_, _> = self
            .program
            .scopes
            .iter()
            .map(|s| (s.id, s.parent))
            .collect();
        let mut first = BTreeMap::new();
        self.budget.charge(Counter::Work, self.local_scopes.len())?;
        let source_scopes = self.local_scopes.clone();
        let parameter_head = self.parameter_slots.last().copied();
        let mut previous_parameter = None;
        for slot in 0..self.function.local_count {
            let vardef = &mut self.function.locals[arg_count + slot as usize];
            if self.parameter_profile && vardef.scope_level == 1 {
                vardef.scope_next = previous_parameter
                    .map_or(image::ScopeLink::Parameters, image::ScopeLink::Local);
                previous_parameter = Some(slot);
                first.insert(1, slot);
                continue;
            }
            if (vardef.is_lexical || vardef.var_kind == 3)
                && !(self.derived_constructor
                    && vardef.scope_level == 0
                    && vardef.name == [116u16, 104, 105, 115])
                && source_scopes.get(&slot) == Some(&self.scope)
                && !self.parameter_region_locals.contains(&slot)
            {
                vardef.scope_level = if self.parameter_profile { 2 } else { 1 };
            }
            if vardef.scope_level == 0 && vardef.name.is_empty() && !vardef.is_captured {
                vardef.scope_next = image::ScopeLink::End;
            } else {
                vardef.scope_next = first.insert(vardef.scope_level, slot).into();
            }
        }
        for (&slot, &source_scope) in &source_scopes {
            let vardef = &self.function.locals[arg_count + slot as usize];
            if !(vardef.is_lexical || vardef.var_kind == 3)
                || !vardef.scope_next.is_end()
                || vardef.scope_level <= 1
            {
                continue;
            }
            let mut parent = if source_scope == self.scope {
                None
            } else {
                parents.get(&source_scope).copied().flatten()
            };
            let mut next = None;
            while let Some(scope) = parent {
                self.budget.charge(Counter::Work, 1)?;
                let level = if scope == self.scope {
                    if self.parameter_profile && !self.parameter_region_locals.contains(&slot) {
                        2
                    } else {
                        1
                    }
                } else {
                    scope + 2
                };
                if let Some(&candidate) = first.get(&level) {
                    next = Some(candidate);
                    break;
                }
                if scope == self.scope {
                    break;
                }
                parent = parents.get(&scope).copied().flatten();
            }
            if next.is_none() && self.parameter_profile {
                next = parameter_head;
            }
            self.function.locals[arg_count + slot as usize].scope_next = next.into();
        }
        self.budget.check()?;
        self.budget
            .charge(Counter::Work, self.nodes.len().saturating_mul(2))?;
        let mut labels = BTreeMap::new();
        let mut offsets = Vec::with_capacity(self.nodes.len());
        let mut length = 0usize;
        for (i, n) in self.nodes.iter().enumerate() {
            offsets.push(length);
            match n {
                Node::Label(l) => {
                    labels.insert(*l, (length, i));
                }
                Node::Instruction(op) => {
                    let schema = opcode(op.name)?;
                    if schema.width as usize != op.args.len() + 1 {
                        return Err(format!("wrong width for {}", op.name));
                    }
                    length += 1 + op.args.len();
                }
            }
        }
        let mut depths = vec![None; self.nodes.len()];
        depths[0] = Some(0usize);
        let mut work = VecDeque::from([0usize]);
        let mut max = 0;
        while let Some(i) = work.pop_front() {
            let depth = depths[i].unwrap();
            let mut next = depth;
            let mut successors = Vec::new();
            let mut taken_edge: Option<(usize, usize)> = None;
            match &self.nodes[i] {
                Node::Label(_) => {}
                Node::Instruction(op) => {
                    if depth < op.pop {
                        return Err(format!("stack underflow {}", op.name));
                    }
                    next = depth - op.pop + op.push;
                    max = max.max(next);
                    if op.terminal {
                        continue;
                    }
                    if let Some(target) = op.target {
                        let j = labels.get(&target).ok_or("undefined label")?.1;
                        match op.taken {
                            Some((tp, tu)) => {
                                if depth < tp {
                                    return Err(format!("stack underflow {}", op.name));
                                }
                                let taken_depth = depth - tp + tu;
                                max = max.max(taken_depth);
                                taken_edge = Some((j, taken_depth));
                            }
                            None => successors.push(j),
                        }
                    }
                    if op.name == "goto" {
                        for j in successors {
                            if let Some(old) = depths[j] {
                                if old != next {
                                    return Err("stack join mismatch".into());
                                }
                            } else {
                                depths[j] = Some(next);
                                work.push_back(j);
                            }
                        }
                        continue;
                    }
                }
            }
            if let Some((j, taken_depth)) = taken_edge {
                if let Some(old) = depths[j] {
                    if old != taken_depth {
                        return Err("stack join mismatch".into());
                    }
                } else {
                    depths[j] = Some(taken_depth);
                    work.push_back(j);
                }
            }
            if i + 1 >= self.nodes.len() {
                return Err("fallthrough".into());
            }
            successors.push(i + 1);
            for j in successors {
                if let Some(old) = depths[j] {
                    if old != next {
                        return Err("stack join mismatch".into());
                    }
                } else {
                    depths[j] = Some(next);
                    work.push_back(j);
                }
            }
        }
        if max > 65535 {
            return Err("capacity: native stack budget exceeded".into());
        }
        self.function.stack_size = u32::try_from(max).map_err(|_| "capacity: stack limit")?;
        let mut positions = Vec::new();
        // lunarstorm patch: debug compiles always carry a user-catch table,
        // possibly empty, so the engine can tell "no user catch" from "not
        // recorded" (see `FunctionImage::user_catches`).
        self.function.user_catches = self.options.debug_markers.then(Vec::new);
        for (i, n) in self.nodes.into_iter().enumerate() {
            if let Node::Instruction(mut op) = n {
                self.function.code.push(opcode(op.name)?.id);
                if let Some(label) = op.target {
                    let target = labels[&label].0;
                    if op.user_catch
                        && let Some(table) = &mut self.function.user_catches
                    {
                        table.push(u32::try_from(target).map_err(|_| "capacity: code offset")?);
                    }
                    if op.taken.is_some() {
                        // `with_*`: atom and flag stay; the label operand sits at offset + 5.
                        let displacement = i32::try_from(target as i64 - offsets[i] as i64 - 5)
                            .map_err(|_| "capacity: branch overflow")?;
                        op.args[4..8].copy_from_slice(&displacement.to_ne_bytes());
                    } else {
                        let displacement = i32::try_from(target as i64 - offsets[i] as i64 - 1)
                            .map_err(|_| "capacity: branch overflow")?;
                        op.args[..4].copy_from_slice(&displacement.to_ne_bytes());
                    }
                }
                self.function.code.extend(op.args);
                let (line, column) = self
                    .source_map
                    .line_column(op.span.start)
                    .ok_or("source position outside original file")?;
                self.function.debug_locations.push(image::DebugLocation {
                    code_offset: offsets[i] as u32,
                    line,
                    column,
                });
                positions.push(Position {
                    code_offset: offsets[i] as u32,
                    source_start: op.span.start,
                    source_end: op.span.end,
                });
            }
        }
        // Nested handlers are emitted outer-first, so targets arrive unordered.
        if let Some(table) = &mut self.function.user_catches {
            table.sort_unstable();
        }
        let own = image::FunctionSpans {
            path: Vec::new(),
            function_start: self.function_span.start,
            function_end: self.function_span.end,
            positions,
        };
        let mut functions = vec![own];
        functions.extend(self.child_spans);
        Ok((self.function, functions))
    }
}

/// Compile an ordinary module using the module grammar and two-phase entry ABI.
pub fn compile_module(source: &str, filename: &str) -> Result<image::VerifiedImage, String> {
    let program = qjs_frontend::compile_program(source, filename, qjs_frontend::SourceMode::Module)
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
    let map = std::sync::Arc::new(qjs_frontend::source::SourceLineMap::from_utf16(
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
