//! Emitter construction and its bookkeeping: function regions, `using`
//! disposal, captures and variable definitions, atoms, instructions, labels
//! and jumps, constants, and binding declaration.

use super::*;

impl<'a> Emitter<'a> {
    pub(super) fn new(
        program: &'a ir::Program,
        function: Option<&'a ir::Function>,
        outer: BTreeMap<u32, Variable>,
        source_map: std::sync::Arc<qjs_js_parser_frontend::source::SourceLineMap>,
        budget: Budget,
    ) -> Result<Self, String> {
        Self::new_region(program, function, outer, source_map, budget, None)
    }
    pub(super) fn new_region(
        program: &'a ir::Program,
        function: Option<&'a ir::Function>,
        outer: BTreeMap<u32, Variable>,
        source_map: std::sync::Arc<qjs_js_parser_frontend::source::SourceLineMap>,
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
            assign_member_slot: None,
            free_temporaries: Vec::new(),
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
    pub(super) fn using_declaration(
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
    pub(super) fn dispose_module_usings(&mut self, span: ir::Span) -> Result<(), String> {
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
    pub(super) fn caught_error(&mut self, span: ir::Span) -> Result<(), String> {
        let error = self.temporary()?;
        self.slot("put_loc", error, 1, 0, span);
        self.slot("get_loc", error, 0, 1, span);
        Ok(())
    }
    pub(super) fn dispose_usings(&mut self, usings: &[(u32, bool)], span: ir::Span) -> Result<(), String> {
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
    pub(super) fn capture_variable(variable: &Variable, idx: u32, kind: u8) -> Capture {
        let mut capture = Self::capture(&variable.binding, idx, kind);
        capture.is_const = variable.native_const;
        capture.is_lexical = variable.native_lexical;
        capture.var_kind = variable.native_kind;
        capture
    }
    pub(super) fn capture(b: &ir::Binding, idx: u32, kind: u8) -> Capture {
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
    pub(super) fn vardef(b: &ir::Binding, var_ref_idx: u32, next: Option<u32>) -> VarDef {
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
    pub(super) fn atom(&mut self, name: &str) -> u32 {
        if self.budget.charge(Counter::Work, name.len()).is_err() {
            return 0;
        }
        if name.len() > self.budget.limits.max_atom_units.saturating_mul(3) {
            let _ = self.budget.text(name.len());
            return 0;
        }
        self.atom_units(&name.encode_utf16().collect::<Vec<_>>())
    }
    pub(super) fn atom_units(&mut self, units: &[u16]) -> u32 {
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
    pub(super) fn instruction(
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
    pub(super) fn effective_strict(&self) -> bool {
        self.function.strict || self.class_strict
    }
    pub(super) fn simple(&mut self, name: &'static str, pop: usize, push: usize, span: ir::Span) {
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
    pub(super) fn atom_op(
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
    pub(super) fn slot(&mut self, name: &'static str, slot: u32, pop: usize, push: usize, span: ir::Span) {
        // A temporary is only touched between `temporary` and `clear_temporaries`;
        // reuse of ended ones (see `temporary`) depends on it.
        debug_assert!(
            !(name.ends_with("_loc") && self.free_temporaries.contains(&slot)),
            "{name} on ended temporary {slot}"
        );
        self.instruction(name, (slot as u16).to_ne_bytes().to_vec(), pop, push, span)
    }
    pub(super) fn label(&mut self) -> u32 {
        if self.budget.charge(Counter::Work, 1).is_err() {
            return 0;
        }
        let id = self.labels;
        self.labels += 1;
        id
    }
    pub(super) fn mark(&mut self, id: u32) {
        if let Some(&depth) = self.label_depths.get(&id) {
            self.depth = depth;
        }
        if self.budget.charge(Counter::Work, 1).is_ok() {
            self.nodes.push(Node::Label(id));
        }
    }
    pub(super) fn jump(&mut self, name: &'static str, label: u32, span: ir::Span) {
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
    pub(super) fn handler(&mut self, label: u32, span: ir::Span, user_catch: bool) -> Result<(), String> {
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
    pub(super) fn with_jump(
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
    pub(super) fn constant(&mut self, make: impl FnOnce() -> Constant, span: ir::Span) {
        if self.budget.charge(Counter::Constants, 1).is_err() {
            return;
        }
        let n = self.function.constants.len() as u32;
        self.function.constants.push(make());
        self.instruction("push_const", n.to_ne_bytes().to_vec(), 0, 1, span)
    }
    pub(super) fn collect(&mut self, body: &'a [ir::Statement]) -> Result<(), String> {
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
    pub(super) fn pattern_bindings(
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
    pub(super) fn declare_pattern(&mut self, pattern: &'a ir::Pattern) -> Result<(), String> {
        for binding in self.pattern_bindings(pattern)? {
            self.declare(binding)?;
        }
        Ok(())
    }
    pub(super) fn declare(&mut self, b: &ir::Binding) -> Result<(), String> {
        self.declare_binding(b, false)
    }
    pub(super) fn declare_binding(&mut self, b: &ir::Binding, private: bool) -> Result<(), String> {
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
    pub(super) fn var_object_local(&mut self, span: ir::Span) -> Result<u64, String> {
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
    pub(super) fn define_var_object(&mut self, object: u64, name: &str, span: ir::Span) -> Result<(), String> {
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
}
