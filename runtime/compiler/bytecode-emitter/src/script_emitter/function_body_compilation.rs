//! Compiling one function body: parameters, hoisting, the prologue and the
//! body's statements.

use super::*;

impl<'a> Emitter<'a> {
    pub(super) fn compile(
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
}
