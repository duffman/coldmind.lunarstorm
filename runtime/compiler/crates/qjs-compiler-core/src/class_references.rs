//! Reference-preserving assignment/update sequencing adapted from
//! Lewin671/quickjs-rust, commit 9d1a4bb5ab25774470ddab81e087f9794424e209,
//! crates/qjs-runtime/src/bytecode/compiler_assign.rs. The emitted instructions
//! here are the pinned native QuickJS opcodes, not the upstream Rust-VM ops.
//!
//! MIT License
//! Copyright (c) 2026 quickjs-rust contributors
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE. Full notice: third-party/quickjs-rust/LICENSE.
use super::*;

type SuperReference = (u32, u32, u32);

impl<'a> Emitter<'a> {
    pub(super) fn super_reference(
        &mut self,
        key: &'a ir::Expression,
        normalize: bool,
        span: ir::Span,
    ) -> Result<SuperReference, String> {
        let receiver = self.temporary()?;
        let base = self.temporary()?;
        let property = self.temporary()?;
        // GetThisBinding must throw before any computed-key effects. The raw
        // key expression precedes GetSuperBase; its later coercion does not.
        self.this_value(span)?;
        self.slot("put_loc", receiver, 1, 0, span);
        self.expression(key)?;
        self.slot("put_loc", property, 1, 0, key.span);
        let home = self
            .home_object_binding
            .as_ref()
            .ok_or("super reference has no home object")?;
        self.budget
            .charge(Counter::Work, home.name.len().saturating_add(1))?;
        let home = ir::Reference {
            symbol: Some(home.symbol),
            name: home.name.clone(),
        };
        self.read(&home, false, span)?;
        self.simple("get_super", 1, 1, span);
        self.slot("put_loc", base, 1, 0, span);
        if normalize {
            self.slot("get_loc", property, 0, 1, key.span);
            self.simple("to_propkey", 1, 1, key.span);
            self.slot("put_loc", property, 1, 0, key.span);
        }
        self.budget.check()?;
        Ok((receiver, base, property))
    }
    pub(super) fn super_get(
        &mut self,
        reference: SuperReference,
        span: ir::Span,
    ) -> Result<(), String> {
        let (receiver, base, key) = reference;
        self.slot("get_loc", receiver, 0, 1, span);
        self.slot("get_loc", base, 0, 1, span);
        self.slot("get_loc", key, 0, 1, span);
        self.simple("get_super_value", 3, 1, span);
        self.budget.check()
    }
    pub(super) fn super_put(
        &mut self,
        reference: SuperReference,
        span: ir::Span,
    ) -> Result<(), String> {
        let (receiver, base, key) = reference;
        let value = self.temporary()?;
        self.slot("put_loc", value, 1, 0, span);
        self.slot("get_loc", receiver, 0, 1, span);
        self.slot("get_loc", base, 0, 1, span);
        self.slot("get_loc", key, 0, 1, span);
        self.slot("get_loc", value, 0, 1, span);
        self.simple("put_super_value", 4, 0, span);
        self.slot("get_loc", value, 0, 1, span);
        self.clear_temporaries(&[value], span);
        self.budget.check()
    }
    fn reference_private_storage(&self, id: u32) -> Result<PrivateStorage, String> {
        let storage = self
            .private_names
            .get(&id)
            .ok_or("private reference has no lexical storage")?;
        self.budget
            .charge(Counter::Work, storage.name.len().saturating_add(1))?;
        let accessors = storage.getter.is_some() || storage.setter.is_some();
        if storage.field.is_some() && (storage.method.is_some() || accessors)
            || storage.method.is_some() && accessors
            || storage.field.is_none() && storage.method.is_none() && !accessors
        {
            return Err("private storage has inconsistent field/method/accessor roles".into());
        }
        Ok(storage.clone())
    }
    fn private_read_only_error(&mut self, name: &[u16], span: ir::Span) -> Result<(), String> {
        let atom = self.atom_units(name);
        let mut operands = atom.to_ne_bytes().to_vec();
        operands.push(0); // JS_THROW_VAR_RO, pinned quickjs.c.
        self.instruction("throw_error", operands, 0, 0, span);
        self.budget.check()
    }
    pub(super) fn private_reference(
        &mut self,
        object: &'a ir::Expression,
        span: ir::Span,
    ) -> Result<u32, String> {
        let base = self.temporary()?;
        // No brand check here: a simple assignment evaluates its RHS first.
        self.expression(object)?;
        self.slot("put_loc", base, 1, 0, span);
        self.budget.check()?;
        Ok(base)
    }
    pub(super) fn private_get(&mut self, base: u32, id: u32, span: ir::Span) -> Result<(), String> {
        let storage = self.reference_private_storage(id)?;
        self.slot("get_loc", base, 0, 1, span);
        if let Some(symbol) = storage.field {
            self.read_auxiliary(symbol, span)?;
            self.simple("get_private_field", 2, 1, span);
        } else if let Some(method) = storage.method {
            self.read_auxiliary(method, span)?;
            self.simple("check_brand", 2, 2, span);
            self.simple("nip", 2, 1, span);
        } else if let Some(getter) = storage.getter {
            self.read_auxiliary(getter, span)?;
            self.simple("check_brand", 2, 2, span);
            self.instruction("call_method", 0u16.to_ne_bytes().to_vec(), 2, 1, span);
        } else {
            // Setter-only access uses the same pinned native error as the C
            // resolver and never invokes the setter or an unrelated getter.
            self.private_read_only_error(&storage.name, span)?;
        }
        self.budget.check()
    }
    pub(super) fn private_put(&mut self, base: u32, id: u32, span: ir::Span) -> Result<(), String> {
        let storage = self.reference_private_storage(id)?;
        let value = self.temporary()?;
        self.slot("put_loc", value, 1, 0, span);
        self.slot("get_loc", base, 0, 1, span);
        if let Some(symbol) = storage.field {
            // Native put_private_field consumes object, value, private symbol.
            self.slot("get_loc", value, 0, 1, span);
            self.read_auxiliary(symbol, span)?;
            self.simple("put_private_field", 3, 0, span);
        } else if let Some(setter) = storage.setter {
            self.read_auxiliary(setter, span)?;
            self.simple("check_brand", 2, 2, span);
            self.slot("get_loc", value, 0, 1, span);
            self.instruction("call_method", 1u16.to_ne_bytes().to_vec(), 3, 1, span);
            self.simple("drop", 1, 0, span);
        } else {
            self.private_read_only_error(&storage.name, span)?;
        }
        self.slot("get_loc", value, 0, 1, span);
        self.clear_temporaries(&[value], span);
        self.budget.check()
    }
    pub(super) fn private_in(
        &mut self,
        object: &'a ir::Expression,
        id: u32,
        span: ir::Span,
    ) -> Result<(), String> {
        let storage = self.reference_private_storage(id)?;
        let token = storage
            .field
            .or(storage.method)
            .or(storage.getter)
            .or(storage.setter)
            .ok_or("private brand has no field symbol or branded method")?;
        self.expression(object)?;
        self.read_auxiliary(token, span)?;
        self.simple("private_in", 2, 1, span);
        self.budget.check()
    }
    pub(super) fn emit_super_assign(
        &mut self,
        key: &'a ir::Expression,
        op: &ir::AssignmentOp,
        value: &'a ir::Expression,
        span: ir::Span,
    ) -> Result<(), String> {
        let reference = self.super_reference(key, !matches!(op, ir::AssignmentOp::Assign), span)?;
        match op {
            ir::AssignmentOp::Assign => {
                self.expression(value)?;
                self.super_put(reference, span)?;
            }
            ir::AssignmentOp::Compound(op) => {
                self.super_get(reference, span)?;
                self.expression(value)?;
                self.simple(Self::binary_opcode(*op), 2, 1, span);
                self.super_put(reference, span)?;
            }
            ir::AssignmentOp::Logical(op) => {
                let end = self.label();
                self.super_get(reference, span)?;
                self.logical_skip(*op, end, span);
                self.simple("drop", 1, 0, span);
                self.expression(value)?;
                self.super_put(reference, span)?;
                self.mark(end);
            }
        }
        self.clear_temporaries(&[reference.0, reference.1, reference.2], span);
        self.budget.check()
    }
    pub(super) fn emit_super_update(
        &mut self,
        key: &'a ir::Expression,
        increment: bool,
        prefix: bool,
        span: ir::Span,
    ) -> Result<(), String> {
        let reference = self.super_reference(key, true, span)?;
        self.super_get(reference, span)?;
        self.simple(
            match (increment, prefix) {
                (true, true) => "inc",
                (false, true) => "dec",
                (true, false) => "post_inc",
                (false, false) => "post_dec",
            },
            1,
            if prefix { 1 } else { 2 },
            span,
        );
        self.super_put(reference, span)?;
        if !prefix {
            self.simple("drop", 1, 0, span);
        }
        self.clear_temporaries(&[reference.0, reference.1, reference.2], span);
        self.budget.check()
    }
    pub(super) fn emit_private_assign(
        &mut self,
        object: &'a ir::Expression,
        id: u32,
        op: &ir::AssignmentOp,
        value: &'a ir::Expression,
        span: ir::Span,
    ) -> Result<(), String> {
        let base = self.private_reference(object, span)?;
        match op {
            ir::AssignmentOp::Assign => {
                self.expression(value)?;
                self.private_put(base, id, span)?;
            }
            ir::AssignmentOp::Compound(op) => {
                self.private_get(base, id, span)?;
                self.expression(value)?;
                self.simple(Self::binary_opcode(*op), 2, 1, span);
                self.private_put(base, id, span)?;
            }
            ir::AssignmentOp::Logical(op) => {
                let end = self.label();
                self.private_get(base, id, span)?;
                self.logical_skip(*op, end, span);
                self.simple("drop", 1, 0, span);
                self.expression(value)?;
                self.private_put(base, id, span)?;
                self.mark(end);
            }
        }
        self.clear_temporaries(&[base], span);
        self.budget.check()
    }
    pub(super) fn emit_private_update(
        &mut self,
        object: &'a ir::Expression,
        id: u32,
        increment: bool,
        prefix: bool,
        span: ir::Span,
    ) -> Result<(), String> {
        let base = self.private_reference(object, span)?;
        self.private_get(base, id, span)?;
        self.simple(
            match (increment, prefix) {
                (true, true) => "inc",
                (false, true) => "dec",
                (true, false) => "post_inc",
                (false, false) => "post_dec",
            },
            1,
            if prefix { 1 } else { 2 },
            span,
        );
        self.private_put(base, id, span)?;
        if !prefix {
            self.simple("drop", 1, 0, span);
        }
        self.clear_temporaries(&[base], span);
        self.budget.check()
    }
}
