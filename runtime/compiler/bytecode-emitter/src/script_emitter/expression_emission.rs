//! Expression emission and finishing a function.

use super::*;

impl<'a> Emitter<'a> {
    pub(super) fn expression(&mut self, e: &'a ir::Expression) -> Result<(), String> {
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
                // lunarstorm patch: every `o[k] = v` took a new local, so a long
                // function (a 998-statement script of `o.pN=1;`) exceeded the
                // verifier's instructions x (locals + 1) budget and failed to load.
                // The slot only lives from the `put_loc` to the `get_loc` below,
                // with nothing but `put_array_el` between them, so one slot per
                // function serves every member assignment, nested ones included.
                let slot = match self.assign_member_slot {
                    Some(slot) => slot,
                    None => {
                        let slot = self.function.local_count;
                        self.budget.slots(
                            self.function.arg_count as usize
                                + self.function.local_count as usize
                                + 1,
                        )?;
                        self.function.local_count += 1;
                        if self.function.local_count > 65535 {
                            return Err("capacity: local count budget".into());
                        }
                        self.function.locals.push(VarDef::default());
                        self.assign_member_slot = Some(slot);
                        slot
                    }
                };
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
    pub(super) fn finish(mut self) -> Result<(FunctionImage, Vec<image::FunctionSpans>), String> {
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
