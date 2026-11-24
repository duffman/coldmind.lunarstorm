//! Native class construction and class-element scheduling.
//!
//! The definition phases and implicit-constructor algorithm are adapted from
//! quickjs-rust compiler_class.rs, commit 9d1a4bb5ab25774470ddab81e087f9794424e209.
//! Its custom VM operations are replaced with independently verified BC93 ops.
//! MIT License — Copyright (c) 2026 quickjs-rust contributors.
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//! The above copyright notice and this permission notice shall be included in
//! all copies or substantial portions of the Software.
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
//! THE SOFTWARE. Full license: third-party/quickjs-rust/LICENSE.
use super::*;

#[derive(Clone)]
pub(super) struct PrivateStorage {
    pub(super) name: Vec<u16>,
    pub(super) field: Option<u64>,
    pub(super) method: Option<u64>,
    pub(super) getter: Option<u64>,
    pub(super) setter: Option<u64>,
}
#[derive(Clone)]
enum FieldKey {
    Static(Vec<u16>),
    Computed(u64),
    Private(u32),
}
#[derive(Clone)]
struct FieldPlan {
    key: FieldKey,
    initializer: Option<(u64, bool)>,
}
enum StaticPlan {
    Field(FieldPlan),
    Block(u64),
}

impl<'a> Emitter<'a> {
    pub(super) fn inherit_auxiliary(&self, child: &mut Self) -> Result<(), String> {
        self.budget.charge(
            Counter::Work,
            self.auxiliary
                .len()
                .saturating_add(self.private_names.len()),
        )?;
        child.next_auxiliary = self.next_auxiliary.clone();
        child.withs = self.withs.clone();
        child.external_ranks = self.external_ranks.clone();
        for (&id, variable) in &self.auxiliary {
            self.budget.string(&variable.binding.name)?;
            if child.function.captures.len() >= 65535 {
                return Err("capacity: class capture count".into());
            }
            let (index, kind) = match variable.place {
                Place::Local(index) => (index, 0),
                Place::Argument(index) => (index, 1),
                Place::Capture(index) => (index, 2),
                Place::Global => return Err("internal class binding cannot be global".into()),
            };
            let slot = child.function.captures.len() as u32;
            child
                .function
                .captures
                .push(Self::capture_variable(variable, index, kind));
            child.auxiliary.insert(
                id,
                Variable {
                    place: Place::Capture(slot),
                    ..variable.clone()
                },
            );
        }
        for (&id, private) in &self.private_names {
            self.budget.charge(Counter::Work, private.name.len())?;
            child.private_names.insert(id, private.clone());
        }
        Ok(())
    }
    pub(super) fn auxiliary_local(
        &mut self,
        name: &[u16],
        scope: u32,
        kind: u8,
        span: ir::Span,
    ) -> Result<u64, String> {
        self.budget.charge(Counter::Work, name.len() + 1)?;
        let name_text = String::from_utf16(name)
            .map_err(|_| "private binding name is not valid identifier UTF-16")?;
        self.budget.string(&name_text)?;
        let key = (scope, name.to_vec(), kind);
        if let Some((id, variable)) = self.auxiliary_declarations.get(&key).cloned() {
            let Place::Local(slot) = variable.place else {
                return Err("cached class binding requires a local".into());
            };
            self.auxiliary.insert(id, variable);
            self.blocks
                .last_mut()
                .ok_or("class binding requires its lexical scope")?
                .push(slot);
            self.slot("set_loc_uninitialized", slot, 0, 0, span);
            return Ok(id);
        }
        let id = self.next_auxiliary.get();
        self.next_auxiliary.set(
            id.checked_add(1)
                .ok_or("capacity: class auxiliary identifier overflow")?,
        );
        let slot = self.temporary()?;
        let reference = self.function.var_ref_count;
        self.function.var_ref_count += 1;
        self.function.locals[(self.function.arg_count + slot) as usize] = VarDef {
            name: name.to_vec(),
            scope_level: scope + 2,
            is_const: true,
            is_lexical: true,
            is_captured: true,
            var_ref_idx: reference,
            var_kind: kind,
            ..Default::default()
        };
        self.local_scopes.insert(slot, scope);
        self.auxiliary.insert(
            id,
            Variable {
                binding: ir::Binding {
                    symbol: 0,
                    scope,
                    name: name_text,
                    kind: ir::DeclarationKind::Const,
                    span,
                },
                place: Place::Local(slot),
                native_const: true,
                native_lexical: true,
                native_kind: kind,
            },
        );
        let variable = self
            .auxiliary
            .get(&id)
            .ok_or("new class binding disappeared")?
            .clone();
        self.auxiliary_declarations.insert(key, (id, variable));
        self.blocks
            .last_mut()
            .ok_or("class binding requires its lexical scope")?
            .push(slot);
        self.slot("set_loc_uninitialized", slot, 0, 0, span);
        Ok(id)
    }
    pub(super) fn read_auxiliary(&mut self, id: u64, span: ir::Span) -> Result<(), String> {
        let variable = self
            .auxiliary
            .get(&id)
            .ok_or("missing class lexical binding")?;
        self.budget
            .charge(Counter::Work, variable.binding.name.len() + 1)?;
        match variable.place {
            Place::Local(slot) => self.slot("get_loc_check", slot, 0, 1, span),
            Place::Capture(slot) => self.slot("get_var_ref_check", slot, 0, 1, span),
            _ => return Err("invalid class lexical storage".into()),
        }
        Ok(())
    }
    pub(super) fn initialize_auxiliary(&mut self, id: u64, span: ir::Span) -> Result<(), String> {
        let variable = self
            .auxiliary
            .get(&id)
            .ok_or("missing class lexical binding")?;
        match variable.place {
            Place::Local(slot) => self.slot("put_loc_check_init", slot, 1, 0, span),
            _ => {
                return Err(
                    "class definition cannot initialize an inherited private binding".into(),
                );
            }
        }
        Ok(())
    }
    fn synthetic_class_function(&self, class: &'a ir::Class, name: &str) -> Result<Self, String> {
        self.budget.charge(Counter::Work, self.env.len())?;
        for variable in self.env.values() {
            self.budget
                .charge(Counter::Work, variable.binding.name.len())?;
        }
        let mut child = Self::new_region(
            self.program,
            None,
            self.env.clone(),
            self.source_map.clone(),
            self.budget.clone(),
            Some(class.span),
        )?;
        self.inherit_auxiliary(&mut child)?;
        self.budget.string(name)?;
        child.function.function_name = name.into();
        child.function.strict = true;
        child.function.flags = Default::default();
        child.scope = class.scope;
        child.root_locals = true;
        child.this_binding = None;
        child.uses_this = false;
        child.options = self.options;
        Ok(child)
    }
    fn append_class_function(
        &mut self,
        image: FunctionImage,
        mut spans: Vec<image::FunctionSpans>,
    ) -> Result<u32, String> {
        self.budget.charge(Counter::Constants, 1)?;
        self.budget.charge(
            Counter::Work,
            spans.iter().map(|span| span.path.len() + 1).sum(),
        )?;
        let index = self.function.constants.len() as u32;
        for span in &mut spans {
            span.path.insert(0, index);
        }
        self.child_spans.extend(spans);
        self.function
            .constants
            .push(Constant::Function(Box::new(image)));
        Ok(index)
    }
    fn synthetic_receiver(&mut self, derived: bool) -> Result<u32, String> {
        self.budget.string("this")?;
        let slot = self.temporary()?;
        let reference = self.function.var_ref_count;
        self.function.var_ref_count += 1;
        self.function.locals[(self.function.arg_count + slot) as usize] = VarDef {
            name: "this".encode_utf16().collect(),
            is_lexical: derived,
            is_captured: true,
            var_ref_idx: reference,
            ..Default::default()
        };
        Ok(slot)
    }
    // Adapted default_constructor: derived defaults forward actual argv directly.
    // In particular, this does not implement forwarding with iterable spread.
    fn default_constructor_constant(&mut self, class: &'a ir::Class) -> Result<u32, String> {
        let derived = class.heritage.is_some();
        let mut child =
            self.synthetic_class_function(class, class.name.as_deref().unwrap_or(""))?;
        child.is_class_constructor = true;
        child.derived_constructor = derived;
        child.function.flags.derived_constructor = derived;
        child.function.flags.new_target = true;
        child.function.flags.super_call = derived;
        child.function.flags.super_allowed = true;
        child.function.flags.arguments_allowed = derived;
        child.simple("check_ctor", 0, 0, class.span);
        let this = child.synthetic_receiver(derived)?;
        if derived {
            child.slot("set_loc_uninitialized", this, 0, 0, class.span);
            child.simple("init_ctor", 0, 1, class.span);
            child.slot("put_loc_check_init", this, 1, 0, class.span);
        } else {
            child.simple("push_this", 0, 1, class.span);
            child.slot("put_loc", this, 1, 0, class.span);
        }
        let binding = &class.instance_initializer_binding;
        child.read(
            &ir::Reference {
                symbol: Some(binding.symbol),
                name: binding.name.clone(),
            },
            false,
            class.span,
        )?;
        let absent = child.label();
        child.simple("dup", 1, 2, class.span);
        child.jump("if_false", absent, class.span);
        child.slot(
            if derived { "get_loc_check" } else { "get_loc" },
            this,
            0,
            1,
            class.span,
        );
        child.simple("swap", 2, 2, class.span);
        child.instruction("call_method", 0u16.to_ne_bytes().to_vec(), 2, 1, class.span);
        child.mark(absent);
        child.simple("drop", 1, 0, class.span);
        if derived {
            child.slot("get_loc_check", this, 0, 1, class.span);
        }
        child.simple(
            if derived { "return" } else { "return_undef" },
            usize::from(derived),
            0,
            class.span,
        );
        let (image, spans) = child.finish()?;
        self.append_class_function(image, spans)
    }
}

