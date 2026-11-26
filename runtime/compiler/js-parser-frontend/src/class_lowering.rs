// Class-definition regions, element scheduling, and private-element separation
// adapted from Lewin671/quickjs-rust, commit
// 9d1a4bb5ab25774470ddab81e087f9794424e209,
// crates/qjs-runtime/src/bytecode/compiler_class.rs.
// Repository notice: third-party/quickjs-rust/LICENSE.
// Adaptation: pinned OXC AST -> our owned source IR and native QuickJS backend;
// no upstream Rust-VM opcodes, parser, runtime, or source regeneration is used.
//
// MIT License
// Copyright (c) 2026 quickjs-rust contributors
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use super::*;
use std::collections::BTreeMap;

impl Lower<'_> {
    pub(super) fn private_name(&self, name: &ast::PrivateIdentifier<'_>) -> R<u32> {
        self.private_frames
            .borrow()
            .iter()
            .rev()
            .find_map(|frame| frame.get(name.name.as_str()).copied())
            .ok_or_else(|| {
                self.unsupported(name.span, "private name has no lexical class environment")
            })
    }
    pub(super) fn mark_super(&self, call: bool, span: oxc_span::Span) -> R<()> {
        let frame = self
            .super_frames
            .borrow()
            .last()
            .cloned()
            .flatten()
            .ok_or_else(|| self.unsupported(span, "super requires a method home object"))?;
        if call && !frame.call_allowed {
            return Err(self.unsupported(span, "super call requires a derived constructor owner"));
        }
        let owner = *self.this_owners.borrow().last().expect("super this owner");
        for used in &mut self.this_uses.borrow_mut()[owner..] {
            *used = true;
        }
        for used in &mut self.home_object_uses.borrow_mut()[owner..] {
            *used = true;
        }
        if call {
            for used in &mut self.new_target_uses.borrow_mut()[owner..] {
                *used = true;
            }
        }
        Ok(())
    }
    pub(super) fn super_key(
        &self,
        member: &ast::MemberExpression<'_>,
    ) -> R<Option<Box<ir::Expression>>> {
        let key = match member {
            ast::MemberExpression::StaticMemberExpression(member)
                if matches!(member.object, ast::Expression::Super(_)) =>
            {
                self.mark_super(false, member.span)?;
                ir::Expression {
                    kind: ir::ExpressionKind::Literal(ir::Literal::String(
                        member.property.name.encode_utf16().collect(),
                    )),
                    span: self.span(member.property.span),
                }
            }
            ast::MemberExpression::ComputedMemberExpression(member)
                if matches!(member.object, ast::Expression::Super(_)) =>
            {
                self.mark_super(false, member.span)?;
                self.expression(&member.expression)?
            }
            _ => return Ok(None),
        };
        Ok(Some(Box::new(key)))
    }
    pub(super) fn method_function(
        &self,
        function: &ast::Function<'_>,
        context: MethodContext,
    ) -> R<ir::Function> {
        let body = function
            .body
            .as_ref()
            .ok_or_else(|| self.unsupported(function.span, "method requires a body"))?;
        let mut lowered = self.function_parts(
            &function.params,
            function.scope_id.get().expect("semantic method scope"),
            function.span,
            function.id.as_ref(),
            false,
            false,
            Some(body),
            None,
            Some(context),
        )?;
        lowered.kind = Self::function_kind(function.r#async, function.generator);
        Ok(lowered)
    }
    fn initializer_scope(&self, parent: u32) -> u32 {
        let id = self.next_scope.get();
        self.next_scope.set(id + 1);
        self.extra_scopes.borrow_mut().push(ir::Scope {
            id,
            parent: Some(parent),
            strict: true,
            function: true,
        });
        id
    }
    fn initializer_function(
        &self,
        value: Option<&ast::Expression<'_>>,
        statements: Option<&[ast::Statement<'_>]>,
        source_scope: Option<u32>,
        parent_scope: u32,
        span: oxc_span::Span,
    ) -> R<ir::Function> {
        let scope = source_scope.unwrap_or_else(|| self.initializer_scope(parent_scope));
        let hidden = |name: &str| ir::Binding {
            symbol: self.next_symbol(),
            scope,
            name: name.into(),
            kind: ir::DeclarationKind::Var,
            span: self.span(span),
        };
        let this_binding = hidden("this");
        let new_target_binding = hidden("new.target");
        let home = hidden("<home_object>");
        self.function_scopes.borrow_mut().push(scope);
        let owner = self.this_uses.borrow().len();
        self.this_owners.borrow_mut().push(owner);
        self.this_bindings.borrow_mut().push(this_binding.clone());
        self.new_target_bindings
            .borrow_mut()
            .push(Some(new_target_binding.clone()));
        self.this_uses.borrow_mut().push(false);
        self.new_target_uses.borrow_mut().push(false);
        self.home_object_uses.borrow_mut().push(false);
        self.arguments_permissions.borrow_mut().push(false);
        self.super_frames.borrow_mut().push(Some(SuperFrame {
            home: home.clone(),
            constructor: None,
            field_initializer: None,
            call_allowed: false,
        }));
        let mut initializer_name_binding = None;
        let body = if let Some(value) = value {
            self.expression(value).map(|value| {
                let span = value.span;
                let value = if Self::anonymous_definition(&value) {
                    let binding = ir::Binding {
                        symbol: self.next_symbol(),
                        scope,
                        name: "<field_name>".into(),
                        kind: ir::DeclarationKind::Parameter,
                        span,
                    };
                    let name = ir::Expression {
                        span,
                        kind: ir::ExpressionKind::Read(ir::Reference {
                            symbol: Some(binding.symbol),
                            name: binding.name.clone(),
                        }),
                    };
                    initializer_name_binding = Some(binding);
                    ir::Expression {
                        span,
                        kind: ir::ExpressionKind::NamedEvaluation {
                            name: Box::new(name),
                            value: Box::new(value),
                        },
                    }
                } else {
                    value
                };
                vec![ir::Statement {
                    span,
                    kind: ir::StatementKind::Return(Some(value)),
                }]
            })
        } else {
            statements
                .unwrap_or(&[])
                .iter()
                .map(|statement| self.statement(statement))
                .collect::<R<_>>()
        };
        self.function_scopes.borrow_mut().pop();
        self.this_owners.borrow_mut().pop();
        self.this_bindings.borrow_mut().pop();
        self.new_target_bindings.borrow_mut().pop();
        self.super_frames.borrow_mut().pop();
        self.arguments_permissions.borrow_mut().pop();
        let uses_this = self
            .this_uses
            .borrow_mut()
            .pop()
            .expect("initializer this context");
        let uses_new_target = self
            .new_target_uses
            .borrow_mut()
            .pop()
            .expect("initializer new.target context");
        let uses_home_object = self
            .home_object_uses
            .borrow_mut()
            .pop()
            .expect("initializer home context");
        let parameters = initializer_name_binding
            .iter()
            .cloned()
            .map(|binding| ir::Pattern::Binding {
                binding,
                initialization_target: None,
            })
            .collect();
        Ok(ir::Function {
            initializer_name_binding,
            arguments_allowed: false,
            is_class_constructor: false,
            derived_constructor: false,
            home_object_binding: Some(home),
            uses_home_object,
            super_constructor_binding: None,
            field_initializer_binding: None,
            super_call_allowed: false,
            is_arrow: false,
            new_target_binding: Some(new_target_binding),
            uses_new_target,
            this_binding: Some(this_binding),
            uses_this,
            arguments_binding: None,
            is_method: true,
            self_binding: None,
            name: None,
            scope,
            parameters,
            has_parameter_expressions: false,
            defined_arg_count: 0,
            body_parameter_copies: Vec::new(),
            rest_parameter: None,
            body: body?,
            strict: true,
            has_direct_eval: false,
            span: self.span(span),
            kind: ir::FunctionKind::Normal,
        })
    }
    fn class_key(&self, key: &ast::PropertyKey<'_>, computed: bool) -> R<ir::ClassKey> {
        if let ast::PropertyKey::PrivateIdentifier(name) = key {
            Ok(ir::ClassKey::Private(self.private_name(name)?))
        } else {
            Ok(ir::ClassKey::Public(self.object_key(key, computed)?))
        }
    }
    pub(super) fn class(&self, class: &ast::Class<'_>) -> R<ir::Class> {
        if !class.decorators.is_empty() {
            return Err(self.unsupported(class.span, "class decorators are not implemented"));
        }
        let scope = class.scope_id.get().expect("semantic class scope").index() as u32;
        let self_binding = class.id.as_ref().map(|id| {
            let mut binding = self.binding(id, ir::DeclarationKind::Const);
            binding.symbol = self.next_symbol();
            binding.scope = scope;
            self.class_name_frames.borrow_mut().push(ClassNameFrame {
                semantic_symbol: id.symbol_id.get().expect("semantic class name").index() as u32,
                binding: binding.clone(),
            });
            binding
        });
        let result = (|| {
            // Heritage uses the enclosing private environment. The new private
            // environment belongs to the class body and its nested functions.
            let heritage = class
                .heritage
                .as_ref()
                .map(|heritage| self.expression(&heritage.expression).map(Box::new))
                .transpose()?;
            let mut private_names: Vec<ir::PrivateName> = Vec::new();
            let mut private_indices: BTreeMap<String, usize> = BTreeMap::new();
            for element in &class.body.body {
                let (key, is_static, kind) = match element {
                    ast::ClassElement::MethodDefinition(method) => (
                        &method.key,
                        method.r#static,
                        if matches!(
                            method.kind,
                            ast::MethodDefinitionKind::Get | ast::MethodDefinitionKind::Set
                        ) {
                            ir::PrivateKind::Accessor
                        } else {
                            ir::PrivateKind::Method
                        },
                    ),
                    ast::ClassElement::PropertyDefinition(field) => {
                        (&field.key, field.r#static, ir::PrivateKind::Field)
                    }
                    _ => continue,
                };
                if let ast::PropertyKey::PrivateIdentifier(name) = key {
                    if let Some(index) = private_indices.get(name.name.as_str()) {
                        let existing = &private_names[*index];
                        if existing.kind != ir::PrivateKind::Accessor
                            || kind != ir::PrivateKind::Accessor
                            || existing.is_static != is_static
                        {
                            return Err(
                                self.unsupported(name.span, "duplicate private class declaration")
                            );
                        }
                    } else {
                        let id = self.next_private_id.get();
                        self.next_private_id.set(id + 1);
                        private_indices.insert(name.name.to_string(), private_names.len());
                        private_names.push(ir::PrivateName {
                            id,
                            name: name.name.encode_utf16().collect(),
                            is_static,
                            kind,
                        });
                    }
                }
            }
            self.private_frames.borrow_mut().push(
                private_indices
                    .iter()
                    .map(|(name, index)| (name.clone(), private_names[*index].id))
                    .collect(),
            );
            let instance_initializer_binding = ir::Binding {
                symbol: self.next_symbol(),
                scope,
                name: format!("<class_fields:{scope}>"),
                kind: ir::DeclarationKind::Const,
                span: self.span(class.span),
            };
            let elements = (|| {
                let mut constructor = None;
                let mut elements = Vec::new();
                for element in &class.body.body {
                    match element {
                        ast::ClassElement::MethodDefinition(method) => {
                            if !method.decorators.is_empty() {
                                return Err(self.unsupported(
                                    method.span,
                                    "method decorators are not implemented",
                                ));
                            }
                            let is_constructor =
                                method.kind == ast::MethodDefinitionKind::Constructor;
                            let mut function = self.method_function(
                                &method.value,
                                MethodContext {
                                    constructor: is_constructor,
                                    derived: heritage.is_some(),
                                    field_initializer: is_constructor
                                        .then(|| instance_initializer_binding.clone()),
                                },
                            )?;
                            if is_constructor {
                                function.span = self.span(class.span);
                                constructor = Some(Box::new(function));
                                continue;
                            }
                            function.span = super::function_source::class_method_span(
                                self.input,
                                method.span,
                                method.r#static,
                            )
                            .ok_or_else(|| {
                                self.unsupported(method.span, "method source boundary is invalid")
                            })?;
                            let kind = match method.kind {
                                ast::MethodDefinitionKind::Method => ir::ClassMethodKind::Method,
                                ast::MethodDefinitionKind::Get => ir::ClassMethodKind::Get,
                                ast::MethodDefinitionKind::Set => ir::ClassMethodKind::Set,
                                ast::MethodDefinitionKind::Constructor => unreachable!(),
                            };
                            elements.push(ir::ClassElement::Method {
                                key: self.class_key(&method.key, method.computed)?,
                                function,
                                kind,
                                is_static: method.r#static,
                            });
                        }
                        ast::ClassElement::PropertyDefinition(field) => {
                            if !field.decorators.is_empty() {
                                return Err(self.unsupported(
                                    field.span,
                                    "field decorators are not implemented",
                                ));
                            }
                            let key = self.class_key(&field.key, field.computed)?;
                            let initializer = field
                                .value
                                .as_ref()
                                .map(|value| {
                                    self.initializer_function(
                                        Some(value),
                                        None,
                                        None,
                                        scope,
                                        value.span(),
                                    )
                                })
                                .transpose()?;
                            elements.push(ir::ClassElement::Field {
                                key,
                                initializer,
                                is_static: field.r#static,
                            });
                        }
                        ast::ClassElement::StaticBlock(block) => {
                            elements.push(ir::ClassElement::StaticBlock(
                                self.initializer_function(
                                    None,
                                    Some(&block.body),
                                    Some(
                                        block
                                            .scope_id
                                            .get()
                                            .expect("semantic static block scope")
                                            .index() as u32,
                                    ),
                                    scope,
                                    block.span,
                                )?,
                            ));
                        }
                        _ => {
                            return Err(self.unsupported(
                                element.span(),
                                "class auto-accessors or type syntax are not implemented",
                            ));
                        }
                    }
                }
                Ok((constructor, elements))
            })();
            self.private_frames.borrow_mut().pop();
            let (constructor, elements) = elements?;
            // Missing constructors are represented explicitly. The native emitter
            // synthesizes a base check_ctor or a derived init_ctor with direct
            // argument-list forwarding; it must not consult Array iteration.
            Ok(ir::Class {
                name: class.id.as_ref().map(|id| id.name.to_string()),
                self_binding: self_binding.clone(),
                heritage,
                constructor,
                elements,
                private_names,
                instance_initializer_binding,
                scope,
                span: self.span(class.span),
            })
        })();
        if self_binding.is_some() {
            self.class_name_frames.borrow_mut().pop();
        }
        result
    }
}
