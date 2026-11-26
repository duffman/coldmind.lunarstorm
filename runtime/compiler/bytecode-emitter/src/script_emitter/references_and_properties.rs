//! References: `with` scopes, variable reads and stores, function values,
//! temporaries, property references and stores, optional chains, binary
//! opcodes and object literal properties.

use super::*;

impl<'a> Emitter<'a> {
    /// Whether `scope` is `ancestor` or nested inside it.
    pub(super) fn scope_inside(&self, scope: u32, ancestor: u32) -> bool {
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
    pub(super) fn with_skips(&self, r: &ir::Reference, index: usize) -> bool {
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
    pub(super) fn with_applies(&self, r: &ir::Reference) -> bool {
        (0..self.withs.len()).any(|index| !self.with_skips(r, index))
    }
    /// Emits the with/var-object tests for `r`; returns the join label if any test was emitted.
    pub(super) fn with_tests(
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
    pub(super) fn read(&mut self, r: &ir::Reference, typeof_mode: bool, span: ir::Span) -> Result<(), String> {
        let done = self.with_tests(r, "with_get_var", 1, 0, (1, 1), span)?;
        self.read_plain(r, typeof_mode, span)?;
        if let Some(label) = done {
            self.mark(label);
        }
        Ok(())
    }
    pub(super) fn read_plain(
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
    pub(super) fn store(&mut self, r: &ir::Reference, init: bool, span: ir::Span) -> Result<(), String> {
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
    pub(super) fn store_plain(&mut self, r: &ir::Reference, init: bool, span: ir::Span) -> Result<(), String> {
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
    pub(super) fn function_constant(&mut self, f: &'a ir::Function, force_home: bool) -> Result<u32, String> {
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
    pub(super) fn function_value(&mut self, function: &'a ir::Function, span: ir::Span) -> Result<(), String> {
        let index = self.function_constant(function, false)?;
        self.instruction("fclosure", index.to_ne_bytes().to_vec(), 0, 1, span);
        Ok(())
    }
    pub(super) fn temporary(&mut self) -> Result<u32, String> {
        self.budget.charge(Counter::Work, 1)?;
        // lunarstorm patch: every temporary was a new local, never reused, so a
        // long function of compound member assignments (5000 x `o.pN+=1;`, three
        // temporaries each) exceeded the verifier's instructions x (locals + 1)
        // budget and failed to load. Decision: reuse a temporary
        // `clear_temporaries` has ended, from the same region (parameter
        // initializers or body), before taking a new local.
        let parameter = self.initializing_parameters;
        if let Some(index) = self
            .free_temporaries
            .iter()
            .rposition(|slot| self.parameter_region_locals.contains(slot) == parameter)
        {
            return Ok(self.free_temporaries.swap_remove(index));
        }
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
    /// Ends `slots`: stores undefined in each and frees it for reuse by
    /// [`Self::temporary`].
    ///
    /// # Arguments
    /// * `slots` - temporaries nothing emitted later reads or writes.
    /// * `span` - source span of the clearing instructions.
    pub(super) fn clear_temporaries(&mut self, slots: &[u32], span: ir::Span) {
        for &slot in slots {
            self.simple("undefined", 0, 1, span);
            self.slot("put_loc", slot, 1, 0, span);
            debug_assert!(!self.free_temporaries.contains(&slot), "temporary {slot} ended twice");
            self.free_temporaries.push(slot);
        }
    }
    // Compound/update/logical references normalize exactly once before GetValue.
    // Simple assignment intentionally keeps native late coercion after its RHS.
    pub(super) fn property_reference(
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
    pub(super) fn property_value(&mut self, base: u32, key: u32, span: ir::Span) {
        self.slot("get_loc", base, 0, 1, span);
        self.slot("get_loc", key, 0, 1, span);
        self.simple("get_array_el", 2, 1, span);
    }
    // Preserve the assigned value independently of setter return values.
    pub(super) fn property_store(&mut self, base: u32, key: u32, span: ir::Span) -> Result<(), String> {
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
    pub(super) fn logical_skip(&mut self, op: ir::Logical, end: u32, span: ir::Span) {
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
    pub(super) fn reference_value(&mut self, expression: &'a ir::Expression) -> Result<(), String> {
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
    pub(super) fn chain(
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
    pub(super) fn binary_opcode(op: ir::Binary) -> &'static str {
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
    pub(super) fn object_property(
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
}
