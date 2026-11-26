//! Blocks and scopes (Annex B copies, captured slots), loops and their
//! completion and cleanup, `switch`, `this`/`return`, generators' `yield`, and
//! destructuring patterns and iteration.

use super::*;

impl<'a> Emitter<'a> {
    pub(super) fn global_flags(b: &ir::Binding) -> u8 {
        match b.kind {
            ir::DeclarationKind::Let => 128 | 2,
            ir::DeclarationKind::Const => 128,
            ir::DeclarationKind::Function => 64,
            _ => 0,
        }
    }
    pub(super) fn begin_block(&mut self, body: &[&'a ir::Statement]) -> Result<(), String> {
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
    pub(super) fn annex_b_copy(&mut self, binding: &'a ir::Binding) -> Result<(), String> {
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
    pub(super) fn scope_level(&self, slot: u32) -> u32 {
        self.function.locals[(self.function.arg_count + slot) as usize].scope_level
    }
    /// The innermost lexical slot: the head of the scope chain that frame
    /// inspection (debug markers, direct eval) walks to find visible bindings.
    pub(super) fn scope_head(&self) -> Option<u32> {
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
    pub(super) fn push_block(&mut self, slots: Vec<u32>) {
        self.blocks.push(slots);
    }
    /// Drop captures that neither this function's code nor its closures use,
    /// renumbering the rest. Functions with direct eval, and debug builds,
    /// keep every capture: the frame is inspected by name at run time.
    pub(super) fn prune_captures(&mut self) -> Result<(), String> {
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
    pub(super) fn end_block(&mut self, span: ir::Span) {
        for slot in self.blocks.pop().expect("block entry") {
            self.slot("close_loc", slot, 0, 0, span);
        }
    }
    pub(super) fn block(&mut self, body: &'a [ir::Statement]) -> Result<(), String> {
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
    pub(super) fn marker(&mut self, span: ir::Span) -> Result<(), String> {
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
    pub(super) fn clone_iteration_slots(&mut self, slots: &[u32], span: ir::Span) -> Result<(), String> {
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
    pub(super) fn close_blocks(&mut self, depth: usize, span: ir::Span) -> Result<(), String> {
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
    pub(super) fn scope_slots(&mut self, scope: u32) -> Result<Vec<u32>, String> {
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
    pub(super) fn jump_target(&self, label: u32) -> JumpTarget {
        JumpTarget {
            label,
            blocks: self.blocks.len(),
            handlers: self.handler_depth,
            unwind: self.unwind.len(),
        }
    }
    pub(super) fn enter_loop(&mut self, next: u32, end: u32, labels: &[usize]) -> Result<(), String> {
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
    pub(super) fn cleanup_to(&mut self, blocks: usize, handlers: usize, span: ir::Span) -> Result<(), String> {
        self.close_blocks(blocks, span)?;
        self.blocks.truncate(blocks);
        for _ in handlers..self.handler_depth {
            self.simple("drop", 1, 0, span);
        }
        self.handler_depth = handlers;
        Ok(())
    }
    pub(super) fn completion(&mut self, completion: Completion, span: ir::Span) -> Result<(), String> {
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
    pub(super) fn finalizer_cleanup(
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
    pub(super) fn switch(
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
    pub(super) fn this_value(&mut self, span: ir::Span) -> Result<(), String> {
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
    pub(super) fn return_value(&mut self, has_value: bool, span: ir::Span) -> Result<(), String> {
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
    pub(super) fn async_iterator_close(&mut self, span: ir::Span) {
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
    pub(super) fn yield_return(&mut self, span: ir::Span) -> Result<(), String> {
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
    pub(super) fn yield_star(&mut self, span: ir::Span) -> Result<(), String> {
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
    pub(super) fn initialize_instance_fields(&mut self, span: ir::Span) -> Result<(), String> {
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
    pub(super) fn infer_pattern_name(
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
    pub(super) fn prepare_pattern_reference(
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
    pub(super) fn prepare_with_reference(
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
    pub(super) fn store_prepared_with(
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
    pub(super) fn pattern(&mut self, pattern: &'a ir::Pattern, span: ir::Span) -> Result<(), String> {
        self.pattern_prepared(pattern, span, None)
    }
    pub(super) fn pattern_prepared(
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
    pub(super) fn iteration_store(&mut self, left: &'a ir::ForTarget, span: ir::Span) -> Result<(), String> {
        let pattern = match left {
            ir::ForTarget::Declaration(declaration) => &declaration.pattern,
            ir::ForTarget::Assignment(pattern) => pattern,
        };
        self.pattern(pattern, span)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn iteration(
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
}