impl<'a> Emitter<'a> {
    fn private_display(name: &[u16]) -> Vec<u16> {
        std::iter::once(b'#' as u16)
            .chain(name.iter().copied())
            .collect()
    }
    fn rename_class_function(&mut self, constant: u32, name: &[u16]) -> Result<(), String> {
        self.budget.charge(Counter::Work, name.len())?;
        let name =
            String::from_utf16(name).map_err(|_| "private method name is not valid UTF-16")?;
        self.budget.string(&name)?;
        let Some(Constant::Function(function)) = self.function.constants.get_mut(constant as usize)
        else {
            return Err("class method requires function constant".into());
        };
        function.function_name = name;
        Ok(())
    }
    fn prepare_private_names(&mut self, class: &'a ir::Class) -> Result<(), String> {
        self.budget.charge(
            Counter::Work,
            class
                .private_names
                .len()
                .saturating_add(class.elements.len()),
        )?;
        let mut accessors = BTreeMap::<u32, (bool, bool)>::new();
        for element in &class.elements {
            if let ir::ClassElement::Method {
                key: ir::ClassKey::Private(id),
                kind,
                ..
            } = element
            {
                let entry = accessors.entry(*id).or_default();
                if *kind == ir::ClassMethodKind::Get {
                    entry.0 = true;
                }
                if *kind == ir::ClassMethodKind::Set {
                    entry.1 = true;
                }
            }
        }
        for private in &class.private_names {
            let name = Self::private_display(&private.name);
            let mut storage = PrivateStorage {
                name: name.clone(),
                field: None,
                method: None,
                getter: None,
                setter: None,
            };
            match private.kind {
                ir::PrivateKind::Field => {
                    let id = self.auxiliary_local(&name, class.scope, 5, class.span)?;
                    let atom = self.atom_units(&name);
                    self.instruction(
                        "private_symbol",
                        atom.to_ne_bytes().to_vec(),
                        0,
                        1,
                        class.span,
                    );
                    self.initialize_auxiliary(id, class.span)?;
                    storage.field = Some(id);
                }
                ir::PrivateKind::Method => {
                    storage.method =
                        Some(self.auxiliary_local(&name, class.scope, 6, class.span)?)
                }
                ir::PrivateKind::Accessor => {
                    let (getter, setter) = accessors.get(&private.id).copied().unwrap_or_default();
                    if getter {
                        storage.getter =
                            Some(self.auxiliary_local(&name, class.scope, 7, class.span)?);
                    }
                    if setter {
                        let setter_name: Vec<_> =
                            name.iter().copied().chain("<set>".encode_utf16()).collect();
                        storage.setter =
                            Some(self.auxiliary_local(&setter_name, class.scope, 8, class.span)?);
                    }
                }
            }
            if self.private_names.insert(private.id, storage).is_some() {
                return Err("private lexical identifiers must be fresh".into());
            }
        }
        Ok(())
    }
    fn field_key_value(&mut self, key: &FieldKey, span: ir::Span) -> Result<(), String> {
        match key {
            FieldKey::Static(name) => {
                self.budget.charge(Counter::Work, name.len())?;
                self.constant(|| Constant::String(name.clone()), span);
            }
            FieldKey::Computed(id) => self.read_auxiliary(*id, span)?,
            FieldKey::Private(id) => {
                let name = self
                    .private_names
                    .get(id)
                    .ok_or("unknown private field identifier")?
                    .name
                    .clone();
                self.budget.charge(Counter::Work, name.len())?;
                self.constant(|| Constant::String(name), span);
            }
        }
        Ok(())
    }
    fn initialize_field(
        &mut self,
        field: &FieldPlan,
        this: u32,
        span: ir::Span,
    ) -> Result<(), String> {
        self.slot("get_loc", this, 0, 1, span);
        match &field.key {
            FieldKey::Static(_) => {}
            FieldKey::Computed(id) => self.read_auxiliary(*id, span)?,
            FieldKey::Private(id) => {
                let symbol = self
                    .private_names
                    .get(id)
                    .and_then(|private| private.field)
                    .ok_or("private field symbol not defined")?;
                self.read_auxiliary(symbol, span)?;
            }
        }
        if let Some((initializer, named)) = field.initializer {
            self.slot("get_loc", this, 0, 1, span);
            self.read_auxiliary(initializer, span)?;
            if named {
                self.field_key_value(&field.key, span)?;
            }
            self.instruction(
                "call_method",
                u16::from(named).to_ne_bytes().to_vec(),
                2 + usize::from(named),
                1,
                span,
            );
        } else {
            self.simple("undefined", 0, 1, span);
        }
        match &field.key {
            FieldKey::Static(name) => {
                let atom = self.atom_units(name);
                self.instruction("define_field", atom.to_ne_bytes().to_vec(), 2, 1, span);
            }
            FieldKey::Computed(_) => {
                self.simple("define_array_el", 3, 2, span);
                self.simple("drop", 1, 0, span);
            }
            FieldKey::Private(_) => self.simple("define_private_field", 3, 1, span),
        }
        self.simple("drop", 1, 0, span);
        Ok(())
    }
    fn instance_initializer_constant(
        &mut self,
        class: &'a ir::Class,
        fields: &[FieldPlan],
        brand: bool,
    ) -> Result<u32, String> {
        let mut child = self.synthetic_class_function(class, "<instance_fields>")?;
        child.function.flags.need_home_object = true;
        child.function.flags.super_allowed = true;
        let this = child.synthetic_receiver(false)?;
        child.simple("push_this", 0, 1, class.span);
        child.slot("put_loc", this, 1, 0, class.span);
        if brand {
            child.budget.string("<home_object>")?;
            let home = child.temporary()?;
            let reference = child.function.var_ref_count;
            child.function.var_ref_count += 1;
            child.function.locals[(child.function.arg_count + home) as usize] = VarDef {
                name: "<home_object>".encode_utf16().collect(),
                is_captured: true,
                var_ref_idx: reference,
                ..Default::default()
            };
            child.instruction("special_object", vec![4], 0, 1, class.span);
            child.slot("put_loc", home, 1, 0, class.span);
            child.slot("get_loc", this, 0, 1, class.span);
            child.slot("get_loc", home, 0, 1, class.span);
            child.simple("add_brand", 2, 0, class.span);
        }
        for field in fields {
            child.initialize_field(field, this, class.span)?;
        }
        child.simple("return_undef", 0, 0, class.span);
        let (image, spans) = child.finish()?;
        self.append_class_function(image, spans)
    }
    // Adapted compile_class_strict / DefineClassElement / FinishClass phases.
    pub(super) fn class_value(
        &mut self,
        class: &'a ir::Class,
        inferred_name: Option<u32>,
    ) -> Result<(), String> {
        self.budget.charge(
            Counter::Work,
            class
                .elements
                .len()
                .saturating_add(self.private_names.len()),
        )?;
        for private in self.private_names.values() {
            self.budget.charge(Counter::Work, private.name.len())?;
        }
        let old_private = self.private_names.clone();
        self.budget.charge(Counter::Work, self.auxiliary.len())?;
        for variable in self.auxiliary.values() {
            self.budget
                .charge(Counter::Work, variable.binding.name.len())?;
        }
        let old_auxiliary = self.auxiliary.clone();
        let previous_strict = self.class_strict;
        self.class_strict = true;
        self.blocks.push(Vec::new());
        let mut class_symbols = Vec::new();
        for binding in class
            .self_binding
            .iter()
            .chain(std::iter::once(&class.instance_initializer_binding))
        {
            if self.env.contains_key(&binding.symbol) {
                return Err("class-local symbol conflicts with its enclosing environment".into());
            }
            if let Some(variable) = self.class_sources.get(&binding.symbol) {
                self.budget
                    .charge(Counter::Work, variable.binding.name.len())?;
                self.env.insert(binding.symbol, variable.clone());
            } else {
                self.declare_binding(binding, true)?;
                let variable = self
                    .env
                    .get(&binding.symbol)
                    .ok_or("class source binding disappeared")?;
                self.budget
                    .charge(Counter::Work, variable.binding.name.len())?;
                self.class_sources.insert(binding.symbol, variable.clone());
            }
            let Place::Local(slot) = self.env[&binding.symbol].place else {
                return Err("class binding requires a local".into());
            };
            self.blocks
                .last_mut()
                .ok_or("missing class lexical scope")?
                .push(slot);
            self.slot("set_loc_uninitialized", slot, 0, 0, class.span);
            class_symbols.push(binding.symbol);
        }
        self.marker(class.span)?;
        let computed_name = class.name.is_none() && inferred_name.is_some();
        if computed_name {
            self.slot(
                "get_loc",
                inferred_name.ok_or("missing inferred name")?,
                0,
                1,
                class.span,
            );
        }
        // Heritage is evaluated before entering this class's private environment.
        if let Some(heritage) = &class.heritage {
            self.expression(heritage)?;
        } else {
            self.simple("undefined", 0, 1, class.span);
        }
        self.prepare_private_names(class)?;
        let constructor = if let Some(constructor) = &class.constructor {
            self.function_constant(constructor, false)?
        } else {
            self.default_constructor_constant(class)?
        };
        // Atomic raw FunctionBytecode producer/consumer: never emit fclosure here.
        self.instruction(
            "push_const",
            constructor.to_ne_bytes().to_vec(),
            0,
            1,
            class.span,
        );
        let name = class
            .name
            .as_deref()
            .unwrap_or("")
            .encode_utf16()
            .collect::<Vec<_>>();
        let mut operands = self.atom_units(&name).to_ne_bytes().to_vec();
        operands.push(u8::from(class.heritage.is_some()));
        self.instruction(
            if computed_name {
                "define_class_computed"
            } else {
                "define_class"
            },
            operands,
            if computed_name { 3 } else { 2 },
            if computed_name { 3 } else { 2 },
            class.span,
        );
        let prototype = self.temporary()?;
        let constructor_value = self.temporary()?;
        self.slot("put_loc", prototype, 1, 0, class.span);
        self.slot("put_loc", constructor_value, 1, 0, class.span);
        if computed_name {
            self.simple("drop", 1, 0, class.span);
        }
        let mut fields = Vec::new();
        let mut static_work = Vec::new();
        let instance_brand = class
            .private_names
            .iter()
            .any(|name| !name.is_static && name.kind != ir::PrivateKind::Field);
        let static_brand = class
            .private_names
            .iter()
            .any(|name| name.is_static && name.kind != ir::PrivateKind::Field);
        // Private method capabilities and their home brands exist before any
        // computed key can test/read a lexically declared private name.
        for element in &class.elements {
            if let ir::ClassElement::Method {
                key: ir::ClassKey::Private(id),
                function,
                kind,
                is_static,
            } = element
            {
                let storage = self
                    .private_names
                    .get(id)
                    .ok_or("unknown private method")?
                    .clone();
                let slot = match kind {
                    ir::ClassMethodKind::Method => storage.method,
                    ir::ClassMethodKind::Get => storage.getter,
                    ir::ClassMethodKind::Set => storage.setter,
                }
                .ok_or("missing private method slot")?;
                self.slot(
                    "get_loc",
                    if *is_static {
                        constructor_value
                    } else {
                        prototype
                    },
                    0,
                    1,
                    class.span,
                );
                let constant = self.function_constant(function, true)?;
                let prefix = match kind {
                    ir::ClassMethodKind::Method => "",
                    ir::ClassMethodKind::Get => "get ",
                    ir::ClassMethodKind::Set => "set ",
                };
                let display: Vec<_> = prefix
                    .encode_utf16()
                    .chain(storage.name.iter().copied())
                    .collect();
                self.rename_class_function(constant, &display)?;
                self.instruction(
                    "fclosure",
                    constant.to_ne_bytes().to_vec(),
                    0,
                    1,
                    class.span,
                );
                self.simple("set_home_object", 2, 2, class.span);
                self.initialize_auxiliary(slot, class.span)?;
                self.simple("drop", 1, 0, class.span);
            }
        }
        if instance_brand {
            self.simple("null", 0, 1, class.span);
            self.slot("get_loc", prototype, 0, 1, class.span);
            self.simple("add_brand", 2, 0, class.span);
        }
        if static_brand {
            self.slot("get_loc", constructor_value, 0, 1, class.span);
            self.slot("get_loc", constructor_value, 0, 1, class.span);
            self.simple("add_brand", 2, 0, class.span);
        }
        for (index, element) in class.elements.iter().enumerate() {
            let home = match element {
                ir::ClassElement::Method { is_static, .. }
                | ir::ClassElement::Field { is_static, .. } => {
                    if *is_static {
                        constructor_value
                    } else {
                        prototype
                    }
                }
                ir::ClassElement::StaticBlock(_) => constructor_value,
            };
            let key = match element {
                ir::ClassElement::Method { key, .. } | ir::ClassElement::Field { key, .. } => {
                    Some(key)
                }
                ir::ClassElement::StaticBlock(_) => None,
            };
            let prepared_key = match key {
                Some(ir::ClassKey::Public(ir::ObjectKey::Static(name))) => {
                    self.budget.charge(Counter::Work, name.len())?;
                    Some(FieldKey::Static(name.clone()))
                }
                Some(ir::ClassKey::Public(ir::ObjectKey::Computed(expression))) => {
                    let name = format!("<computed_field_{index}>")
                        .encode_utf16()
                        .collect::<Vec<_>>();
                    let id = self.auxiliary_local(&name, class.scope, 0, expression.span)?;
                    self.marker(expression.span)?;
                    self.expression(expression)?;
                    self.simple("to_propkey", 1, 1, expression.span);
                    self.initialize_auxiliary(id, expression.span)?;
                    Some(FieldKey::Computed(id))
                }
                Some(ir::ClassKey::Private(id)) => Some(FieldKey::Private(*id)),
                None => None,
            };
            match element {
                ir::ClassElement::Method { function, kind, .. } => {
                    if matches!(&prepared_key, Some(FieldKey::Private(_))) {
                        continue;
                    }
                    self.slot("get_loc", home, 0, 1, class.span);
                    {
                        let flags = match kind {
                            ir::ClassMethodKind::Method => 0,
                            ir::ClassMethodKind::Get => 1,
                            ir::ClassMethodKind::Set => 2,
                        };
                        match prepared_key.as_ref().ok_or("method key missing")? {
                            FieldKey::Static(name) => {
                                self.function_value(function, class.span)?;
                                let mut operands = self.atom_units(name).to_ne_bytes().to_vec();
                                operands.push(flags);
                                self.instruction("define_method", operands, 2, 1, class.span);
                            }
                            FieldKey::Computed(id) => {
                                self.read_auxiliary(*id, class.span)?;
                                self.function_value(function, class.span)?;
                                self.instruction(
                                    "define_method_computed",
                                    vec![flags],
                                    3,
                                    1,
                                    class.span,
                                );
                            }
                            FieldKey::Private(_) => unreachable!(),
                        }
                        self.simple("drop", 1, 0, class.span);
                    }
                }
                ir::ClassElement::Field {
                    initializer,
                    is_static,
                    ..
                } => {
                    let initializer = if let Some(function) = initializer {
                        let name = format!("<field_initializer_{index}>")
                            .encode_utf16()
                            .collect::<Vec<_>>();
                        let id = self.auxiliary_local(&name, class.scope, 0, function.span)?;
                        self.slot("get_loc", home, 0, 1, class.span);
                        let constant =
                            self.function_constant(function, function.uses_home_object)?;
                        self.instruction(
                            "fclosure",
                            constant.to_ne_bytes().to_vec(),
                            0,
                            1,
                            function.span,
                        );
                        self.simple("set_home_object", 2, 2, function.span);
                        self.initialize_auxiliary(id, function.span)?;
                        self.simple("drop", 1, 0, function.span);
                        Some((id, function.initializer_name_binding.is_some()))
                    } else {
                        None
                    };
                    let field = FieldPlan {
                        key: prepared_key.ok_or("field key missing")?,
                        initializer,
                    };
                    if *is_static {
                        static_work.push(StaticPlan::Field(field));
                    } else {
                        fields.push(field);
                    }
                }
                ir::ClassElement::StaticBlock(function) => {
                    let name = format!("<static_block_{index}>")
                        .encode_utf16()
                        .collect::<Vec<_>>();
                    let id = self.auxiliary_local(&name, class.scope, 0, function.span)?;
                    self.slot("get_loc", home, 0, 1, class.span);
                    let constant = self.function_constant(function, function.uses_home_object)?;
                    self.instruction(
                        "fclosure",
                        constant.to_ne_bytes().to_vec(),
                        0,
                        1,
                        function.span,
                    );
                    self.simple("set_home_object", 2, 2, function.span);
                    self.initialize_auxiliary(id, function.span)?;
                    self.simple("drop", 1, 0, function.span);
                    static_work.push(StaticPlan::Block(id));
                }
            }
        }
        // Computed keys have all completed; only now expose the inner class name.
        if let Some(binding) = &class.self_binding {
            self.slot("get_loc", constructor_value, 0, 1, class.span);
            self.store(
                &ir::Reference {
                    symbol: Some(binding.symbol),
                    name: binding.name.clone(),
                },
                true,
                class.span,
            )?;
        }
        if fields.is_empty() && !instance_brand {
            self.simple("undefined", 0, 1, class.span);
        } else {
            let constant = self.instance_initializer_constant(class, &fields, instance_brand)?;
            self.slot("get_loc", prototype, 0, 1, class.span);
            self.instruction(
                "fclosure",
                constant.to_ne_bytes().to_vec(),
                0,
                1,
                class.span,
            );
            self.simple("set_home_object", 2, 2, class.span);
            self.simple("nip", 2, 1, class.span);
        }
        let binding = &class.instance_initializer_binding;
        self.store(
            &ir::Reference {
                symbol: Some(binding.symbol),
                name: binding.name.clone(),
            },
            true,
            class.span,
        )?;
        for work in static_work {
            self.marker(class.span)?;
            match work {
                StaticPlan::Field(field) => {
                    self.initialize_field(&field, constructor_value, class.span)?
                }
                StaticPlan::Block(block) => {
                    self.slot("get_loc", constructor_value, 0, 1, class.span);
                    self.read_auxiliary(block, class.span)?;
                    self.instruction("call_method", 0u16.to_ne_bytes().to_vec(), 2, 1, class.span);
                    self.simple("drop", 1, 0, class.span);
                }
            }
        }
        self.slot("get_loc", constructor_value, 0, 1, class.span);
        self.end_block(class.span);
        self.clear_temporaries(&[prototype, constructor_value], class.span);
        self.auxiliary = old_auxiliary;
        self.private_names = old_private;
        for symbol in class_symbols {
            self.env.remove(&symbol);
        }
        self.class_strict = previous_strict;
        Ok(())
    }
}
