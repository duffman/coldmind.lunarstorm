//! Statement emission: every statement kind (declarations, control flow, `try`,
//! loops, `with`, labels), with completion values and abrupt exits.

use super::*;

impl<'a> Emitter<'a> {
    pub(super) fn statement(&mut self, s: &'a ir::Statement) -> Result<(), String> {
        self.statement_with_labels(s, &[])
    }
    pub(super) fn statement_with_labels(
        &mut self,
        s: &'a ir::Statement,
        labels: &[usize],
    ) -> Result<(), String> {
        let base = std::mem::replace(&mut self.statement_base, self.depth);
        let result = self.statement_at_base(s, labels);
        self.statement_base = base;
        result
    }
    pub(super) fn statement_at_base(&mut self, s: &'a ir::Statement, labels: &[usize]) -> Result<(), String> {
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
}
