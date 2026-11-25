//! Post-parse, pre-semantic budgets for the exact pinned OXC AST.
//! The parser itself is bounded by the caller's worker heap/deadline boundary.
use crate::{Category, Diagnostic, source::SourceInput};
use oxc_ast::{AstKind, ast::*};
use oxc_ast_visit::{Visit, walk};
use oxc_span::GetSpan;
use oxc_syntax::scope::ScopeFlags;

/// Limits on original parsed syntax, before semantic analysis/lowering.
/// Counts include Program and every concrete OXC AstKind. BindingIdentifier
/// occurrences count even when declarations later merge into one symbol.
/// Compiler-created strict directives and implicit bindings are not source AST.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompileLimits {
    pub max_ast_nodes: usize,
    pub max_nesting: usize,
    pub max_functions: usize,
    pub max_bindings: usize,
}
impl Default for CompileLimits {
    fn default() -> Self {
        Self {
            max_ast_nodes: 1_000_000,
            max_nesting: 256,
            max_functions: 65_535,
            max_bindings: 65_535,
        }
    }
}
impl CompileLimits {
    pub fn validate(&self) -> Result<(), &'static str> {
        let ceiling = Self::default();
        if self.max_ast_nodes > ceiling.max_ast_nodes
            || self.max_nesting > ceiling.max_nesting
            || self.max_functions > ceiling.max_functions
            || self.max_bindings > ceiling.max_bindings
        {
            Err("frontend limit override exceeds qualified ceiling")
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParsePhase {
    Begin,
    End,
}

pub(crate) fn diagnostic(
    filename: &str,
    input: &SourceInput,
    span: oxc_span::Span,
    message: &str,
) -> Diagnostic {
    let original = input
        .original_span(span.start as usize..span.end as usize)
        .expect("budget span in parser view");
    Diagnostic {
        category: Category::Capacity,
        filename: filename.into(),
        span: qjs_ir::Span {
            start: original.start,
            end: original.end,
        },
        message: message.into(),
    }
}
pub(crate) fn validate_limits(
    limits: &CompileLimits,
    input: &SourceInput,
    filename: &str,
) -> Result<(), Vec<Diagnostic>> {
    limits.validate().map_err(|message| {
        vec![diagnostic(
            filename,
            input,
            oxc_span::Span::new(0, 0),
            message,
        )]
    })
}

type Expand<'a> = fn(&mut Children<'a>, AstKind<'a>);
struct Pending<'a> {
    kind: AstKind<'a>,
    depth: usize,
    expand: Expand<'a>,
}
struct Children<'a> {
    pending: Vec<Pending<'a>>,
    depth: usize,
    limits: CompileLimits,
    nodes: usize,
    functions: usize,
    bindings: usize,
    failure: Option<(oxc_span::Span, &'static str)>,
}
impl<'a> Children<'a> {
    fn push(&mut self, kind: AstKind<'a>, expand: Expand<'a>) {
        if self.failure.is_some() {
            return;
        }
        let depth = self.depth + 1;
        let message = if depth > self.limits.max_nesting {
            Some("frontend AST nesting capacity exceeded")
        } else if self.nodes >= self.limits.max_ast_nodes {
            Some("frontend AST node capacity exceeded")
        } else if matches!(
            kind,
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_)
        ) && self.functions >= self.limits.max_functions
        {
            Some("frontend function capacity exceeded")
        } else if matches!(kind, AstKind::BindingIdentifier(_))
            && self.bindings >= self.limits.max_bindings
        {
            Some("frontend binding capacity exceeded")
        } else {
            None
        };
        if let Some(message) = message {
            self.failure = Some((kind.span(), message));
            return;
        }
        if self.pending.try_reserve(1).is_err() {
            self.failure = Some((
                kind.span(),
                "frontend AST worklist allocation capacity exceeded",
            ));
            return;
        }
        self.nodes += 1;
        self.functions += usize::from(matches!(
            kind,
            AstKind::Function(_) | AstKind::ArrowFunctionExpression(_)
        ));
        self.bindings += usize::from(matches!(kind, AstKind::BindingIdentifier(_)));
        self.pending.push(Pending {
            kind,
            depth,
            expand,
        });
    }
}
// Each adapter queues one concrete node; only its later explicit work item walks
// immediate children. Thus even unsupported/deep syntax never recurses here.
macro_rules! nodes {($(($kind:ident,$visit:ident,$walk:ident,$ty:ty)),*$(,)?)=>{
 #[cfg(test)] const COVERED:&[oxc_ast::AstType]=&[$(oxc_ast::AstType::$kind),*];
 impl<'a> Visit<'a> for Children<'a>{$(fn $visit(&mut self,it:&$ty){self.push(AstKind::$kind(self.alloc(it)),|children,kind|{let AstKind::$kind(it)=kind else{unreachable!()};walk::$walk(children,it);});})*
 fn visit_function(&mut self,it:&Function<'a>,_flags:ScopeFlags){self.push(AstKind::Function(self.alloc(it)),|children,kind|{let AstKind::Function(it)=kind else{unreachable!()};walk::walk_function(children,it,ScopeFlags::Function);});}
 }
};}
nodes! {
 (AssignmentTargetPropertyIdentifier,visit_assignment_target_property_identifier,walk_assignment_target_property_identifier,AssignmentTargetPropertyIdentifier<'a>),
 (AssignmentTargetPropertyProperty,visit_assignment_target_property_property,walk_assignment_target_property_property,AssignmentTargetPropertyProperty<'a>),
 (TSConstructSignatureDeclaration,visit_ts_construct_signature_declaration,walk_ts_construct_signature_declaration,TSConstructSignatureDeclaration<'a>),
 (Program,visit_program,walk_program,Program<'a>),
 (IdentifierName,visit_identifier_name,walk_identifier_name,IdentifierName<'a>),
 (IdentifierReference,visit_identifier_reference,walk_identifier_reference,IdentifierReference<'a>),
 (BindingIdentifier,visit_binding_identifier,walk_binding_identifier,BindingIdentifier<'a>),
 (LabelIdentifier,visit_label_identifier,walk_label_identifier,LabelIdentifier<'a>),
 (ThisExpression,visit_this_expression,walk_this_expression,ThisExpression),
 (ArrayExpression,visit_array_expression,walk_array_expression,ArrayExpression<'a>),
 (Elision,visit_elision,walk_elision,Elision),
 (ObjectExpression,visit_object_expression,walk_object_expression,ObjectExpression<'a>),
 (ObjectProperty,visit_object_property,walk_object_property,ObjectProperty<'a>),
 (TemplateLiteral,visit_template_literal,walk_template_literal,TemplateLiteral<'a>),
 (TaggedTemplateExpression,visit_tagged_template_expression,walk_tagged_template_expression,TaggedTemplateExpression<'a>),
 (TemplateElement,visit_template_element,walk_template_element,TemplateElement<'a>),
 (ComputedMemberExpression,visit_computed_member_expression,walk_computed_member_expression,ComputedMemberExpression<'a>),
 (StaticMemberExpression,visit_static_member_expression,walk_static_member_expression,StaticMemberExpression<'a>),
 (PrivateFieldExpression,visit_private_field_expression,walk_private_field_expression,PrivateFieldExpression<'a>),
 (CallExpression,visit_call_expression,walk_call_expression,CallExpression<'a>),
 (NewExpression,visit_new_expression,walk_new_expression,NewExpression<'a>),
 (ImportMeta,visit_import_meta,walk_import_meta,ImportMeta),
 (NewTarget,visit_new_target,walk_new_target,NewTarget),
 (SpreadElement,visit_spread_element,walk_spread_element,SpreadElement<'a>),
 (UpdateExpression,visit_update_expression,walk_update_expression,UpdateExpression<'a>),
 (UnaryExpression,visit_unary_expression,walk_unary_expression,UnaryExpression<'a>),
 (BinaryExpression,visit_binary_expression,walk_binary_expression,BinaryExpression<'a>),
 (PrivateInExpression,visit_private_in_expression,walk_private_in_expression,PrivateInExpression<'a>),
 (LogicalExpression,visit_logical_expression,walk_logical_expression,LogicalExpression<'a>),
 (ConditionalExpression,visit_conditional_expression,walk_conditional_expression,ConditionalExpression<'a>),
 (AssignmentExpression,visit_assignment_expression,walk_assignment_expression,AssignmentExpression<'a>),
 (ArrayAssignmentTarget,visit_array_assignment_target,walk_array_assignment_target,ArrayAssignmentTarget<'a>),
 (ObjectAssignmentTarget,visit_object_assignment_target,walk_object_assignment_target,ObjectAssignmentTarget<'a>),
 (AssignmentTargetRest,visit_assignment_target_rest,walk_assignment_target_rest,AssignmentTargetRest<'a>),
 (AssignmentTargetWithDefault,visit_assignment_target_with_default,walk_assignment_target_with_default,AssignmentTargetWithDefault<'a>),
 (SequenceExpression,visit_sequence_expression,walk_sequence_expression,SequenceExpression<'a>),
 (Super,visit_super,walk_super,Super),
 (AwaitExpression,visit_await_expression,walk_await_expression,AwaitExpression<'a>),
 (ChainExpression,visit_chain_expression,walk_chain_expression,ChainExpression<'a>),
 (ParenthesizedExpression,visit_parenthesized_expression,walk_parenthesized_expression,ParenthesizedExpression<'a>),
 (Directive,visit_directive,walk_directive,Directive<'a>),
 (Hashbang,visit_hashbang,walk_hashbang,Hashbang<'a>),
 (BlockStatement,visit_block_statement,walk_block_statement,BlockStatement<'a>),
 (VariableDeclaration,visit_variable_declaration,walk_variable_declaration,VariableDeclaration<'a>),
 (VariableDeclarator,visit_variable_declarator,walk_variable_declarator,VariableDeclarator<'a>),
 (EmptyStatement,visit_empty_statement,walk_empty_statement,EmptyStatement),
 (ExpressionStatement,visit_expression_statement,walk_expression_statement,ExpressionStatement<'a>),
 (IfStatement,visit_if_statement,walk_if_statement,IfStatement<'a>),
 (DoWhileStatement,visit_do_while_statement,walk_do_while_statement,DoWhileStatement<'a>),
 (WhileStatement,visit_while_statement,walk_while_statement,WhileStatement<'a>),
 (ForStatement,visit_for_statement,walk_for_statement,ForStatement<'a>),
 (ForInStatement,visit_for_in_statement,walk_for_in_statement,ForInStatement<'a>),
 (ForOfStatement,visit_for_of_statement,walk_for_of_statement,ForOfStatement<'a>),
 (ContinueStatement,visit_continue_statement,walk_continue_statement,ContinueStatement<'a>),
 (BreakStatement,visit_break_statement,walk_break_statement,BreakStatement<'a>),
 (ReturnStatement,visit_return_statement,walk_return_statement,ReturnStatement<'a>),
 (WithStatement,visit_with_statement,walk_with_statement,WithStatement<'a>),
 (SwitchStatement,visit_switch_statement,walk_switch_statement,SwitchStatement<'a>),
 (SwitchCase,visit_switch_case,walk_switch_case,SwitchCase<'a>),
 (LabeledStatement,visit_labeled_statement,walk_labeled_statement,LabeledStatement<'a>),
 (ThrowStatement,visit_throw_statement,walk_throw_statement,ThrowStatement<'a>),
 (TryStatement,visit_try_statement,walk_try_statement,TryStatement<'a>),
 (CatchClause,visit_catch_clause,walk_catch_clause,CatchClause<'a>),
 (CatchParameter,visit_catch_parameter,walk_catch_parameter,CatchParameter<'a>),
 (DebuggerStatement,visit_debugger_statement,walk_debugger_statement,DebuggerStatement),
 (AssignmentPattern,visit_assignment_pattern,walk_assignment_pattern,AssignmentPattern<'a>),
 (ObjectPattern,visit_object_pattern,walk_object_pattern,ObjectPattern<'a>),
 (BindingProperty,visit_binding_property,walk_binding_property,BindingProperty<'a>),
 (ArrayPattern,visit_array_pattern,walk_array_pattern,ArrayPattern<'a>),
 (BindingRestElement,visit_binding_rest_element,walk_binding_rest_element,BindingRestElement<'a>),
 (FormalParameters,visit_formal_parameters,walk_formal_parameters,FormalParameters<'a>),
 (FormalParameter,visit_formal_parameter,walk_formal_parameter,FormalParameter<'a>),
 (FormalParameterRest,visit_formal_parameter_rest,walk_formal_parameter_rest,FormalParameterRest<'a>),
 (FunctionBody,visit_function_body,walk_function_body,FunctionBody<'a>),
 (ArrowFunctionExpression,visit_arrow_function_expression,walk_arrow_function_expression,ArrowFunctionExpression<'a>),
 (YieldExpression,visit_yield_expression,walk_yield_expression,YieldExpression<'a>),
 (Class,visit_class,walk_class,Class<'a>),
 (ClassBody,visit_class_body,walk_class_body,ClassBody<'a>),
 (MethodDefinition,visit_method_definition,walk_method_definition,MethodDefinition<'a>),
 (PropertyDefinition,visit_property_definition,walk_property_definition,PropertyDefinition<'a>),
 (PrivateIdentifier,visit_private_identifier,walk_private_identifier,PrivateIdentifier<'a>),
 (StaticBlock,visit_static_block,walk_static_block,StaticBlock<'a>),
 (AccessorProperty,visit_accessor_property,walk_accessor_property,AccessorProperty<'a>),
 (ImportExpression,visit_import_expression,walk_import_expression,ImportExpression<'a>),
 (ImportDeclaration,visit_import_declaration,walk_import_declaration,ImportDeclaration<'a>),
 (ImportSpecifier,visit_import_specifier,walk_import_specifier,ImportSpecifier<'a>),
 (ImportDefaultSpecifier,visit_import_default_specifier,walk_import_default_specifier,ImportDefaultSpecifier<'a>),
 (ImportNamespaceSpecifier,visit_import_namespace_specifier,walk_import_namespace_specifier,ImportNamespaceSpecifier<'a>),
 (WithClause,visit_with_clause,walk_with_clause,WithClause<'a>),
 (ImportAttribute,visit_import_attribute,walk_import_attribute,ImportAttribute<'a>),
 (ExportDeclaration,visit_export_declaration,walk_export_declaration,ExportDeclaration<'a>),
 (ExportNamedDeclaration,visit_export_named_declaration,walk_export_named_declaration,ExportNamedDeclaration<'a>),
 (ExportFromDeclaration,visit_export_from_declaration,walk_export_from_declaration,ExportFromDeclaration<'a>),
 (ExportDefaultDeclaration,visit_export_default_declaration,walk_export_default_declaration,ExportDefaultDeclaration<'a>),
 (ExportAllDeclaration,visit_export_all_declaration,walk_export_all_declaration,ExportAllDeclaration<'a>),
 (ExportSpecifier,visit_export_specifier,walk_export_specifier,ExportSpecifier<'a>),
 (V8IntrinsicExpression,visit_v8_intrinsic_expression,walk_v8_intrinsic_expression,V8IntrinsicExpression<'a>),
 (BooleanLiteral,visit_boolean_literal,walk_boolean_literal,BooleanLiteral),
 (NullLiteral,visit_null_literal,walk_null_literal,NullLiteral),
 (NumericLiteral,visit_numeric_literal,walk_numeric_literal,NumericLiteral<'a>),
 (StringLiteral,visit_string_literal,walk_string_literal,StringLiteral<'a>),
 (BigIntLiteral,visit_big_int_literal,walk_big_int_literal,BigIntLiteral<'a>),
 (RegExpLiteral,visit_reg_exp_literal,walk_reg_exp_literal,RegExpLiteral<'a>),
 (JSXElement,visit_jsx_element,walk_jsx_element,JSXElement<'a>),
 (JSXOpeningElement,visit_jsx_opening_element,walk_jsx_opening_element,JSXOpeningElement<'a>),
 (JSXClosingElement,visit_jsx_closing_element,walk_jsx_closing_element,JSXClosingElement<'a>),
 (JSXFragment,visit_jsx_fragment,walk_jsx_fragment,JSXFragment<'a>),
 (JSXOpeningFragment,visit_jsx_opening_fragment,walk_jsx_opening_fragment,JSXOpeningFragment),
 (JSXClosingFragment,visit_jsx_closing_fragment,walk_jsx_closing_fragment,JSXClosingFragment),
 (JSXNamespacedName,visit_jsx_namespaced_name,walk_jsx_namespaced_name,JSXNamespacedName<'a>),
 (JSXMemberExpression,visit_jsx_member_expression,walk_jsx_member_expression,JSXMemberExpression<'a>),
 (JSXExpressionContainer,visit_jsx_expression_container,walk_jsx_expression_container,JSXExpressionContainer<'a>),
 (JSXEmptyExpression,visit_jsx_empty_expression,walk_jsx_empty_expression,JSXEmptyExpression),
 (JSXAttribute,visit_jsx_attribute,walk_jsx_attribute,JSXAttribute<'a>),
 (JSXSpreadAttribute,visit_jsx_spread_attribute,walk_jsx_spread_attribute,JSXSpreadAttribute<'a>),
 (JSXIdentifier,visit_jsx_identifier,walk_jsx_identifier,JSXIdentifier<'a>),
 (JSXSpreadChild,visit_jsx_spread_child,walk_jsx_spread_child,JSXSpreadChild<'a>),
 (JSXText,visit_jsx_text,walk_jsx_text,JSXText<'a>),
 (TSThisParameter,visit_ts_this_parameter,walk_ts_this_parameter,TSThisParameter<'a>),
 (TSEnumDeclaration,visit_ts_enum_declaration,walk_ts_enum_declaration,TSEnumDeclaration<'a>),
 (TSEnumBody,visit_ts_enum_body,walk_ts_enum_body,TSEnumBody<'a>),
 (TSEnumMember,visit_ts_enum_member,walk_ts_enum_member,TSEnumMember<'a>),
 (TSTypeAnnotation,visit_ts_type_annotation,walk_ts_type_annotation,TSTypeAnnotation<'a>),
 (TSLiteralType,visit_ts_literal_type,walk_ts_literal_type,TSLiteralType<'a>),
 (TSConditionalType,visit_ts_conditional_type,walk_ts_conditional_type,TSConditionalType<'a>),
 (TSUnionType,visit_ts_union_type,walk_ts_union_type,TSUnionType<'a>),
 (TSIntersectionType,visit_ts_intersection_type,walk_ts_intersection_type,TSIntersectionType<'a>),
 (TSParenthesizedType,visit_ts_parenthesized_type,walk_ts_parenthesized_type,TSParenthesizedType<'a>),
 (TSTypeOperator,visit_ts_type_operator,walk_ts_type_operator,TSTypeOperator<'a>),
 (TSArrayType,visit_ts_array_type,walk_ts_array_type,TSArrayType<'a>),
 (TSIndexedAccessType,visit_ts_indexed_access_type,walk_ts_indexed_access_type,TSIndexedAccessType<'a>),
 (TSTupleType,visit_ts_tuple_type,walk_ts_tuple_type,TSTupleType<'a>),
 (TSNamedTupleMember,visit_ts_named_tuple_member,walk_ts_named_tuple_member,TSNamedTupleMember<'a>),
 (TSOptionalType,visit_ts_optional_type,walk_ts_optional_type,TSOptionalType<'a>),
 (TSRestType,visit_ts_rest_type,walk_ts_rest_type,TSRestType<'a>),
 (TSAnyKeyword,visit_ts_any_keyword,walk_ts_any_keyword,TSAnyKeyword),
 (TSStringKeyword,visit_ts_string_keyword,walk_ts_string_keyword,TSStringKeyword),
 (TSBooleanKeyword,visit_ts_boolean_keyword,walk_ts_boolean_keyword,TSBooleanKeyword),
 (TSNumberKeyword,visit_ts_number_keyword,walk_ts_number_keyword,TSNumberKeyword),
 (TSNeverKeyword,visit_ts_never_keyword,walk_ts_never_keyword,TSNeverKeyword),
 (TSIntrinsicKeyword,visit_ts_intrinsic_keyword,walk_ts_intrinsic_keyword,TSIntrinsicKeyword),
 (TSUnknownKeyword,visit_ts_unknown_keyword,walk_ts_unknown_keyword,TSUnknownKeyword),
 (TSNullKeyword,visit_ts_null_keyword,walk_ts_null_keyword,TSNullKeyword),
 (TSUndefinedKeyword,visit_ts_undefined_keyword,walk_ts_undefined_keyword,TSUndefinedKeyword),
 (TSVoidKeyword,visit_ts_void_keyword,walk_ts_void_keyword,TSVoidKeyword),
 (TSSymbolKeyword,visit_ts_symbol_keyword,walk_ts_symbol_keyword,TSSymbolKeyword),
 (TSThisType,visit_ts_this_type,walk_ts_this_type,TSThisType),
 (TSObjectKeyword,visit_ts_object_keyword,walk_ts_object_keyword,TSObjectKeyword),
 (TSBigIntKeyword,visit_ts_big_int_keyword,walk_ts_big_int_keyword,TSBigIntKeyword),
 (TSTypeReference,visit_ts_type_reference,walk_ts_type_reference,TSTypeReference<'a>),
 (TSQualifiedName,visit_ts_qualified_name,walk_ts_qualified_name,TSQualifiedName<'a>),
 (TSTypeParameterInstantiation,visit_ts_type_parameter_instantiation,walk_ts_type_parameter_instantiation,TSTypeParameterInstantiation<'a>),
 (TSTypeParameter,visit_ts_type_parameter,walk_ts_type_parameter,TSTypeParameter<'a>),
 (TSTypeParameterDeclaration,visit_ts_type_parameter_declaration,walk_ts_type_parameter_declaration,TSTypeParameterDeclaration<'a>),
 (TSTypeAliasDeclaration,visit_ts_type_alias_declaration,walk_ts_type_alias_declaration,TSTypeAliasDeclaration<'a>),
 (TSClassImplements,visit_ts_class_implements,walk_ts_class_implements,TSClassImplements<'a>),
 (TSInterfaceDeclaration,visit_ts_interface_declaration,walk_ts_interface_declaration,TSInterfaceDeclaration<'a>),
 (TSInterfaceBody,visit_ts_interface_body,walk_ts_interface_body,TSInterfaceBody<'a>),
 (TSPropertySignature,visit_ts_property_signature,walk_ts_property_signature,TSPropertySignature<'a>),
 (TSIndexSignature,visit_ts_index_signature,walk_ts_index_signature,TSIndexSignature<'a>),
 (TSCallSignatureDeclaration,visit_ts_call_signature_declaration,walk_ts_call_signature_declaration,TSCallSignatureDeclaration<'a>),
 (TSMethodSignature,visit_ts_method_signature,walk_ts_method_signature,TSMethodSignature<'a>),
 (TSIndexSignatureName,visit_ts_index_signature_name,walk_ts_index_signature_name,TSIndexSignatureName<'a>),
 (TSInterfaceHeritage,visit_ts_interface_heritage,walk_ts_interface_heritage,TSInterfaceHeritage<'a>),
 (TSTypePredicate,visit_ts_type_predicate,walk_ts_type_predicate,TSTypePredicate<'a>),
 (TSExternalModuleDeclaration,visit_ts_external_module_declaration,walk_ts_external_module_declaration,TSExternalModuleDeclaration<'a>),
 (TSNamespaceDeclaration,visit_ts_namespace_declaration,walk_ts_namespace_declaration,TSNamespaceDeclaration<'a>),
 (TSGlobalDeclaration,visit_ts_global_declaration,walk_ts_global_declaration,TSGlobalDeclaration<'a>),
 (TSModuleBlock,visit_ts_module_block,walk_ts_module_block,TSModuleBlock<'a>),
 (TSTypeLiteral,visit_ts_type_literal,walk_ts_type_literal,TSTypeLiteral<'a>),
 (TSInferType,visit_ts_infer_type,walk_ts_infer_type,TSInferType<'a>),
 (TSTypeQuery,visit_ts_type_query,walk_ts_type_query,TSTypeQuery<'a>),
 (TSImportType,visit_ts_import_type,walk_ts_import_type,TSImportType<'a>),
 (TSImportTypeQualifiedName,visit_ts_import_type_qualified_name,walk_ts_import_type_qualified_name,TSImportTypeQualifiedName<'a>),
 (TSFunctionType,visit_ts_function_type,walk_ts_function_type,TSFunctionType<'a>),
 (TSConstructorType,visit_ts_constructor_type,walk_ts_constructor_type,TSConstructorType<'a>),
 (TSMappedType,visit_ts_mapped_type,walk_ts_mapped_type,TSMappedType<'a>),
 (TSTemplateLiteralType,visit_ts_template_literal_type,walk_ts_template_literal_type,TSTemplateLiteralType<'a>),
 (TSAsExpression,visit_ts_as_expression,walk_ts_as_expression,TSAsExpression<'a>),
 (TSSatisfiesExpression,visit_ts_satisfies_expression,walk_ts_satisfies_expression,TSSatisfiesExpression<'a>),
 (TSTypeAssertion,visit_ts_type_assertion,walk_ts_type_assertion,TSTypeAssertion<'a>),
 (TSImportEqualsDeclaration,visit_ts_import_equals_declaration,walk_ts_import_equals_declaration,TSImportEqualsDeclaration<'a>),
 (TSExternalModuleReference,visit_ts_external_module_reference,walk_ts_external_module_reference,TSExternalModuleReference<'a>),
 (TSNonNullExpression,visit_ts_non_null_expression,walk_ts_non_null_expression,TSNonNullExpression<'a>),
 (Decorator,visit_decorator,walk_decorator,Decorator<'a>),
 (TSExportAssignment,visit_ts_export_assignment,walk_ts_export_assignment,TSExportAssignment<'a>),
 (TSNamespaceExportDeclaration,visit_ts_namespace_export_declaration,walk_ts_namespace_export_declaration,TSNamespaceExportDeclaration<'a>),
 (TSInstantiationExpression,visit_ts_instantiation_expression,walk_ts_instantiation_expression,TSInstantiationExpression<'a>),
 (JSDocNullableType,visit_js_doc_nullable_type,walk_js_doc_nullable_type,JSDocNullableType<'a>),
 (JSDocNonNullableType,visit_js_doc_non_nullable_type,walk_js_doc_non_nullable_type,JSDocNonNullableType<'a>),
 (JSDocUnknownType,visit_js_doc_unknown_type,walk_js_doc_unknown_type,JSDocUnknownType),
}

pub(crate) fn check(
    program: &Program<'_>,
    input: &SourceInput,
    filename: &str,
    limits: &CompileLimits,
) -> Result<(), Vec<Diagnostic>> {
    let mut children = Children {
        pending: Vec::new(),
        depth: 0,
        limits: *limits,
        nodes: 0,
        functions: 0,
        bindings: 0,
        failure: None,
    };
    children.visit_program(program);
    while children.failure.is_none() {
        let Some(item) = children.pending.pop() else {
            break;
        };
        children.depth = item.depth;
        let first = children.pending.len();
        (item.expand)(&mut children, item.kind);
        children.pending[first..].reverse();
    }
    if let Some((span, message)) = children.failure {
        Err(vec![diagnostic(filename, input, span, message)])
    } else {
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn every_pinned_ast_kind_has_a_nonrecursive_adapter() {
        let mut kinds: Vec<_> = super::COVERED.iter().map(|kind| *kind as u8).collect();
        kinds.push(oxc_ast::AstType::Function as u8);
        kinds.sort_unstable();
        assert_eq!(
            kinds,
            (0..=oxc_ast::ast_kind::AST_TYPE_MAX).collect::<Vec<_>>()
        );
    }
}
