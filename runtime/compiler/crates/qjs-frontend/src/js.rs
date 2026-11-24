//! OXC lowering for general script semantics. No source regeneration or C parser.
//! Cooked literals use OXC JSStr::encode_utf16, retaining unpaired surrogates.
use crate::{Category, CompileLimits, Diagnostic, ParsePhase, source::SourceInput};
use oxc_ast::ast;
use oxc_ast_visit::Visit;
use oxc_span::GetSpan;
use qjs_ir::js as ir;
#[path = "class_lowering.rs"]
mod class_lowering;
#[path = "function_source.rs"]
mod function_source;
#[path = "parameter_semantics.rs"]
mod parameter_semantics;
#[derive(Clone, Copy, Debug)]
pub enum SourceMode {
    Script,
    Module,
}
pub fn compile_program(
    source: &str,
    filename: &str,
    mode: SourceMode,
) -> Result<ir::Program, Vec<Diagnostic>> {
    let input =
        SourceInput::from_utf16(&source.encode_utf16().collect::<Vec<_>>()).map_err(|message| {
            vec![Diagnostic {
                category: Category::UnsupportedSemantics,
                filename: filename.into(),
                span: qjs_ir::Span { start: 0, end: 0 },
                message: message.into(),
            }]
        })?;
    compile_input(
        &input,
        filename,
        mode,
        false,
        &CompileLimits::default(),
        None,
    )
}
pub fn compile_program_input(
    input: &SourceInput,
    filename: &str,
) -> Result<ir::Program, Vec<Diagnostic>> {
    compile_input(
        input,
        filename,
        SourceMode::Script,
        false,
        &CompileLimits::default(),
        None,
    )
}
/// Parse owned UTF-16 source using the module grammar and strict semantics.
pub fn compile_module_input(
    input: &SourceInput,
    filename: &str,
) -> Result<ir::Program, Vec<Diagnostic>> {
    compile_input(
        input,
        filename,
        SourceMode::Module,
        false,
        &CompileLimits::default(),
        None,
    )
}
/// Lower eval source in its caller's strict Script context without changing owned source.
pub fn compile_eval_program_input(
    input: &SourceInput,
    filename: &str,
    inherited_strict: bool,
) -> Result<ir::Program, Vec<Diagnostic>> {
    compile_input(
        input,
        filename,
        SourceMode::Script,
        inherited_strict,
        &CompileLimits::default(),
        None,
    )
}
fn compile_input(
    input: &SourceInput,
    filename: &str,
    mode: SourceMode,
    inherited_strict: bool,
    limits: &CompileLimits,
    observer: Option<&mut dyn FnMut(ParsePhase)>,
) -> Result<ir::Program, Vec<Diagnostic>> {
    compile_input_in(
        input,
        filename,
        mode,
        inherited_strict,
        false,
        &[],
        limits,
        observer,
    )
}
/// `eval_new_target`: direct-eval code whose caller frame has `new.target`,
/// so `new.target` is valid at the eval program's top level.
#[allow(
    clippy::too_many_arguments,
    reason = "the one internal entry every public compile function narrows to"
)]
fn compile_input_in(
    input: &SourceInput,
    filename: &str,
    mode: SourceMode,
    inherited_strict: bool,
    eval_new_target: bool,
    outer_private: &[String],
    limits: &CompileLimits,
    mut observer: Option<&mut dyn FnMut(ParsePhase)>,
) -> Result<ir::Program, Vec<Diagnostic>> {
    let admitted = |e: &oxc_diagnostics::OxcDiagnostic| {
        let text = e.to_string();
        !(eval_new_target && text.contains("new.target")
            || outer_private.iter().any(|name| {
                text == format!("Private field '#{name}' must be declared in an enclosing class")
                    || text
                        == format!(
                            "Private identifier '#{name}' is not allowed outside class bodies"
                        )
            }))
    };
    crate::budget::validate_limits(limits, input, filename)?;
    // Declared before the arena: the AST parsed from a rewritten view borrows its text.
    let call_target_view;
    let arena = oxc_allocator::Allocator::default();
    let source_type = match mode {
        SourceMode::Script => oxc_span::SourceType::script(),
        SourceMode::Module => oxc_span::SourceType::mjs(),
    };
    let mut parsed = parse(&arena, input.parser_text(), source_type, &mut observer);
    let input = if matches!(mode, SourceMode::Script)
        && !inherited_strict
        && call_target_end(&parsed.diagnostics, input).is_some()
    {
        call_target_view = call_targets(input, source_type, &mut observer);
        parsed = parse(
            &arena,
            call_target_view.parser_text(),
            source_type,
            &mut observer,
        );
        &call_target_view
    } else {
        input
    };
    let report = |category, e: &oxc_diagnostics::OxcDiagnostic| Diagnostic {
        category,
        filename: filename.into(),
        span: {
            let span = crate::error_span(e);
            let original = input
                .original_span(span.start as usize..span.end as usize)
                .expect("diagnostic span in parse view");
            qjs_ir::Span {
                start: original.start,
                end: original.end,
            }
        },
        message: e.to_string(),
    };
    if parsed.diagnostics.iter().any(admitted) {
        return Err(parsed
            .diagnostics
            .iter()
            .filter(|e| admitted(e))
            .map(|e| report(Category::Parse, e))
            .collect());
    }
    crate::budget::check(&parsed.program, input, filename, limits)?;
    let synthetic_strict = inherited_strict && !parsed.program.has_use_strict_directive();
    if synthetic_strict {
        // OXC has no public inherited-strict parser option. Validate the same Script
        // with a parser-only directive, retaining the original AST for all emission.
        let text = input.parser_text();
        let insertion = if text.starts_with("#!") {
            text.char_indices()
                .find(|(_, c)| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
                .map_or(text.len(), |(at, c)| {
                    let mut end = at + c.len_utf8();
                    if c == '\r' && text.as_bytes().get(end) == Some(&b'\n') {
                        end += 1;
                    }
                    end
                })
        } else {
            0
        };
        let prefix = if text.starts_with("#!")
            && insertion == text.len()
            && !text.ends_with(['\n', '\r', '\u{2028}', '\u{2029}'])
        {
            "\n'use strict';\n"
        } else {
            "'use strict';\n"
        };
        let mut validation_text = String::with_capacity(text.len() + prefix.len());
        validation_text.push_str(&text[..insertion]);
        validation_text.push_str(prefix);
        validation_text.push_str(&text[insertion..]);
        let validation_arena = oxc_allocator::Allocator::default();
        let validation = parse(
            &validation_arena,
            &validation_text,
            oxc_span::SourceType::script(),
            &mut observer,
        );
        if validation.diagnostics.iter().any(admitted) {
            let original_offset = |offset: u32| {
                let offset = offset as usize;
                if offset <= insertion {
                    offset
                } else if offset >= insertion + prefix.len() {
                    offset - prefix.len()
                } else {
                    insertion
                }
            };
            return Err(validation
                .diagnostics
                .iter()
                .filter(|e| admitted(e))
                .map(|error| {
                    let span = crate::error_span(error);
                    let span = input
                        .original_span(original_offset(span.start)..original_offset(span.end))
                        .expect("strict validation span in original source");
                    Diagnostic {
                        category: Category::Parse,
                        filename: filename.into(),
                        span: qjs_ir::Span {
                            start: span.start,
                            end: span.end,
                        },
                        message: error.to_string(),
                    }
                })
                .collect());
        }
        let mut directive_program = parse(
            &arena,
            "'use strict';",
            oxc_span::SourceType::script(),
            &mut observer,
        );
        let mut directive = directive_program.program.directives.remove(0);
        directive.span = oxc_span::Span::new(0, 0);
        directive.expression.span = oxc_span::Span::new(0, 0);
        parsed.program.directives.insert(0, directive);
    }
    let semantic = oxc_semantic::SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&parsed.program);
    // Strict code keeps the early SyntaxError the parser gave before the suffix.
    let strict_call_target = semantic.semantic.nodes().iter().find_map(|node| {
        let oxc_ast::AstKind::StaticMemberExpression(member) = node.kind() else {
            return None;
        };
        (input.is_inserted(member.property.span.start..member.property.span.end)
            && semantic
                .semantic
                .scoping()
                .scope_flags(node.scope_id())
                .is_strict_mode())
        .then(|| oxc_span::GetSpan::span(&member.object))
    });
    if let Some(span) = strict_call_target {
        let span = input
            .original_span(span.start as usize..span.end as usize)
            .expect("call target span in parse view");
        return Err(vec![Diagnostic {
            category: Category::Parse,
            filename: filename.into(),
            span: qjs_ir::Span {
                start: span.start,
                end: span.end,
            },
            message: "Cannot assign to this expression".into(),
        }]);
    }
    if semantic.diagnostics.iter().any(admitted) {
        return Err(semantic
            .diagnostics
            .iter()
            .filter(|e| admitted(e))
            .map(|e| report(Category::Semantic, e))
            .collect());
    }
    let root_this = ir::Binding {
        symbol: semantic.semantic.scoping().symbols_len() as u32,
        scope: 0,
        name: "this".into(),
        kind: ir::DeclarationKind::Var,
        span: ir::Span {
            start: 0,
            end: input.original().len() as u32,
        },
    };
    let lower = Lower {
        input,
        filename,
        scoping: semantic.semantic.scoping(),
        next_symbol: std::cell::Cell::new(semantic.semantic.scoping().symbols_len() as u32 + 2),
        argument_frames: std::cell::RefCell::new(Vec::new()),
        catch_frames: std::cell::RefCell::new(Vec::new()),
        own_name_frames: std::cell::RefCell::new(Vec::new()),
        body_var_frames: std::cell::RefCell::new(Vec::new()),
        prepass_remaining: std::cell::Cell::new(limits.max_ast_nodes),
        function_scopes: std::cell::RefCell::new(vec![0]),
        this_bindings: std::cell::RefCell::new(vec![root_this.clone()]),
        this_owners: std::cell::RefCell::new(vec![0]),
        this_uses: std::cell::RefCell::new(vec![false]),
        new_target_bindings: std::cell::RefCell::new(vec![eval_new_target.then(|| ir::Binding {
            symbol: semantic.semantic.scoping().symbols_len() as u32 + 1,
            scope: 0,
            name: "new.target".into(),
            kind: ir::DeclarationKind::Var,
            span: ir::Span { start: 0, end: 0 },
        })]),
        new_target_uses: std::cell::RefCell::new(vec![false]),
        direct_eval_uses: std::cell::RefCell::new(vec![false]),
        super_frames: std::cell::RefCell::new(vec![None]),
        home_object_uses: std::cell::RefCell::new(vec![false]),
        arguments_permissions: std::cell::RefCell::new(vec![true]),
        class_name_frames: std::cell::RefCell::new(Vec::new()),
        // Eval code sees the caller frame's private names as ids 0..n.
        private_frames: std::cell::RefCell::new(if outer_private.is_empty() {
            Vec::new()
        } else {
            vec![
                outer_private
                    .iter()
                    .enumerate()
                    .map(|(id, name)| (name.clone(), id as u32))
                    .collect(),
            ]
        }),
        next_private_id: std::cell::Cell::new(outer_private.len() as u32),
        next_scope: std::cell::Cell::new(semantic.semantic.scoping().scopes_len() as u32),
        extra_scopes: std::cell::RefCell::new(Vec::new()),
    };
    let (mut body, module) = match mode {
        SourceMode::Script => (
            parsed
                .program
                .body
                .iter()
                .map(|s| lower.statement(s))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| vec![e])?,
            None,
        ),
        SourceMode::Module => {
            let (body, module) = lower.module(&parsed.program.body).map_err(|e| vec![e])?;
            (body, Some(module))
        }
    };
    let mut directives: Vec<_> = parsed
        .program
        .directives
        .iter()
        .skip(usize::from(synthetic_strict))
        .map(|directive| ir::Statement {
            span: lower.span(directive.span),
            kind: ir::StatementKind::Expression(ir::Expression {
                span: lower.span(directive.expression.span),
                kind: ir::ExpressionKind::Literal(ir::Literal::String(
                    directive.expression.value.encode_utf16().collect(),
                )),
            }),
        })
        .collect();
    directives.append(&mut body);
    let body = directives;
    let mut scopes: Vec<_> = lower
        .scoping
        .scope_descendants_from_root()
        .map(|id| ir::Scope {
            id: id.index() as u32,
            parent: lower.scoping.scope_parent_id(id).map(|p| p.index() as u32),
            strict: lower.scoping.scope_flags(id).is_strict_mode(),
            function: lower.scoping.scope_flags(id).is_function(),
        })
        .collect();
    scopes.extend(lower.extra_scopes.borrow().iter().cloned());
    let uses_this = lower.this_uses.borrow()[0];
    Ok(ir::Program {
        this_binding: Some(root_this),
        uses_this,
        module,
        scopes,
        filename: filename.into(),
        source: input.original().to_vec(),
        body,
        strict: matches!(mode, SourceMode::Module)
            || parsed
                .program
                .directives
                .iter()
                .any(|d| d.directive == "use strict"),
        scope: 0,
    })
}
// The arguments object lives in the function environment, inside a named
// function expression's name environment. Keep their symbol identities apart.
struct ArgumentFrame {
    scope: u32,
    binding: Option<ir::Binding>,
    hidden_name_symbol: Option<u32>,
}
// OXC merges catch/var redeclarations and can move their shared symbol to the
// var scope. Keep the real catch environment separate from that hoisted binding.
struct CatchFrame {
    name: String,
    semantic_symbol: u32,
    binding: ir::Binding,
    owner_scope: u32,
}
struct OwnNameFrame {
    semantic_symbol: u32,
    binding: ir::Binding,
}
// The original AST has already passed admission. This additional, aggregate-
// bounded scan visits only the current function's statement/var environment;
// expressions and nested function bodies cannot contribute hoisted body vars.
struct OwnNameScan<'s> {
    name: &'s str,
    owner_scope: u32,
    scoping: &'s oxc_semantic::Scoping,
    remaining: usize,
    exhausted: bool,
    found: bool,
}
impl OwnNameScan<'_> {
    fn charge(&mut self) -> bool {
        if self.found || self.exhausted {
            return false;
        }
        if let Some(remaining) = self.remaining.checked_sub(1) {
            self.remaining = remaining;
            true
        } else {
            self.exhausted = true;
            false
        }
    }
}
impl<'a> Visit<'a> for OwnNameScan<'_> {
    fn visit_statement(&mut self, statement: &ast::Statement<'a>) {
        if self.charge() {
            oxc_ast_visit::walk::walk_statement(self, statement);
        }
    }
    fn visit_statements(&mut self, statements: &oxc_allocator::Vec<'a, ast::Statement<'a>>) {
        for statement in statements {
            if self.found || self.exhausted {
                break;
            }
            self.visit_statement(statement);
        }
    }
    fn visit_expression(&mut self, _: &ast::Expression<'a>) {}
    fn visit_class(&mut self, _: &ast::Class<'a>) {}
    fn visit_variable_declaration(&mut self, declaration: &ast::VariableDeclaration<'a>) {
        if declaration.kind != ast::VariableDeclarationKind::Var {
            return;
        }
        for declaration in &declaration.declarations {
            if !self.charge() {
                break;
            }
            if Lower::pattern_has_name(&declaration.id, self.name) {
                self.found = true;
            }
        }
    }
    fn visit_function(&mut self, function: &ast::Function<'a>, _: oxc_syntax::scope::ScopeFlags) {
        if function.r#type == ast::FunctionType::FunctionDeclaration
            && let Some(id) = &function.id
            && id.name == self.name
            && let Some(symbol) = id.symbol_id.get()
            && self.scoping.symbol_scope_id(symbol).index() as u32 == self.owner_scope
        {
            self.found = true;
        }
        // No descent into the nested function's environment.
    }
}
struct SourceBodyVar {
    symbols: std::collections::BTreeSet<u32>,
    span: oxc_span::Span,
}
struct BodyVarScan<'s> {
    owner_scope: u32,
    scoping: &'s oxc_semantic::Scoping,
    remaining: usize,
    exhausted: bool,
    recording: bool,
    names: std::collections::BTreeMap<String, SourceBodyVar>,
}
impl BodyVarScan<'_> {
    fn charge(&mut self) -> bool {
        if self.exhausted {
            return false;
        }
        if let Some(remaining) = self.remaining.checked_sub(1) {
            self.remaining = remaining;
            true
        } else {
            self.exhausted = true;
            false
        }
    }
    fn record(&mut self, id: &ast::BindingIdentifier<'_>) {
        if !self.charge() {
            return;
        }
        let symbol = id
            .symbol_id
            .get()
            .expect("semantic body declaration")
            .index() as u32;
        if let Some(entry) = self.names.get_mut(id.name.as_str()) {
            entry.symbols.insert(symbol);
        } else {
            self.names.insert(
                id.name.to_string(),
                SourceBodyVar {
                    symbols: std::collections::BTreeSet::from([symbol]),
                    span: id.span,
                },
            );
        }
    }
}
impl<'a> Visit<'a> for BodyVarScan<'_> {
    fn visit_statement(&mut self, statement: &ast::Statement<'a>) {
        if self.charge() {
            oxc_ast_visit::walk::walk_statement(self, statement);
        }
    }
    fn visit_statements(&mut self, statements: &oxc_allocator::Vec<'a, ast::Statement<'a>>) {
        for statement in statements {
            if self.exhausted {
                break;
            }
            self.visit_statement(statement);
        }
    }
    fn visit_expression(&mut self, _: &ast::Expression<'a>) {}
    fn visit_class(&mut self, _: &ast::Class<'a>) {}
    fn visit_variable_declaration(&mut self, declaration: &ast::VariableDeclaration<'a>) {
        if declaration.kind != ast::VariableDeclarationKind::Var {
            return;
        }
        self.recording = true;
        for declaration in &declaration.declarations {
            if self.exhausted {
                break;
            }
            self.visit_binding_pattern(&declaration.id);
        }
        self.recording = false;
    }
    fn visit_binding_identifier(&mut self, id: &ast::BindingIdentifier<'a>) {
        if self.recording {
            self.record(id);
        }
    }
    fn visit_function(&mut self, function: &ast::Function<'a>, _: oxc_syntax::scope::ScopeFlags) {
        if function.r#type == ast::FunctionType::FunctionDeclaration
            && let Some(id) = &function.id
            && let Some(symbol) = id.symbol_id.get()
            && self.scoping.symbol_scope_id(symbol).index() as u32 == self.owner_scope
        {
            self.record(id);
        }
    }
}
struct BodyVarFrame {
    scope: u32,
    by_symbol: std::collections::BTreeMap<u32, ir::Binding>,
    by_name: std::collections::BTreeMap<String, ir::Binding>,
}
#[derive(Clone)]
struct MethodContext {
    constructor: bool,
    derived: bool,
    field_initializer: Option<ir::Binding>,
}
#[derive(Clone)]
struct SuperFrame {
    home: ir::Binding,
    constructor: Option<ir::Binding>,
    field_initializer: Option<ir::Binding>,
    call_allowed: bool,
}
struct ClassNameFrame {
    semantic_symbol: u32,
    binding: ir::Binding,
}
struct Lower<'a> {
    input: &'a SourceInput,
    filename: &'a str,
    scoping: &'a oxc_semantic::Scoping,
    next_symbol: std::cell::Cell<u32>,
    argument_frames: std::cell::RefCell<Vec<ArgumentFrame>>,
    catch_frames: std::cell::RefCell<Vec<CatchFrame>>,
    own_name_frames: std::cell::RefCell<Vec<OwnNameFrame>>,
    body_var_frames: std::cell::RefCell<Vec<BodyVarFrame>>,
    prepass_remaining: std::cell::Cell<usize>,
    function_scopes: std::cell::RefCell<Vec<u32>>,
    this_bindings: std::cell::RefCell<Vec<ir::Binding>>,
    this_owners: std::cell::RefCell<Vec<usize>>,
    this_uses: std::cell::RefCell<Vec<bool>>,
    new_target_bindings: std::cell::RefCell<Vec<Option<ir::Binding>>>,
    new_target_uses: std::cell::RefCell<Vec<bool>>,
    direct_eval_uses: std::cell::RefCell<Vec<bool>>,
    super_frames: std::cell::RefCell<Vec<Option<SuperFrame>>>,
    home_object_uses: std::cell::RefCell<Vec<bool>>,
    arguments_permissions: std::cell::RefCell<Vec<bool>>,
    class_name_frames: std::cell::RefCell<Vec<ClassNameFrame>>,
    private_frames: std::cell::RefCell<Vec<std::collections::BTreeMap<String, u32>>>,
    next_private_id: std::cell::Cell<u32>,
    next_scope: std::cell::Cell<u32>,
    extra_scopes: std::cell::RefCell<Vec<ir::Scope>>,
}
type R<T> = Result<T, Diagnostic>;
impl Lower<'_> {
    fn span(&self, s: oxc_span::Span) -> ir::Span {
        let original = self
            .input
            .original_span(s.start as usize..s.end as usize)
            .expect("OXC span lies in parse view");
        ir::Span {
            start: original.start,
            end: original.end,
        }
    }
    fn unsupported(&self, s: oxc_span::Span, message: &str) -> Diagnostic {
        let s = self.span(s);
        Diagnostic {
            category: Category::UnsupportedSyntax,
            filename: self.filename.into(),
            span: qjs_ir::Span {
                start: s.start,
                end: s.end,
            },
            message: message.into(),
        }
    }
    fn binding(&self, id: &ast::BindingIdentifier<'_>, kind: ir::DeclarationKind) -> ir::Binding {
        let symbol = id.symbol_id.get().expect("semantic symbol");
        let mut binding_symbol = symbol.index() as u32;
        let mut binding_scope = self.scoping.symbol_scope_id(symbol).index() as u32;
        if id.name == "arguments"
            && matches!(
                kind,
                ir::DeclarationKind::Var | ir::DeclarationKind::Function
            )
            && let Some(ArgumentFrame {
                scope,
                binding: Some(arguments),
                hidden_name_symbol: Some(hidden),
            }) = self.argument_frames.borrow().last()
            && *hidden == binding_symbol
        {
            binding_symbol = arguments.symbol;
            binding_scope = *scope;
        }
        if matches!(
            kind,
            ir::DeclarationKind::Var | ir::DeclarationKind::Function
        ) && let Some(owner) = self.function_scopes.borrow().last()
            && let Some(frame) = self.own_name_frames.borrow().iter().rev().find(|frame| {
                frame.binding.scope == *owner && frame.binding.name == id.name.as_str()
            })
        {
            binding_symbol = frame.binding.symbol;
            binding_scope = frame.binding.scope;
        }
        if matches!(
            kind,
            ir::DeclarationKind::Var | ir::DeclarationKind::Function
        ) && let Some(scope) = self.function_scopes.borrow().last()
            && let Some(frame) = self
                .body_var_frames
                .borrow()
                .iter()
                .rev()
                .find(|frame| frame.scope == *scope)
            && let Some(binding) = frame.by_name.get(id.name.as_str())
        {
            binding_symbol = binding.symbol;
            binding_scope = binding.scope;
        }
        ir::Binding {
            symbol: binding_symbol,
            scope: binding_scope,
            name: id.name.to_string(),
            kind,
            span: self.span(id.span),
        }
    }
    fn next_symbol(&self) -> u32 {
        let symbol = self.next_symbol.get();
        self.next_symbol.set(symbol + 1);
        symbol
    }
    fn reference(&self, id: &ast::IdentifierReference<'_>) -> ir::Reference {
        let semantic = id
            .reference_id
            .get()
            .and_then(|r| self.scoping.get_reference(r).symbol_id());
        let mut symbol = semantic.map(|s| s.index() as u32);
        if id.name == "arguments"
            && let Some(ArgumentFrame {
                scope,
                binding: Some(binding),
                hidden_name_symbol,
            }) = self.argument_frames.borrow().last()
        {
            let local = semantic.is_some_and(|s| {
                self.scoping
                    .scope_ancestors(self.scoping.symbol_scope_id(s))
                    .any(|ancestor| ancestor.index() as u32 == *scope)
            });
            if !local || symbol.is_some_and(|id| Some(id) == *hidden_name_symbol) {
                symbol = Some(binding.symbol);
            }
        }
        // Match the current mapped symbol, so a nested ordinary function's
        // implicit arguments object still shadows catch (arguments).
        if let Some(frame) = self
            .catch_frames
            .borrow()
            .iter()
            .rev()
            .find(|frame| symbol == Some(frame.semantic_symbol))
        {
            symbol = Some(frame.binding.symbol);
        }
        if let Some(frame) = self
            .own_name_frames
            .borrow()
            .iter()
            .rev()
            .find(|frame| symbol == Some(frame.semantic_symbol))
        {
            symbol = Some(frame.binding.symbol);
        }
        if let Some(current) = symbol
            && let Some(binding) = self
                .body_var_frames
                .borrow()
                .iter()
                .rev()
                .find_map(|frame| frame.by_symbol.get(&current))
        {
            symbol = Some(binding.symbol);
        }
        if let Some(frame) = self
            .class_name_frames
            .borrow()
            .iter()
            .rev()
            .find(|frame| symbol == Some(frame.semantic_symbol))
        {
            symbol = Some(frame.binding.symbol);
        }
        ir::Reference {
            symbol,
            name: id.name.to_string(),
        }
    }
    fn declaration_binding(
        &self,
        id: &ast::BindingIdentifier<'_>,
        kind: ir::DeclarationKind,
    ) -> (ir::Binding, Option<ir::Reference>) {
        let mut binding = self.binding(id, kind);
        let mut initialization_target = None;
        if kind == ir::DeclarationKind::Var {
            let owner_scope = *self
                .function_scopes
                .borrow()
                .last()
                .expect("function scope");
            if let Some(frame) =
                self.catch_frames.borrow().iter().rev().find(|frame| {
                    frame.owner_scope == owner_scope && frame.name == id.name.as_str()
                })
            {
                let scope = oxc_syntax::scope::ScopeId::from_usize(owner_scope as usize);
                let symbol = self
                    .scoping
                    .get_binding(scope, id.name)
                    .expect("semantic hoisted catch var binding");
                binding.symbol = symbol.index() as u32;
                binding.scope = owner_scope;
                if let Some(own) = self.own_name_frames.borrow().iter().rev().find(|own| {
                    own.binding.scope == owner_scope && own.binding.name == id.name.as_str()
                }) {
                    binding.symbol = own.binding.symbol;
                }
                if id.name == "arguments"
                    && let Some(ArgumentFrame {
                        scope,
                        binding: Some(arguments),
                        ..
                    }) = self.argument_frames.borrow().last()
                    && *scope == owner_scope
                {
                    binding.symbol = arguments.symbol;
                }
                initialization_target = Some(ir::Reference {
                    symbol: Some(frame.binding.symbol),
                    name: frame.name.clone(),
                });
            }
        }
        if kind == ir::DeclarationKind::Var
            && let Some(scope) = self.function_scopes.borrow().last()
            && let Some(frame) = self
                .body_var_frames
                .borrow()
                .iter()
                .rev()
                .find(|frame| frame.scope == *scope)
            && let Some(body) = frame.by_name.get(id.name.as_str())
        {
            binding.symbol = body.symbol;
            binding.scope = body.scope;
        }
        (binding, initialization_target)
    }
    fn anonymous_definition(value: &ir::Expression) -> bool {
        match &value.kind {
            ir::ExpressionKind::Function(function) => function.name.is_none(),
            ir::ExpressionKind::Class(class) => class.name.is_none(),
            _ => false,
        }
    }
    fn named_value(value: ir::Expression, name: Vec<u16>) -> ir::Expression {
        if !Self::anonymous_definition(&value) {
            return value;
        }
        let span = value.span;
        ir::Expression {
            span,
            kind: ir::ExpressionKind::NamedEvaluation {
                name: Box::new(ir::Expression {
                    span,
                    kind: ir::ExpressionKind::Literal(ir::Literal::String(name)),
                }),
                value: Box::new(value),
            },
        }
    }
    fn pattern_default(pattern: ir::Pattern, value: ir::Expression) -> ir::Pattern {
        let value = match &pattern {
            ir::Pattern::Binding { binding, .. } => {
                Self::named_value(value, binding.name.encode_utf16().collect())
            }
            ir::Pattern::Target(reference) => {
                Self::named_value(value, reference.name.encode_utf16().collect())
            }
            _ => value,
        };
        ir::Pattern::Default {
            pattern: Box::new(pattern),
            value,
        }
    }
    fn binding_pattern(
        &self,
        pattern: &ast::BindingPattern<'_>,
        kind: ir::DeclarationKind,
    ) -> R<ir::Pattern> {
        Ok(match pattern {
            ast::BindingPattern::BindingIdentifier(id) => {
                let (binding, initialization_target) = self.declaration_binding(id, kind);
                ir::Pattern::Binding {
                    binding,
                    initialization_target,
                }
            }
            ast::BindingPattern::AssignmentPattern(pattern) => Self::pattern_default(
                self.binding_pattern(&pattern.left, kind)?,
                self.expression(&pattern.right)?,
            ),
            ast::BindingPattern::ArrayPattern(pattern) => ir::Pattern::Array {
                elements: pattern
                    .elements
                    .iter()
                    .map(|element| {
                        element
                            .as_ref()
                            .map(|element| self.binding_pattern(element, kind))
                            .transpose()
                    })
                    .collect::<R<_>>()?,
                rest: pattern
                    .rest
                    .as_ref()
                    .map(|rest| self.binding_pattern(&rest.argument, kind).map(Box::new))
                    .transpose()?,
            },
            ast::BindingPattern::ObjectPattern(pattern) => ir::Pattern::Object {
                properties: pattern
                    .properties
                    .iter()
                    .map(|property| {
                        Ok(ir::PatternProperty {
                            key: self.object_key(&property.key, property.computed)?,
                            pattern: self.binding_pattern(&property.value, kind)?,
                        })
                    })
                    .collect::<R<_>>()?,
                rest: pattern
                    .rest
                    .as_ref()
                    .map(|rest| self.binding_pattern(&rest.argument, kind).map(Box::new))
                    .transpose()?,
            },
        })
    }
    fn assignment_default(&self, target: &ast::AssignmentTargetMaybeDefault<'_>) -> R<ir::Pattern> {
        if let ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(default) = target {
            Ok(Self::pattern_default(
                self.assignment_pattern(&default.binding)?,
                self.expression(&default.init)?,
            ))
        } else {
            self.assignment_pattern(
                target
                    .as_assignment_target()
                    .expect("assignment pattern target"),
            )
        }
    }
    fn assignment_pattern(&self, target: &ast::AssignmentTarget<'_>) -> R<ir::Pattern> {
        if let ast::AssignmentTarget::AssignmentTargetIdentifier(id) = target {
            return Ok(ir::Pattern::Target(self.reference(id)));
        }
        if let Some(member) = target.as_member_expression() {
            if let Some(key) = self.super_key(member)? {
                return Ok(ir::Pattern::SuperMember { key });
            }
            if let ast::MemberExpression::PrivateFieldExpression(member) = member {
                return Ok(ir::Pattern::PrivateMember {
                    object: Box::new(self.expression(&member.object)?),
                    id: self.private_name(&member.field)?,
                });
            }
            let (object, key) = self.member_reference(member)?;
            return Ok(ir::Pattern::Member { object, key });
        }
        Ok(match target {
            ast::AssignmentTarget::ArrayAssignmentTarget(pattern) => ir::Pattern::Array {
                elements: pattern
                    .elements
                    .iter()
                    .map(|element| {
                        element
                            .as_ref()
                            .map(|element| self.assignment_default(element))
                            .transpose()
                    })
                    .collect::<R<_>>()?,
                rest: pattern
                    .rest
                    .as_ref()
                    .map(|rest| self.assignment_pattern(&rest.target).map(Box::new))
                    .transpose()?,
            },
            ast::AssignmentTarget::ObjectAssignmentTarget(pattern) => ir::Pattern::Object {
                properties: pattern
                    .properties
                    .iter()
                    .map(|property| {
                        Ok(match property {
                            ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(
                                property,
                            ) => {
                                let mut pattern =
                                    ir::Pattern::Target(self.reference(&property.binding));
                                if let Some(value) = &property.init {
                                    pattern =
                                        Self::pattern_default(pattern, self.expression(value)?);
                                }
                                ir::PatternProperty {
                                    key: ir::ObjectKey::Static(
                                        property.binding.name.encode_utf16().collect(),
                                    ),
                                    pattern,
                                }
                            }
                            ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(
                                property,
                            ) => ir::PatternProperty {
                                key: self.object_key(&property.name, property.computed)?,
                                pattern: self.assignment_default(&property.binding)?,
                            },
                        })
                    })
                    .collect::<R<_>>()?,
                rest: pattern
                    .rest
                    .as_ref()
                    .map(|rest| self.assignment_pattern(&rest.target).map(Box::new))
                    .transpose()?,
            },
            _ => return Err(self.unsupported(target.span(), "assignment pattern not implemented")),
        })
    }
    fn declaration(&self, declaration: &ast::VariableDeclaration<'_>) -> R<ir::StatementKind> {
        let (kind, using) = match declaration.kind {
            ast::VariableDeclarationKind::Var => (ir::DeclarationKind::Var, None),
            ast::VariableDeclarationKind::Let => (ir::DeclarationKind::Let, None),
            ast::VariableDeclarationKind::Const => (ir::DeclarationKind::Const, None),
            ast::VariableDeclarationKind::Using => (ir::DeclarationKind::Const, Some(false)),
            ast::VariableDeclarationKind::AwaitUsing => (ir::DeclarationKind::Const, Some(true)),
        };
        Ok(ir::StatementKind::Declaration(
            declaration
                .declarations
                .iter()
                .map(|declaration| {
                    Ok(ir::Declarator {
                        pattern: self.binding_pattern(&declaration.id, kind)?,
                        using,
                        initializer: declaration
                            .init
                            .as_ref()
                            .map(|value| {
                                let value = self.expression(value)?;
                                Ok(
                                    if let ast::BindingPattern::BindingIdentifier(id) =
                                        &declaration.id
                                    {
                                        Self::named_value(value, id.name.encode_utf16().collect())
                                    } else {
                                        value
                                    },
                                )
                            })
                            .transpose()?,
                    })
                })
                .collect::<R<_>>()?,
        ))
    }
    fn for_target(&self, target: &ast::ForStatementLeft<'_>) -> R<ir::ForTarget> {
        if let ast::ForStatementLeft::VariableDeclaration(declaration) = target {
            let ir::StatementKind::Declaration(mut declarations) = self.declaration(declaration)?
            else {
                unreachable!()
            };
            if declarations.len() != 1 {
                return Err(
                    self.unsupported(declaration.span, "loop declaration requires one binding")
                );
            }
            return Ok(ir::ForTarget::Declaration(
                declarations.pop().expect("one loop binding"),
            ));
        }
        let target = target
            .as_assignment_target()
            .expect("loop assignment target");
        if let Some(member) = target
            .as_member_expression()
            .filter(|m| self.suffixed_object(m).is_some())
        {
            let (object, key) = self.target_reference(member)?;
            return Ok(ir::ForTarget::Assignment(ir::Pattern::Member {
                object,
                key,
            }));
        }
        Ok(ir::ForTarget::Assignment(self.assignment_pattern(target)?))
    }
    fn catch_body(
        &self,
        handler: &ast::CatchClause<'_>,
    ) -> R<(Option<ir::Pattern>, Vec<ir::Statement>)> {
        let mut catch_frame = false;
        let binding = if let Some(parameter) = &handler.param {
            if let ast::BindingPattern::BindingIdentifier(id) = &parameter.pattern {
                let semantic_symbol =
                    id.symbol_id.get().expect("semantic catch symbol").index() as u32;
                let mut binding = self.binding(id, ir::DeclarationKind::Catch);
                binding.symbol = self.next_symbol();
                binding.scope = handler
                    .body
                    .scope_id
                    .get()
                    .expect("semantic catch body scope")
                    .index() as u32;
                let owner_scope = *self
                    .function_scopes
                    .borrow()
                    .last()
                    .expect("function scope");
                self.catch_frames.borrow_mut().push(CatchFrame {
                    name: binding.name.clone(),
                    semantic_symbol,
                    binding: binding.clone(),
                    owner_scope,
                });
                catch_frame = true;
                Some(ir::Pattern::Binding {
                    binding,
                    initialization_target: None,
                })
            } else {
                // The pinned native compiler uses TOK_LET for destructured
                // catch bindings: their cells are lexical and initialized in order.
                Some(self.binding_pattern(&parameter.pattern, ir::DeclarationKind::Let)?)
            }
        } else {
            None
        };
        let body = handler
            .body
            .body
            .iter()
            .map(|statement| self.statement(statement))
            .collect::<R<_>>();
        if catch_frame {
            self.catch_frames.borrow_mut().pop();
        }
        Ok((binding, body?))
    }
    fn statement(&self, s: &ast::Statement<'_>) -> R<ir::Statement> {
        let kind = match s {
            ast::Statement::ClassDeclaration(class) => {
                let id = class.id.as_ref().ok_or_else(|| {
                    self.unsupported(class.span, "class declaration requires a name")
                })?;
                ir::StatementKind::Class {
                    binding: self.binding(id, ir::DeclarationKind::Let),
                    class: self.class(class)?,
                }
            }
            ast::Statement::EmptyStatement(_) => ir::StatementKind::Empty,
            ast::Statement::ExpressionStatement(x) => {
                ir::StatementKind::Expression(self.expression(&x.expression)?)
            }
            ast::Statement::VariableDeclaration(x) => self.declaration(x)?,
            ast::Statement::BlockStatement(x) => ir::StatementKind::Block(
                x.body.iter().map(|s| self.statement(s)).collect::<R<_>>()?,
            ),
            ast::Statement::IfStatement(x) => ir::StatementKind::If {
                test: self.expression(&x.test)?,
                consequent: Box::new(self.statement(&x.consequent)?),
                alternate: x
                    .alternate
                    .as_ref()
                    .map(|s| self.statement(s).map(Box::new))
                    .transpose()?,
            },
            ast::Statement::WithStatement(x) => ir::StatementKind::With {
                object: self.expression(&x.object)?,
                body: Box::new(self.statement(&x.body)?),
                scope: x.scope_id.get().expect("semantic with scope").index() as u32,
            },
            ast::Statement::ForOfStatement(statement) => ir::StatementKind::ForOf {
                is_await: statement.r#await,
                left: self.for_target(&statement.left)?,
                right: self.expression(&statement.right)?,
                body: Box::new(self.statement(&statement.body)?),
                scope: statement
                    .scope_id
                    .get()
                    .expect("semantic for-of scope")
                    .index() as u32,
            },
            ast::Statement::ForInStatement(statement) => ir::StatementKind::ForIn {
                left: self.for_target(&statement.left)?,
                right: self.expression(&statement.right)?,
                body: Box::new(self.statement(&statement.body)?),
                scope: statement
                    .scope_id
                    .get()
                    .expect("semantic for-in scope")
                    .index() as u32,
            },
            ast::Statement::ForStatement(x) => {
                let init = x
                    .init
                    .as_ref()
                    .map(|init| {
                        let kind =
                            if let ast::ForStatementInit::VariableDeclaration(declaration) = init {
                                self.declaration(declaration)?
                            } else {
                                ir::StatementKind::Expression(self.expression(
                                    init.as_expression().expect("for init expression"),
                                )?)
                            };
                        Ok(ir::Statement {
                            kind,
                            span: self.span(init.span()),
                        })
                    })
                    .transpose()?;
                ir::StatementKind::For {
                    init: init.map(Box::new),
                    test: x.test.as_ref().map(|e| self.expression(e)).transpose()?,
                    update: x.update.as_ref().map(|e| self.expression(e)).transpose()?,
                    body: Box::new(self.statement(&x.body)?),
                }
            }
            ast::Statement::DoWhileStatement(x) => ir::StatementKind::DoWhile {
                test: self.expression(&x.test)?,
                body: Box::new(self.statement(&x.body)?),
            },
            ast::Statement::BreakStatement(statement) => {
                if let Some(label) = &statement.label {
                    ir::StatementKind::BreakLabel(label.name.to_string())
                } else {
                    ir::StatementKind::Break
                }
            }
            ast::Statement::ContinueStatement(statement) => {
                if let Some(label) = &statement.label {
                    ir::StatementKind::ContinueLabel(label.name.to_string())
                } else {
                    ir::StatementKind::Continue
                }
            }
            ast::Statement::LabeledStatement(statement) => ir::StatementKind::Label {
                label: statement.label.name.to_string(),
                body: Box::new(self.statement(&statement.body)?),
            },
            ast::Statement::SwitchStatement(statement) => ir::StatementKind::Switch {
                discriminant: self.expression(&statement.discriminant)?,
                cases: statement
                    .cases
                    .iter()
                    .map(|case| {
                        Ok(ir::SwitchCase {
                            test: case
                                .test
                                .as_ref()
                                .map(|test| self.expression(test))
                                .transpose()?,
                            consequent: case
                                .consequent
                                .iter()
                                .map(|statement| self.statement(statement))
                                .collect::<R<_>>()?,
                            span: self.span(case.span),
                        })
                    })
                    .collect::<R<_>>()?,
                scope: statement
                    .scope_id
                    .get()
                    .expect("semantic switch scope")
                    .index() as u32,
            },
            ast::Statement::WhileStatement(x) => ir::StatementKind::While {
                test: self.expression(&x.test)?,
                body: Box::new(self.statement(&x.body)?),
            },
            ast::Statement::ThrowStatement(statement) => {
                ir::StatementKind::Throw(self.expression(&statement.argument)?)
            }
            ast::Statement::TryStatement(statement) => {
                let mut body = Some(
                    statement
                        .block
                        .body
                        .iter()
                        .map(|statement| self.statement(statement))
                        .collect::<R<_>>()?,
                );
                let body_scope = statement
                    .block
                    .scope_id
                    .get()
                    .expect("semantic try block scope")
                    .index() as u32;
                let protected = if let Some(handler) = &statement.handler {
                    let (binding, statements) = self.catch_body(handler)?;
                    Some(ir::Statement {
                        kind: ir::StatementKind::TryCatch {
                            body: body.take().expect("original try body"),
                            binding,
                            handler: statements,
                            body_scope,
                            catch_scope: handler
                                .scope_id
                                .get()
                                .expect("semantic catch clause scope")
                                .index() as u32,
                            handler_scope: handler
                                .body
                                .scope_id
                                .get()
                                .expect("semantic catch body scope")
                                .index() as u32,
                        },
                        span: self.span(statement.span),
                    })
                } else {
                    None
                };
                if let Some(finalizer) = &statement.finalizer {
                    ir::StatementKind::TryFinally {
                        body: if let Some(protected) = protected {
                            vec![protected]
                        } else {
                            body.take().expect("original try body")
                        },
                        finalizer: finalizer
                            .body
                            .iter()
                            .map(|statement| self.statement(statement))
                            .collect::<R<_>>()?,
                        body_scope: if statement.handler.is_some() {
                            None
                        } else {
                            Some(body_scope)
                        },
                        finalizer_scope: finalizer
                            .scope_id
                            .get()
                            .expect("semantic finally scope")
                            .index() as u32,
                    }
                } else {
                    protected
                        .ok_or_else(|| {
                            self.unsupported(statement.span, "try requires catch or finally")
                        })?
                        .kind
                }
            }
            ast::Statement::ReturnStatement(x) => ir::StatementKind::Return(
                x.argument
                    .as_ref()
                    .map(|e| self.expression(e))
                    .transpose()?,
            ),
            ast::Statement::FunctionDeclaration(x) => {
                let id = x
                    .id
                    .as_ref()
                    .ok_or_else(|| self.unsupported(x.span, "function declaration needs a name"))?;
                ir::StatementKind::Function {
                    binding: self.binding(id, ir::DeclarationKind::Function),
                    function: self.function(x)?,
                }
            }
            ast::Statement::DebuggerStatement(_) => ir::StatementKind::Debugger,
            _ => {
                return Err(
                    self.unsupported(s.span(), "statement not implemented in general lowering")
                );
            }
        };
        Ok(ir::Statement {
            kind,
            span: self.span(s.span()),
        })
    }
    fn pattern_has_name(pattern: &ast::BindingPattern<'_>, name: &str) -> bool {
        match pattern {
            ast::BindingPattern::BindingIdentifier(id) => id.name == name,
            ast::BindingPattern::AssignmentPattern(pattern) => {
                Self::pattern_has_name(&pattern.left, name)
            }
            ast::BindingPattern::ArrayPattern(pattern) => {
                pattern
                    .elements
                    .iter()
                    .flatten()
                    .any(|pattern| Self::pattern_has_name(pattern, name))
                    || pattern
                        .rest
                        .as_ref()
                        .is_some_and(|rest| Self::pattern_has_name(&rest.argument, name))
            }
            ast::BindingPattern::ObjectPattern(pattern) => {
                pattern
                    .properties
                    .iter()
                    .any(|property| Self::pattern_has_name(&property.value, name))
                    || pattern
                        .rest
                        .as_ref()
                        .is_some_and(|rest| Self::pattern_has_name(&rest.argument, name))
            }
        }
    }
    /// The suspension kind of a function with these `async`/`*` modifiers.
    pub(super) fn function_kind(is_async: bool, generator: bool) -> ir::FunctionKind {
        match (is_async, generator) {
            (false, false) => ir::FunctionKind::Normal,
            (false, true) => ir::FunctionKind::Generator,
            (true, false) => ir::FunctionKind::Async,
            (true, true) => ir::FunctionKind::AsyncGenerator,
        }
    }
    fn function(&self, function: &ast::Function<'_>) -> R<ir::Function> {
        let body = function
            .body
            .as_ref()
            .ok_or_else(|| self.unsupported(function.span, "missing function body"))?;
        let mut lowered = self.function_parts(
            &function.params,
            function.scope_id.get().expect("semantic function scope"),
            function.span,
            function.id.as_ref(),
            function.r#type == ast::FunctionType::FunctionExpression,
            false,
            Some(body),
            None,
            None,
        )?;
        lowered.kind = Self::function_kind(function.r#async, function.generator);
        Ok(lowered)
    }
    fn arrow(&self, arrow: &ast::ArrowFunctionExpression<'_>) -> R<ir::Function> {
        let (body, expression) = match &arrow.body {
            ast::ArrowFunctionBody::FunctionBody(body) => (Some(body.as_ref()), None),
            body => (
                None,
                Some(body.as_expression().expect("arrow expression body")),
            ),
        };
        let mut lowered = self.function_parts(
            &arrow.params,
            arrow.scope_id.get().expect("semantic arrow scope"),
            arrow.span,
            None,
            false,
            true,
            body,
            expression,
            None,
        )?;
        lowered.kind = Self::function_kind(arrow.r#async, false);
        Ok(lowered)
    }
    #[allow(clippy::too_many_arguments)]
    fn function_parts(
        &self,
        source_parameters: &ast::FormalParameters<'_>,
        scope: oxc_syntax::scope::ScopeId,
        span: oxc_span::Span,
        name: Option<&ast::BindingIdentifier<'_>>,
        named_expression: bool,
        is_arrow: bool,
        source_body: Option<&ast::FunctionBody<'_>>,
        expression_body: Option<&ast::Expression<'_>>,
        method: Option<MethodContext>,
    ) -> R<ir::Function> {
        let scope_index = scope.index() as u32;
        let arguments_allowed = !is_arrow
            || *self
                .arguments_permissions
                .borrow()
                .last()
                .expect("arguments permission");
        let has_name = |name: &str| {
            source_parameters
                .items
                .iter()
                .any(|parameter| Self::pattern_has_name(&parameter.pattern, name))
                || source_parameters
                    .rest
                    .as_ref()
                    .is_some_and(|rest| Self::pattern_has_name(&rest.rest.argument, name))
        };
        let has_parameter_expressions =
            parameter_semantics::contains_expressions(source_parameters);
        let defined_arg_count = source_parameters
            .items
            .iter()
            .position(|parameter| parameter.initializer.is_some())
            .unwrap_or(source_parameters.items.len()) as u32;
        let hidden_name_symbol = if named_expression {
            name.filter(|name| name.name == "arguments")
                .and_then(|name| name.symbol_id.get())
                .map(|symbol| symbol.index() as u32)
        } else {
            None
        };
        let source_arguments = self
            .scoping
            .get_binding(scope, "arguments".into())
            .filter(|symbol| Some(symbol.index() as u32) != hidden_name_symbol);
        let shadowed_arguments = has_name("arguments")
            || !has_parameter_expressions
                && source_arguments
                    .is_some_and(|symbol| self.scoping.symbol_flags(symbol).is_block_scoped());
        let arguments_binding = if is_arrow || shadowed_arguments {
            None
        } else {
            Some(ir::Binding {
                symbol: if has_parameter_expressions {
                    self.next_symbol()
                } else {
                    source_arguments
                        .map_or_else(|| self.next_symbol(), |symbol| symbol.index() as u32)
                },
                scope: scope_index,
                name: "arguments".into(),
                kind: ir::DeclarationKind::Var,
                span: self.span(span),
            })
        };
        let this_binding = if is_arrow {
            self.this_bindings.borrow().last().cloned()
        } else {
            Some(ir::Binding {
                symbol: self.next_symbol(),
                scope: scope_index,
                name: "this".into(),
                kind: ir::DeclarationKind::Var,
                span: self.span(span),
            })
        };
        let new_target_binding = if is_arrow {
            self.new_target_bindings.borrow().last().cloned().flatten()
        } else {
            Some(ir::Binding {
                symbol: self.next_symbol(),
                scope: scope_index,
                name: "new.target".into(),
                kind: ir::DeclarationKind::Var,
                span: self.span(span),
            })
        };
        let super_frame = if is_arrow {
            self.super_frames.borrow().last().cloned().flatten()
        } else if let Some(method) = &method {
            let home = ir::Binding {
                symbol: self.next_symbol(),
                scope: scope_index,
                name: "<home_object>".into(),
                kind: ir::DeclarationKind::Var,
                span: self.span(span),
            };
            let constructor = if method.constructor && method.derived {
                Some(ir::Binding {
                    symbol: self.next_symbol(),
                    scope: scope_index,
                    name: "this.active_func".into(),
                    kind: ir::DeclarationKind::Var,
                    span: self.span(span),
                })
            } else {
                None
            };
            Some(SuperFrame {
                home,
                constructor,
                field_initializer: method.field_initializer.clone(),
                call_allowed: method.constructor && method.derived,
            })
        } else {
            None
        };
        let mut body_variables = BodyVarScan {
            owner_scope: scope_index,
            scoping: self.scoping,
            remaining: self.prepass_remaining.get(),
            exhausted: false,
            recording: false,
            names: std::collections::BTreeMap::new(),
        };
        if has_parameter_expressions && let Some(body) = source_body {
            body_variables.visit_statements(&body.statements);
            self.prepass_remaining.set(body_variables.remaining);
            if body_variables.exhausted {
                let mut diagnostic =
                    self.unsupported(span, "parameter body environment capacity exceeded");
                diagnostic.category = Category::Capacity;
                return Err(diagnostic);
            }
        }
        let own_name = if !has_parameter_expressions
            && named_expression
            && let Some(name) = name
            && name.name != "arguments"
            && !has_name(name.name.as_str())
            && let Some(symbol) = name.symbol_id.get()
            && self.scoping.get_binding(scope, name.name) == Some(symbol)
            && let Some(body) = source_body
        {
            let mut scan = OwnNameScan {
                name: name.name.as_str(),
                owner_scope: scope_index,
                scoping: self.scoping,
                remaining: self.prepass_remaining.get(),
                exhausted: false,
                found: false,
            };
            scan.visit_statements(&body.statements);
            self.prepass_remaining.set(scan.remaining);
            if scan.exhausted {
                let mut diagnostic =
                    self.unsupported(span, "function environment traversal capacity exceeded");
                diagnostic.category = Category::Capacity;
                return Err(diagnostic);
            }
            scan.found.then(|| OwnNameFrame {
                semantic_symbol: symbol.index() as u32,
                binding: ir::Binding {
                    symbol: self.next_symbol(),
                    scope: scope_index,
                    name: name.name.to_string(),
                    kind: ir::DeclarationKind::Var,
                    span: self.span(name.span),
                },
            })
        } else {
            None
        };
        if !is_arrow {
            self.argument_frames.borrow_mut().push(ArgumentFrame {
                scope: scope_index,
                binding: arguments_binding.clone(),
                hidden_name_symbol,
            });
        }
        self.function_scopes.borrow_mut().push(scope_index);
        let owner = if is_arrow {
            *self
                .this_owners
                .borrow()
                .last()
                .expect("lexical this owner")
        } else {
            self.this_uses.borrow().len()
        };
        self.this_owners.borrow_mut().push(owner);
        self.this_bindings
            .borrow_mut()
            .push(this_binding.clone().expect("function this binding"));
        self.new_target_bindings
            .borrow_mut()
            .push(new_target_binding.clone());
        self.this_uses.borrow_mut().push(false);
        self.new_target_uses.borrow_mut().push(false);
        self.direct_eval_uses.borrow_mut().push(false);
        self.super_frames.borrow_mut().push(super_frame.clone());
        self.home_object_uses.borrow_mut().push(false);
        self.arguments_permissions
            .borrow_mut()
            .push(arguments_allowed);
        let lowered: R<_> = (|| {
            // All parameter names exist before any default is evaluated. Emission
            // creates their TDZ cells before running this ordered pattern list.
            let parameters = source_parameters
                .items
                .iter()
                .map(|parameter| {
                    let mut pattern =
                        self.binding_pattern(&parameter.pattern, ir::DeclarationKind::Parameter)?;
                    if let Some(value) = &parameter.initializer {
                        pattern = Self::pattern_default(pattern, self.expression(value)?);
                    }
                    Ok(pattern)
                })
                .collect::<R<Vec<_>>>()?;
            let rest_parameter = source_parameters
                .rest
                .as_ref()
                .map(|rest| {
                    self.binding_pattern(&rest.rest.argument, ir::DeclarationKind::Parameter)
                })
                .transpose()?;
            let mut parameter_bindings = std::collections::BTreeMap::new();
            for parameter in parameters.iter().chain(rest_parameter.iter()) {
                parameter.for_each_binding(&mut |binding| {
                    parameter_bindings.insert(binding.name.clone(), binding.clone());
                });
            }
            if let Some(arguments) = &arguments_binding {
                parameter_bindings.insert(arguments.name.clone(), arguments.clone());
            }
            let mut body_parameter_copies = Vec::new();
            let mut body_frame = BodyVarFrame {
                scope: scope_index,
                by_symbol: std::collections::BTreeMap::new(),
                by_name: std::collections::BTreeMap::new(),
            };
            for (name, source) in body_variables.names {
                let binding = ir::Binding {
                    symbol: self.next_symbol(),
                    scope: scope_index,
                    name: name.clone(),
                    kind: ir::DeclarationKind::Var,
                    span: self.span(source.span),
                };
                for symbol in source.symbols {
                    body_frame.by_symbol.insert(symbol, binding.clone());
                }
                if let Some(symbol) = self.scoping.get_binding(scope, name.as_str().into()) {
                    body_frame
                        .by_symbol
                        .insert(symbol.index() as u32, binding.clone());
                }
                if let Some(parameter) = parameter_bindings.get(&name) {
                    body_frame
                        .by_symbol
                        .insert(parameter.symbol, binding.clone());
                    body_parameter_copies.push(ir::BodyParameterCopy {
                        parameter: ir::Reference {
                            symbol: Some(parameter.symbol),
                            name: name.clone(),
                        },
                        binding: binding.clone(),
                    });
                }
                body_frame.by_name.insert(name, binding);
            }
            let has_body_frame = !body_frame.by_name.is_empty();
            if has_body_frame {
                self.body_var_frames.borrow_mut().push(body_frame);
            }
            let has_own_name = own_name.is_some();
            if let Some(frame) = own_name {
                self.own_name_frames.borrow_mut().push(frame);
            }
            let body = if let Some(body) = source_body {
                body.statements
                    .iter()
                    .map(|statement| self.statement(statement))
                    .collect::<R<_>>()
            } else {
                let expression = expression_body.expect("arrow expression body");
                self.expression(expression).map(|value| {
                    vec![ir::Statement {
                        kind: ir::StatementKind::Return(Some(value)),
                        span: self.span(expression.span()),
                    }]
                })
            };
            if has_own_name {
                self.own_name_frames.borrow_mut().pop();
            }
            if has_body_frame {
                self.body_var_frames.borrow_mut().pop();
            }
            Ok((parameters, rest_parameter, body_parameter_copies, body?))
        })();
        if !is_arrow {
            self.argument_frames.borrow_mut().pop();
        }
        self.function_scopes.borrow_mut().pop();
        self.this_bindings.borrow_mut().pop();
        self.new_target_bindings.borrow_mut().pop();
        self.this_owners.borrow_mut().pop();
        self.super_frames.borrow_mut().pop();
        self.arguments_permissions.borrow_mut().pop();
        let uses_home_object = self
            .home_object_uses
            .borrow_mut()
            .pop()
            .expect("function home object context");
        let uses_this = self
            .this_uses
            .borrow_mut()
            .pop()
            .expect("function this context");
        let uses_new_target = self
            .new_target_uses
            .borrow_mut()
            .pop()
            .expect("function new.target context");
        let has_direct_eval = self
            .direct_eval_uses
            .borrow_mut()
            .pop()
            .expect("function direct eval context");
        let (parameters, rest_parameter, body_parameter_copies, body) = lowered?;
        Ok(ir::Function {
            initializer_name_binding: None,
            arguments_allowed,
            is_class_constructor: method.as_ref().is_some_and(|method| method.constructor),
            derived_constructor: method
                .as_ref()
                .is_some_and(|method| method.constructor && method.derived),
            home_object_binding: super_frame.as_ref().map(|frame| frame.home.clone()),
            uses_home_object,
            super_constructor_binding: super_frame
                .as_ref()
                .and_then(|frame| frame.constructor.clone()),
            field_initializer_binding: super_frame
                .as_ref()
                .and_then(|frame| frame.field_initializer.clone()),
            super_call_allowed: super_frame.as_ref().is_some_and(|frame| frame.call_allowed),
            is_arrow,
            new_target_binding,
            uses_new_target,
            this_binding,
            uses_this,
            arguments_binding,
            is_method: method.is_some(),
            self_binding: None,
            name: name.map(|name| name.name.to_string()),
            scope: scope_index,
            parameters,
            has_parameter_expressions,
            defined_arg_count,
            body_parameter_copies,
            rest_parameter,
            body,
            strict: self.scoping.scope_flags(scope).is_strict_mode(),
            has_direct_eval,
            span: self.span(span),
            kind: ir::FunctionKind::Normal,
        })
    }
    fn binary(op: oxc_syntax::operator::BinaryOperator) -> ir::Binary {
        use oxc_syntax::operator::BinaryOperator as B;
        match op {
            B::Addition => ir::Binary::Add,
            B::Subtraction => ir::Binary::Subtract,
            B::Multiplication => ir::Binary::Multiply,
            B::Division => ir::Binary::Divide,
            B::Remainder => ir::Binary::Remainder,
            B::Exponential => ir::Binary::Exponent,
            B::Equality => ir::Binary::Equal,
            B::Inequality => ir::Binary::NotEqual,
            B::StrictEquality => ir::Binary::StrictEqual,
            B::StrictInequality => ir::Binary::StrictNotEqual,
            B::LessThan => ir::Binary::Less,
            B::LessEqualThan => ir::Binary::LessEqual,
            B::GreaterThan => ir::Binary::Greater,
            B::GreaterEqualThan => ir::Binary::GreaterEqual,
            B::BitwiseAnd => ir::Binary::BitAnd,
            B::BitwiseOR => ir::Binary::BitOr,
            B::BitwiseXOR => ir::Binary::BitXor,
            B::ShiftLeft => ir::Binary::ShiftLeft,
            B::ShiftRight => ir::Binary::ShiftRight,
            B::ShiftRightZeroFill => ir::Binary::ShiftUnsigned,
            B::In => ir::Binary::In,
            B::Instanceof => ir::Binary::Instanceof,
        }
    }
    /// A member suffixed by `call_targets`: `Some` with the call, or the SyntaxError the
    /// parser gave for anything other than a plain call.
    fn call_target(&self, member: &ast::MemberExpression<'_>) -> Option<R<Box<ir::Expression>>> {
        let object = self.suffixed_object(member)?;
        Some(match object {
            ast::Expression::CallExpression(call)
                if !call.optional && !matches!(call.callee, ast::Expression::Super(_)) =>
            {
                self.expression(object).map(|call| {
                    Box::new(ir::Expression {
                        span: call.span,
                        kind: ir::ExpressionKind::InvalidCallTarget(Box::new(call)),
                    })
                })
            }
            object => Err(self.invalid_target(object.span())),
        })
    }
    /// The object of a member whose property is the `CALL_TARGET_SUFFIX` of the parse view.
    fn suffixed_object<'b>(
        &self,
        member: &'b ast::MemberExpression<'b>,
    ) -> Option<&'b ast::Expression<'b>> {
        let ast::MemberExpression::StaticMemberExpression(member) = member else {
            return None;
        };
        self.input
            .is_inserted(member.property.span.start..member.property.span.end)
            .then_some(&member.object)
    }
    fn invalid_target(&self, s: oxc_span::Span) -> Diagnostic {
        Diagnostic {
            category: Category::Parse,
            ..self.unsupported(s, "Cannot assign to this expression")
        }
    }
    /// A member assignment, update or for-in/of target; Annex B admits a call there.
    fn target_reference(
        &self,
        member: &ast::MemberExpression<'_>,
    ) -> R<(Box<ir::Expression>, Box<ir::Expression>)> {
        let Some(call) = self.call_target(member) else {
            return self.member_reference(member);
        };
        let call = call?;
        let key = Box::new(ir::Expression {
            span: call.span,
            kind: ir::ExpressionKind::Literal(ir::Literal::Undefined),
        });
        Ok((call, key))
    }
    fn member_reference(
        &self,
        member: &ast::MemberExpression<'_>,
    ) -> R<(Box<ir::Expression>, Box<ir::Expression>)> {
        if let Some(object) = self.suffixed_object(member) {
            // Destructuring and logical assignment need a real reference.
            return Err(self.invalid_target(object.span()));
        }
        match member {
            ast::MemberExpression::StaticMemberExpression(member) if !member.optional => Ok((
                Box::new(self.expression(&member.object)?),
                Box::new(ir::Expression {
                    kind: ir::ExpressionKind::Literal(ir::Literal::String(
                        member.property.name.encode_utf16().collect(),
                    )),
                    span: self.span(member.property.span),
                }),
            )),
            ast::MemberExpression::ComputedMemberExpression(member) if !member.optional => Ok((
                Box::new(self.expression(&member.object)?),
                Box::new(self.expression(&member.expression)?),
            )),
            _ => Err(self.unsupported(
                member.span(),
                "optional/private property references not implemented",
            )),
        }
    }
    fn object_key(&self, key: &ast::PropertyKey<'_>, computed: bool) -> R<ir::ObjectKey> {
        if !computed {
            match key {
                ast::PropertyKey::StaticIdentifier(key) => {
                    return Ok(ir::ObjectKey::Static(key.name.encode_utf16().collect()));
                }
                ast::PropertyKey::StringLiteral(key) => {
                    return Ok(ir::ObjectKey::Static(key.value.encode_utf16().collect()));
                }
                _ => {}
            }
        }
        // Numeric/BigInt literal names need JavaScript's canonical ToPropertyKey
        // spelling, rather than the original source-token text.
        let expression = key
            .as_expression()
            .ok_or_else(|| self.unsupported(key.span(), "property key not implemented"))?;
        Ok(ir::ObjectKey::Computed(self.expression(expression)?))
    }
    fn object_property(&self, property: &ast::ObjectProperty<'_>) -> R<ir::ObjectEntry> {
        let key = self.object_key(&property.key, property.computed)?;
        let is_method = property.method || property.kind != ast::PropertyKind::Init;
        let mut value = if is_method
            && let ast::Expression::FunctionExpression(function) = &property.value
        {
            let mut function = self.method_function(
                function,
                MethodContext {
                    constructor: false,
                    derived: false,
                    field_initializer: None,
                },
            )?;
            function.span = function_source::class_method_span(self.input, property.span, false)
                .ok_or_else(|| {
                    self.unsupported(property.span, "method source boundary is invalid")
                })?;
            ir::Expression {
                kind: ir::ExpressionKind::Function(Box::new(function)),
                span: self.span(property.value.span()),
            }
        } else {
            self.expression(&property.value)?
        };
        if is_method && let ir::ExpressionKind::Function(function) = &mut value.kind {
            function.is_method = true;
        }
        Ok(match property.kind {
            ast::PropertyKind::Get => ir::ObjectEntry::Getter { key, value },
            ast::PropertyKind::Set => ir::ObjectEntry::Setter { key, value },
            ast::PropertyKind::Init
                if !property.computed
                    && !property.method
                    && !property.shorthand
                    && matches!(&key, ir::ObjectKey::Static(name)
                        if name.iter().copied().eq("__proto__".encode_utf16())) =>
            {
                ir::ObjectEntry::Prototype { value }
            }
            ast::PropertyKind::Init => {
                if !property.method
                    && let ir::ObjectKey::Static(name) = &key
                {
                    value = Self::named_value(value, name.clone());
                }
                let infer_name = !property.method && Self::anonymous_definition(&value);
                ir::ObjectEntry::Data {
                    key,
                    value,
                    method: property.method,
                    infer_name,
                }
            }
        })
    }
    fn chain_member(
        &self,
        member: &ast::MemberExpression<'_>,
        segments: &mut Vec<ir::ChainSegment>,
    ) -> R<ir::Expression> {
        if let Some(key) = self.super_key(member)? {
            return Ok(ir::Expression {
                kind: ir::ExpressionKind::SuperMember { key },
                span: self.span(member.span()),
            });
        }
        if let ast::MemberExpression::PrivateFieldExpression(member) = member {
            let base = self.chain_base(&member.object, segments)?;
            segments.push(ir::ChainSegment::PrivateMember {
                id: self.private_name(&member.field)?,
                optional: member.optional,
                span: self.span(member.span),
            });
            return Ok(base);
        }
        let (object, key, optional) = match member {
            ast::MemberExpression::StaticMemberExpression(member) => (
                &member.object,
                ir::Expression {
                    kind: ir::ExpressionKind::Literal(ir::Literal::String(
                        member.property.name.encode_utf16().collect(),
                    )),
                    span: self.span(member.property.span),
                },
                member.optional,
            ),
            ast::MemberExpression::ComputedMemberExpression(member) => (
                &member.object,
                self.expression(&member.expression)?,
                member.optional,
            ),
            _ => {
                return Err(self.unsupported(
                    member.span(),
                    "private optional-chain references not implemented",
                ));
            }
        };
        let base = self.chain_base(object, segments)?;
        segments.push(ir::ChainSegment::Member {
            key,
            optional,
            span: self.span(member.span()),
        });
        Ok(base)
    }
    fn call_arguments(
        &self,
        arguments: &oxc_allocator::Vec<'_, ast::Argument<'_>>,
    ) -> R<Vec<ir::CallArgument>> {
        arguments
            .iter()
            .map(|argument| {
                Ok(match argument {
                    ast::Argument::SpreadElement(spread) => {
                        ir::CallArgument::Spread(self.expression(&spread.argument)?)
                    }
                    argument => ir::CallArgument::Value(self.expression(
                        argument.as_expression().ok_or_else(|| {
                            self.unsupported(argument.span(), "call argument not implemented")
                        })?,
                    )?),
                })
            })
            .collect()
    }
    /// Direct eval sees the caller's frame: its `this` and `new.target`
    /// must exist as named bindings, like inside a nested arrow function.
    fn direct_eval(&self, call: &ast::CallExpression<'_>) -> R<ir::Expression> {
        *self
            .direct_eval_uses
            .borrow_mut()
            .last_mut()
            .expect("direct eval frame") = true;
        let owner = *self.this_owners.borrow().last().expect("this owner");
        for used in &mut self.this_uses.borrow_mut()[owner..] {
            *used = true;
        }
        if self
            .new_target_bindings
            .borrow()
            .last()
            .and_then(Option::as_ref)
            .is_some()
        {
            for used in &mut self.new_target_uses.borrow_mut()[owner..] {
                *used = true;
            }
        }
        let arguments = self.call_arguments(&call.arguments)?;
        Ok(ir::Expression {
            kind: ir::ExpressionKind::DirectEval {
                callee: Box::new(self.expression(&call.callee)?),
                arguments,
            },
            span: self.span(call.span),
        })
    }
    fn chain_call(
        &self,
        call: &ast::CallExpression<'_>,
        segments: &mut Vec<ir::ChainSegment>,
    ) -> R<ir::Expression> {
        if matches!(call.callee, ast::Expression::Super(_)) {
            self.mark_super(true, call.span)?;
            let arguments = self.call_arguments(&call.arguments)?;
            return Ok(ir::Expression {
                kind: ir::ExpressionKind::SuperCall { arguments },
                span: self.span(call.span),
            });
        }
        if !call.optional
            && matches!(call.callee.without_parentheses(), ast::Expression::Identifier(id) if id.name=="eval")
        {
            return self.direct_eval(call);
        }
        let base = self.chain_base(&call.callee, segments)?;
        let arguments = self.call_arguments(&call.arguments)?;
        segments.push(ir::ChainSegment::Call {
            arguments,
            optional: call.optional,
            span: self.span(call.span),
        });
        Ok(base)
    }
    fn chain_base(
        &self,
        expression: &ast::Expression<'_>,
        segments: &mut Vec<ir::ChainSegment>,
    ) -> R<ir::Expression> {
        if let Some(member) = expression.as_member_expression() {
            self.chain_member(member, segments)
        } else if let ast::Expression::CallExpression(call) = expression {
            self.chain_call(call, segments)
        } else {
            // In particular, do not flatten across parentheses/inner ChainExpression.
            self.expression(expression)
        }
    }
    fn expression(&self, e: &ast::Expression<'_>) -> R<ir::Expression> {
        use oxc_syntax::operator::{LogicalOperator as L, UnaryOperator as U};
        let kind = match e {
            ast::Expression::ClassExpression(class) => {
                ir::ExpressionKind::Class(Box::new(self.class(class)?))
            }
            ast::Expression::PrivateFieldExpression(member) => {
                if member.optional {
                    return Err(self.unsupported(
                        member.span,
                        "private optional reference requires its chain boundary",
                    ));
                }
                ir::ExpressionKind::PrivateMember {
                    object: Box::new(self.expression(&member.object)?),
                    id: self.private_name(&member.field)?,
                }
            }
            ast::Expression::PrivateInExpression(expression) => ir::ExpressionKind::PrivateIn {
                object: Box::new(self.expression(&expression.right)?),
                id: self.private_name(&expression.left)?,
            },
            ast::Expression::TemplateLiteral(x) => {
                let mut parts = Vec::new();
                for quasi in &x.quasis {
                    let raw = self
                        .input
                        .original_units(quasi.span.start as usize..quasi.span.end as usize)
                        .expect("template original span");
                    let mut normalized = Vec::new();
                    let mut i = 0;
                    while i < raw.len() {
                        if raw[i] == 13 {
                            normalized.push(10);
                            if raw.get(i + 1) == Some(&10) {
                                i += 1;
                            }
                        } else {
                            normalized.push(raw[i]);
                        }
                        i += 1;
                    }
                    let cooked = quasi
                        .value
                        .cooked
                        .as_ref()
                        .ok_or_else(|| {
                            self.unsupported(quasi.span, "invalid untagged template escape")
                        })?
                        .encode_utf16()
                        .collect();
                    parts.push(ir::TemplatePart {
                        raw: normalized,
                        cooked,
                    });
                }
                ir::ExpressionKind::Template {
                    parts,
                    substitutions: x
                        .expressions
                        .iter()
                        .map(|e| self.expression(e))
                        .collect::<R<_>>()?,
                }
            }
            ast::Expression::SequenceExpression(x) => ir::ExpressionKind::Sequence(
                x.expressions
                    .iter()
                    .map(|e| self.expression(e))
                    .collect::<R<_>>()?,
            ),
            ast::Expression::ConditionalExpression(x) => ir::ExpressionKind::Conditional {
                test: Box::new(self.expression(&x.test)?),
                consequent: Box::new(self.expression(&x.consequent)?),
                alternate: Box::new(self.expression(&x.alternate)?),
            },
            ast::Expression::ArrayExpression(x)
                if x.elements
                    .iter()
                    .all(|element| element.as_expression().is_some()) =>
            {
                ir::ExpressionKind::Array(
                    x.elements
                        .iter()
                        .filter_map(|element| element.as_expression())
                        .map(|element| self.expression(element))
                        .collect::<R<_>>()?,
                )
            }
            ast::Expression::ArrayExpression(x) => ir::ExpressionKind::SpreadArray(
                x.elements
                    .iter()
                    .map(|element| match element {
                        ast::ArrayExpressionElement::SpreadElement(spread) => {
                            Ok(ir::ArrayElement::Spread(self.expression(&spread.argument)?))
                        }
                        ast::ArrayExpressionElement::Elision(_) => Ok(ir::ArrayElement::Hole),
                        element => Ok(ir::ArrayElement::Value(
                            self.expression(element.to_expression())?,
                        )),
                    })
                    .collect::<R<_>>()?,
            ),
            ast::Expression::RegExpLiteral(x) => {
                let start = x.span.start as usize;
                let end = x.span.end as usize;
                let raw = x.raw.as_ref().map(|raw| raw.as_str()).unwrap_or("");
                let slash = raw
                    .rfind('/')
                    .filter(|&at| at > 0 && raw.len() == end - start)
                    .ok_or_else(|| self.unsupported(x.span, "regexp literal source is invalid"))?;
                let pattern = start + 1..start + slash;
                let flags = start + slash + 1..end;
                self.input
                    .validate_regexp_literal(pattern.clone(), flags.clone())
                    .map_err(|e| {
                        let mut diagnostic = self.unsupported(x.span, &e.message);
                        diagnostic.category = Category::Parse;
                        diagnostic
                    })?;
                let units = |range| {
                    self.input
                        .original_units(range)
                        .map(<[u16]>::to_vec)
                        .ok_or_else(|| self.unsupported(x.span, "regexp literal source is invalid"))
                };
                ir::ExpressionKind::RegExp {
                    pattern: units(pattern)?,
                    flags: units(flags)?,
                }
            }
            ast::Expression::ImportMeta(_) => ir::ExpressionKind::ImportMeta,
            ast::Expression::TaggedTemplateExpression(x) => {
                let (mut cooked, mut raw) = (Vec::new(), Vec::new());
                for quasi in &x.quasi.quasis {
                    let units = self
                        .input
                        .original_units(quasi.span.start as usize..quasi.span.end as usize)
                        .expect("template original span");
                    // Raw strings normalize CR and CRLF to LF.
                    let mut normalized = Vec::with_capacity(units.len());
                    let mut i = 0;
                    while i < units.len() {
                        if units[i] == 13 {
                            normalized.push(10);
                            if units.get(i + 1) == Some(&10) {
                                i += 1;
                            }
                        } else {
                            normalized.push(units[i]);
                        }
                        i += 1;
                    }
                    raw.push(normalized);
                    cooked.push(
                        quasi
                            .value
                            .cooked
                            .as_ref()
                            .map(|c| c.encode_utf16().collect()),
                    );
                }
                let mut arguments = vec![ir::CallArgument::Value(ir::Expression {
                    kind: ir::ExpressionKind::TemplateObject { cooked, raw },
                    span: self.span(x.quasi.span),
                })];
                for expression in &x.quasi.expressions {
                    arguments.push(ir::CallArgument::Value(self.expression(expression)?));
                }
                ir::ExpressionKind::Call {
                    callee: Box::new(self.expression(&x.tag)?),
                    arguments,
                }
            }
            ast::Expression::ImportExpression(x) => {
                if x.phase.is_some() {
                    return Err(self.unsupported(x.span, "import phase modifiers not implemented"));
                }
                ir::ExpressionKind::ImportCall {
                    specifier: Box::new(self.expression(&x.source)?),
                    options: x
                        .options
                        .as_ref()
                        .map(|o| self.expression(o).map(Box::new))
                        .transpose()?,
                }
            }
            ast::Expression::ObjectExpression(x) => {
                let mut properties = Vec::new();
                for property in &x.properties {
                    properties.push(match property {
                        ast::ObjectPropertyKind::ObjectProperty(property) => {
                            self.object_property(property)?
                        }
                        ast::ObjectPropertyKind::SpreadProperty(spread) => {
                            ir::ObjectEntry::Spread {
                                value: self.expression(&spread.argument)?,
                            }
                        }
                    });
                }
                ir::ExpressionKind::Object(properties)
            }
            ast::Expression::StaticMemberExpression(x) => {
                if matches!(x.object, ast::Expression::Super(_)) {
                    self.mark_super(false, x.span)?;
                    return Ok(ir::Expression {
                        kind: ir::ExpressionKind::SuperMember {
                            key: Box::new(ir::Expression {
                                kind: ir::ExpressionKind::Literal(ir::Literal::String(
                                    x.property.name.encode_utf16().collect(),
                                )),
                                span: self.span(x.property.span),
                            }),
                        },
                        span: self.span(x.span),
                    });
                }
                if x.optional {
                    return Err(self.unsupported(x.span, "optional members not implemented"));
                }
                ir::ExpressionKind::Member {
                    object: Box::new(self.expression(&x.object)?),
                    key: Box::new(ir::Expression {
                        kind: ir::ExpressionKind::Literal(ir::Literal::String(
                            x.property.name.encode_utf16().collect(),
                        )),
                        span: self.span(x.property.span),
                    }),
                }
            }
            ast::Expression::ComputedMemberExpression(x) => {
                if matches!(x.object, ast::Expression::Super(_)) {
                    self.mark_super(false, x.span)?;
                    return Ok(ir::Expression {
                        kind: ir::ExpressionKind::SuperMember {
                            key: Box::new(self.expression(&x.expression)?),
                        },
                        span: self.span(x.span),
                    });
                }
                if x.optional {
                    return Err(self.unsupported(x.span, "optional members not implemented"));
                }
                ir::ExpressionKind::Member {
                    object: Box::new(self.expression(&x.object)?),
                    key: Box::new(self.expression(&x.expression)?),
                }
            }
            ast::Expression::ChainExpression(chain) => {
                let mut segments = Vec::new();
                let base = if let Some(member) = chain.expression.as_member_expression() {
                    self.chain_member(member, &mut segments)?
                } else if let ast::ChainElement::CallExpression(call) = &chain.expression {
                    self.chain_call(call, &mut segments)?
                } else {
                    return Err(self.unsupported(chain.span, "chain element not implemented"));
                };
                ir::ExpressionKind::Chain {
                    base: Box::new(base),
                    segments,
                }
            }
            ast::Expression::UpdateExpression(x) => {
                let increment = x.operator == oxc_syntax::operator::UpdateOperator::Increment;
                if let ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = &x.argument {
                    ir::ExpressionKind::Update {
                        target: self.reference(id),
                        increment,
                        prefix: x.prefix,
                    }
                } else if let Some(member) = x.argument.as_member_expression() {
                    if let Some(key) = self.super_key(member)? {
                        return Ok(ir::Expression {
                            kind: ir::ExpressionKind::UpdateSuper {
                                key,
                                increment,
                                prefix: x.prefix,
                            },
                            span: self.span(x.span),
                        });
                    }
                    if let ast::MemberExpression::PrivateFieldExpression(member) = member {
                        return Ok(ir::Expression {
                            kind: ir::ExpressionKind::UpdatePrivate {
                                object: Box::new(self.expression(&member.object)?),
                                id: self.private_name(&member.field)?,
                                increment,
                                prefix: x.prefix,
                            },
                            span: self.span(x.span),
                        });
                    }
                    let (object, key) = self.target_reference(member)?;
                    ir::ExpressionKind::UpdateMember {
                        object,
                        key,
                        increment,
                        prefix: x.prefix,
                    }
                } else {
                    return Err(self.unsupported(x.span, "update target not implemented"));
                }
            }
            ast::Expression::BigIntLiteral(x) => {
                let mut bytes = Vec::<u8>::new();
                for digit in x.value.bytes() {
                    if !digit.is_ascii_digit() {
                        return Err(self.unsupported(x.span, "invalid decimal bigint from parser"));
                    }
                    let mut carry = u16::from(digit - b'0');
                    for byte in &mut bytes {
                        let value = u16::from(*byte) * 10 + carry;
                        *byte = value as u8;
                        carry = value >> 8;
                    }
                    if carry != 0 {
                        bytes.push(carry as u8);
                    }
                }
                if bytes.last().is_some_and(|b| b & 128 != 0) {
                    bytes.push(0);
                }
                ir::ExpressionKind::Literal(ir::Literal::BigInt(bytes))
            }
            ast::Expression::NumericLiteral(x) => {
                ir::ExpressionKind::Literal(ir::Literal::Number(x.value))
            }
            ast::Expression::BooleanLiteral(x) => {
                ir::ExpressionKind::Literal(ir::Literal::Bool(x.value))
            }
            ast::Expression::StringLiteral(x) => {
                ir::ExpressionKind::Literal(ir::Literal::String(x.value.encode_utf16().collect()))
            }
            ast::Expression::NullLiteral(_) => ir::ExpressionKind::Literal(ir::Literal::Null),
            ast::Expression::Identifier(x) => ir::ExpressionKind::Read(self.reference(x)),
            ast::Expression::ThisExpression(_) => {
                let owner = *self.this_owners.borrow().last().expect("this owner");
                for used in &mut self.this_uses.borrow_mut()[owner..] {
                    *used = true;
                }
                ir::ExpressionKind::This
            }
            ast::Expression::NewTarget(target) => {
                if self
                    .new_target_bindings
                    .borrow()
                    .last()
                    .and_then(Option::as_ref)
                    .is_none()
                {
                    return Err(self.unsupported(
                        target.span,
                        "new.target requires an ordinary function owner",
                    ));
                }
                let owner = *self.this_owners.borrow().last().expect("new.target owner");
                for used in &mut self.new_target_uses.borrow_mut()[owner..] {
                    *used = true;
                }
                ir::ExpressionKind::NewTarget
            }
            ast::Expression::ParenthesizedExpression(x) => return self.expression(&x.expression),
            ast::Expression::UnaryExpression(x) => ir::ExpressionKind::Unary {
                op: match x.operator {
                    U::UnaryNegation => ir::Unary::Negate,
                    U::UnaryPlus => ir::Unary::Plus,
                    U::LogicalNot => ir::Unary::Not,
                    U::BitwiseNot => ir::Unary::BitNot,
                    U::Typeof => ir::Unary::Typeof,
                    U::Void => ir::Unary::Void,
                    U::Delete => ir::Unary::Delete,
                },
                argument: Box::new(self.expression(&x.argument)?),
            },
            ast::Expression::BinaryExpression(x) => ir::ExpressionKind::Binary {
                op: Self::binary(x.operator),
                left: Box::new(self.expression(&x.left)?),
                right: Box::new(self.expression(&x.right)?),
            },
            ast::Expression::LogicalExpression(x) => ir::ExpressionKind::Logical {
                op: match x.operator {
                    L::And => ir::Logical::And,
                    L::Or => ir::Logical::Or,
                    L::Coalesce => ir::Logical::Nullish,
                },
                left: Box::new(self.expression(&x.left)?),
                right: Box::new(self.expression(&x.right)?),
            },
            ast::Expression::AssignmentExpression(x) => {
                let value = Box::new(self.expression(&x.right)?);
                let logical = x.operator.to_logical_operator().map(|op| match op {
                    L::And => ir::Logical::And,
                    L::Or => ir::Logical::Or,
                    L::Coalesce => ir::Logical::Nullish,
                });
                let binary = x.operator.to_binary_operator().map(Self::binary);
                if let Some(member) = x.left.as_member_expression() {
                    let op = if let Some(op) = logical {
                        ir::AssignmentOp::Logical(op)
                    } else if let Some(op) = binary {
                        ir::AssignmentOp::Compound(op)
                    } else {
                        ir::AssignmentOp::Assign
                    };
                    if let Some(key) = self.super_key(member)? {
                        return Ok(ir::Expression {
                            kind: ir::ExpressionKind::AssignSuper { key, value, op },
                            span: self.span(x.span),
                        });
                    }
                    if let ast::MemberExpression::PrivateFieldExpression(member) = member {
                        return Ok(ir::Expression {
                            kind: ir::ExpressionKind::AssignPrivate {
                                object: Box::new(self.expression(&member.object)?),
                                id: self.private_name(&member.field)?,
                                value,
                                op,
                            },
                            span: self.span(x.span),
                        });
                    }
                }
                if x.operator.is_assign()
                    && matches!(
                        &x.left,
                        ast::AssignmentTarget::ArrayAssignmentTarget(_)
                            | ast::AssignmentTarget::ObjectAssignmentTarget(_)
                    )
                {
                    ir::ExpressionKind::DestructureAssign {
                        pattern: Box::new(self.assignment_pattern(&x.left)?),
                        value,
                    }
                } else if let ast::AssignmentTarget::AssignmentTargetIdentifier(id) = &x.left {
                    let value = if x.operator.is_assign() || logical.is_some() {
                        Box::new(Self::named_value(*value, id.name.encode_utf16().collect()))
                    } else {
                        value
                    };
                    let target = self.reference(id);
                    if let Some(op) = logical {
                        ir::ExpressionKind::LogicalAssign { target, op, value }
                    } else {
                        let value = if let Some(op) = binary {
                            Box::new(ir::Expression {
                                span: self.span(x.span),
                                kind: ir::ExpressionKind::Binary {
                                    op,
                                    left: Box::new(ir::Expression {
                                        span: self.span(id.span),
                                        kind: ir::ExpressionKind::Read(target.clone()),
                                    }),
                                    right: value,
                                },
                            })
                        } else {
                            value
                        };
                        ir::ExpressionKind::Assign { target, value }
                    }
                } else if let Some(member) = x.left.as_member_expression() {
                    let (object, key) = if logical.is_some() {
                        self.member_reference(member)?
                    } else {
                        self.target_reference(member)?
                    };
                    if let Some(op) = logical {
                        ir::ExpressionKind::LogicalAssignMember {
                            object,
                            key,
                            op,
                            value,
                        }
                    } else if let Some(op) = binary {
                        ir::ExpressionKind::CompoundAssignMember {
                            object,
                            key,
                            op,
                            value,
                        }
                    } else {
                        ir::ExpressionKind::AssignMember { object, key, value }
                    }
                } else {
                    return Err(self.unsupported(x.span, "assignment target not implemented"));
                }
            }
            ast::Expression::ArrowFunctionExpression(arrow) => {
                ir::ExpressionKind::Function(Box::new(self.arrow(arrow)?))
            }
            ast::Expression::FunctionExpression(x) => {
                let mut function = self.function(x)?;
                function.self_binding =
                    x.id.as_ref()
                        .map(|id| self.binding(id, ir::DeclarationKind::FunctionName));
                ir::ExpressionKind::Function(Box::new(function))
            }
            ast::Expression::NewExpression(x) => {
                let arguments = self.call_arguments(&x.arguments)?;
                ir::ExpressionKind::Construct {
                    callee: Box::new(self.expression(&x.callee)?),
                    arguments,
                }
            }
            ast::Expression::CallExpression(x) => {
                if matches!(x.callee, ast::Expression::Super(_)) {
                    self.mark_super(true, x.span)?;
                    let arguments = self.call_arguments(&x.arguments)?;
                    return Ok(ir::Expression {
                        kind: ir::ExpressionKind::SuperCall { arguments },
                        span: self.span(x.span),
                    });
                }
                if x.optional {
                    return Err(self.unsupported(x.span, "optional calls not implemented"));
                }
                if matches!(x.callee.without_parentheses(),ast::Expression::Identifier(id) if id.name=="eval")
                {
                    return self.direct_eval(x);
                }
                let arguments = self.call_arguments(&x.arguments)?;
                ir::ExpressionKind::Call {
                    callee: Box::new(self.expression(&x.callee)?),
                    arguments,
                }
            }
            ast::Expression::AwaitExpression(x) => {
                ir::ExpressionKind::Await(Box::new(self.expression(&x.argument)?))
            }
            ast::Expression::YieldExpression(x) => ir::ExpressionKind::Yield {
                argument: x
                    .argument
                    .as_ref()
                    .map(|a| self.expression(a).map(Box::new))
                    .transpose()?,
                delegate: x.delegate,
            },
            _ => {
                return Err(
                    self.unsupported(e.span(), "expression not implemented in general lowering")
                );
            }
        };
        Ok(ir::Expression {
            kind,
            span: self.span(e.span()),
        })
    }
}

impl Lower<'_> {
    fn export_name(name: &ast::ModuleExportName<'_>) -> Vec<u16> {
        match name {
            ast::ModuleExportName::IdentifierName(n) => n.name.encode_utf16().collect(),
            ast::ModuleExportName::IdentifierReference(n) => n.name.encode_utf16().collect(),
            ast::ModuleExportName::StringLiteral(n) => n.value.encode_utf16().collect(),
        }
    }
    /// The index of the module request for `source` with its attributes,
    /// adding it when new.
    ///
    /// lunarstorm patch: jetbunny dropped the `with` clause ("the host picks a
    /// module's kind from its file"), so rquickjs loaders never received
    /// import attributes. They are now carried. A request stays one per
    /// specifier (jetbunny's identity, which Lunarstorm's module graph keys
    /// on); the C parser instead made a second request for a second attribute
    /// set. That case is now a compile error, never a silent pick of one set.
    ///
    /// # Arguments
    /// * `module` - the module's linkage being built.
    /// * `source` - the specifier.
    /// * `with` - the `with` (or legacy `assert`) clause, if any.
    ///
    /// # Returns
    /// The request index.
    ///
    /// # Errors
    /// A key appears twice in the clause, or the specifier was already
    /// requested with a different attribute set.
    fn request(
        &self,
        module: &mut ir::Module,
        source: &ast::StringLiteral<'_>,
        with: Option<&ast::WithClause<'_>>,
    ) -> R<u32> {
        let name: Vec<u16> = source.value.encode_utf16().collect();
        let mut attributes: Vec<(Vec<u16>, Vec<u16>)> = Vec::new();
        for entry in with.map(|w| w.with_entries.as_slice()).unwrap_or_default() {
            let key: Vec<u16> = match &entry.key {
                ast::ImportAttributeKey::Identifier(k) => k.name.encode_utf16().collect(),
                ast::ImportAttributeKey::StringLiteral(k) => k.value.encode_utf16().collect(),
            };
            if attributes.iter().any(|(k, _)| *k == key) {
                return Err(self.unsupported(entry.span, "duplicate with key"));
            }
            attributes.push((key, entry.value.value.encode_utf16().collect()));
        }
        if let Some(at) = module.requests.iter().position(|r| *r == name) {
            let known = &module.request_attributes[at];
            if known.len() != attributes.len() || !known.iter().all(|p| attributes.contains(p)) {
                return Err(self.unsupported(source.span, "conflicting import attributes"));
            }
            Ok(at as u32)
        } else {
            let at = module.requests.len() as u32;
            module.requests.push(name);
            module.request_attributes.push(attributes);
            Ok(at)
        }
    }
    fn module(&self, statements: &[ast::Statement<'_>]) -> R<(Vec<ir::Statement>, ir::Module)> {
        let mut module = ir::Module::default();
        let mut body = Vec::new();
        for statement in statements {
            match statement {
                ast::Statement::ImportDeclaration(import) => {
                    if import.phase.is_some() {
                        return Err(self.unsupported(import.span, "import phases not implemented"));
                    }
                    let request = self.request(&mut module, &import.source, import.with_clause.as_deref())?;
                    if let Some(specifiers) = &import.specifiers {
                        for specifier in specifiers {
                            let (id, name) = match specifier {
                                ast::ImportDeclarationSpecifier::ImportSpecifier(s) => {
                                    (&s.local, Self::export_name(&s.imported))
                                }
                                ast::ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                    (&s.local, "default".encode_utf16().collect())
                                }
                                ast::ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                                    (&s.local, vec![b'*' as u16])
                                }
                            };
                            module.imports.push(ir::Import {
                                binding: self.binding(id, ir::DeclarationKind::Import),
                                request,
                                name,
                            });
                        }
                    }
                }
                ast::Statement::ExportDeclaration(export) => {
                    let kind = match &export.declaration {
                        ast::Declaration::VariableDeclaration(d) => self.declaration(d)?,
                        ast::Declaration::ClassDeclaration(class) => {
                            let id = class.id.as_ref().ok_or_else(|| {
                                self.unsupported(class.span, "exported class requires a name")
                            })?;
                            ir::StatementKind::Class {
                                binding: self.binding(id, ir::DeclarationKind::Let),
                                class: self.class(class)?,
                            }
                        }
                        ast::Declaration::FunctionDeclaration(f) => {
                            let id = f.id.as_ref().ok_or_else(|| {
                                self.unsupported(f.span, "exported function name")
                            })?;
                            ir::StatementKind::Function {
                                binding: self.binding(id, ir::DeclarationKind::Function),
                                function: self.function(f)?,
                            }
                        }
                        _ => {
                            return Err(
                                self.unsupported(export.span, "export declaration not implemented")
                            );
                        }
                    };
                    match &kind {
                        ir::StatementKind::Declaration(declarations) => {
                            for d in declarations {
                                d.pattern.for_each_binding(&mut |binding| {
                                    module.exports.push(ir::Export::Local {
                                        symbol: binding.symbol,
                                        name: binding.name.encode_utf16().collect(),
                                    });
                                });
                            }
                        }
                        ir::StatementKind::Function { binding, .. }
                        | ir::StatementKind::Class { binding, .. } => {
                            module.exports.push(ir::Export::Local {
                                symbol: binding.symbol,
                                name: binding.name.encode_utf16().collect(),
                            })
                        }
                        _ => unreachable!(),
                    }
                    body.push(ir::Statement {
                        kind,
                        span: self.span(export.span),
                    });
                }
                ast::Statement::ExportNamedDeclaration(export) => {
                    for specifier in &export.specifiers {
                        let symbol = match &specifier.local {
                            ast::ModuleExportName::IdentifierReference(id) => {
                                self.reference(id).symbol
                            }
                            ast::ModuleExportName::IdentifierName(id) => self
                                .scoping
                                .get_root_binding(id.name)
                                .map(|s| s.index() as u32),
                            _ => None,
                        }
                        .ok_or_else(|| self.unsupported(specifier.span, "unbound export name"))?;
                        module.exports.push(ir::Export::Local {
                            symbol,
                            name: Self::export_name(&specifier.exported),
                        });
                    }
                }
                ast::Statement::ExportFromDeclaration(export) => {
                    let request = self.request(&mut module, &export.source, export.with_clause.as_deref())?;
                    for specifier in &export.specifiers {
                        module.exports.push(ir::Export::Indirect {
                            request,
                            import: Self::export_name(&specifier.local),
                            name: Self::export_name(&specifier.exported),
                        });
                    }
                }
                ast::Statement::ExportAllDeclaration(export) => {
                    let request = self.request(&mut module, &export.source, export.with_clause.as_deref())?;
                    if let Some(name) = &export.exported {
                        module.exports.push(ir::Export::Indirect {
                            request,
                            import: vec![b'*' as u16],
                            name: Self::export_name(name),
                        });
                    } else {
                        module.exports.push(ir::Export::Star { request });
                    }
                }
                ast::Statement::ExportDefaultDeclaration(export) => {
                    let (binding, kind) =
                        if let ast::ExportDefaultDeclarationKind::FunctionDeclaration(function) =
                            &export.declaration
                        {
                            let binding = if let Some(id) = &function.id {
                                self.binding(id, ir::DeclarationKind::Function)
                            } else {
                                let symbol = self.next_symbol();
                                ir::Binding {
                                    symbol,
                                    scope: 0,
                                    name: "*default*".into(),
                                    kind: ir::DeclarationKind::Function,
                                    span: self.span(export.span),
                                }
                            };
                            let mut function = self.function(function)?;
                            if function.name.is_none() {
                                function.name = Some("default".into());
                            }
                            (
                                binding.clone(),
                                ir::StatementKind::Function { binding, function },
                            )
                        } else if let ast::ExportDefaultDeclarationKind::ClassDeclaration(class) =
                            &export.declaration
                        {
                            let binding = if let Some(id) = &class.id {
                                self.binding(id, ir::DeclarationKind::Let)
                            } else {
                                ir::Binding {
                                    symbol: self.next_symbol(),
                                    scope: 0,
                                    name: "*default*".into(),
                                    kind: ir::DeclarationKind::Const,
                                    span: self.span(export.span),
                                }
                            };
                            let mut class = self.class(class)?;
                            if class.name.is_none() {
                                class.name = Some("default".into());
                            }
                            (binding.clone(), ir::StatementKind::Class { binding, class })
                        } else {
                            let expression =
                                export.declaration.as_expression().ok_or_else(|| {
                                    self.unsupported(
                                        export.span,
                                        "default export declaration not implemented",
                                    )
                                })?;
                            let symbol = self.next_symbol();
                            let binding = ir::Binding {
                                symbol,
                                scope: 0,
                                name: "*default*".into(),
                                kind: ir::DeclarationKind::Const,
                                span: self.span(export.span),
                            };
                            let mut initializer = self.expression(expression)?;
                            if let ir::ExpressionKind::Function(function) = &mut initializer.kind
                                && function.name.is_none()
                            {
                                function.name = Some("default".into());
                            }
                            if let ir::ExpressionKind::Class(class) = &mut initializer.kind
                                && class.name.is_none()
                            {
                                class.name = Some("default".into());
                            }
                            (
                                binding.clone(),
                                ir::StatementKind::Declaration(vec![ir::Declarator {
                                    pattern: ir::Pattern::Binding {
                                        binding,
                                        initialization_target: None,
                                    },
                                    using: None,
                                    initializer: Some(initializer),
                                }]),
                            )
                        };
                    module.exports.push(ir::Export::Local {
                        symbol: binding.symbol,
                        name: "default".encode_utf16().collect(),
                    });
                    body.push(ir::Statement {
                        kind,
                        span: self.span(export.span),
                    });
                }
                _ => body.push(self.statement(statement)?),
            }
        }
        Ok((body, module))
    }
}

/// Optional lifecycle observation covers only actual Parser::parse calls, never
/// parser construction, AST capacity checks, semantic analysis or lowering.
/// Parser-only member suffix that turns an Annex B call target into one OXC can represent.
const CALL_TARGET_SUFFIX: &str = ".a";
/// Annex B call assignment targets (`f() = 1`, `f()++`, `for (f() in o)`): OXC has no AST
/// for them and stops at the first one. Each gets `CALL_TARGET_SUFFIX` in the parse view,
/// and lowering (`Lower::call_target`) turns the suffixed member back into the call.
fn call_targets(
    input: &crate::source::SourceInput,
    source_type: oxc_span::SourceType,
    observer: &mut Option<&mut dyn FnMut(ParsePhase)>,
) -> crate::source::SourceInput {
    let mut view = input.clone();
    loop {
        let arena = oxc_allocator::Allocator::default();
        let parsed = parse(&arena, view.parser_text(), source_type, observer);
        let Some(end) = call_target_end(&parsed.diagnostics, &view) else {
            return view;
        };
        view = view.with_insertion(end, CALL_TARGET_SUFFIX);
    }
}
/// End of the first target OXC refused that may be a call: its text ends in `)` and has
/// no suffix yet, so each `)` is suffixed at most once.
fn call_target_end(
    diagnostics: &[oxc_diagnostics::OxcDiagnostic],
    view: &crate::source::SourceInput,
) -> Option<usize> {
    diagnostics.iter().find_map(|e| {
        let end = crate::error_span(e).end;
        (e.to_string() == "Cannot assign to this expression"
            && view.parser_text()[..end as usize].ends_with(')')
            && !view.insertion_at(end))
        .then_some(end as usize)
    })
}
fn parse<'a>(
    arena: &'a oxc_allocator::Allocator,
    text: &'a str,
    source_type: oxc_span::SourceType,
    observer: &mut Option<&mut dyn FnMut(ParsePhase)>,
) -> oxc_parser::ParserReturn<'a> {
    let parser = oxc_parser::Parser::new(arena, text, source_type);
    if let Some(observer) = observer.as_mut() {
        observer(ParsePhase::Begin);
    }
    let parsed = parser.parse();
    if let Some(observer) = observer.as_mut() {
        observer(ParsePhase::End);
    }
    parsed
}
pub fn compile_program_with_limits(
    source: &str,
    filename: &str,
    mode: SourceMode,
    limits: &CompileLimits,
) -> Result<ir::Program, Vec<Diagnostic>> {
    let input =
        SourceInput::from_utf16(&source.encode_utf16().collect::<Vec<_>>()).map_err(|message| {
            vec![Diagnostic {
                category: Category::Capacity,
                filename: filename.into(),
                span: qjs_ir::Span { start: 0, end: 0 },
                message: message.into(),
            }]
        })?;
    compile_input(&input, filename, mode, false, limits, None)
}
pub fn compile_program_input_with_limits(
    input: &SourceInput,
    filename: &str,
    limits: &CompileLimits,
) -> Result<ir::Program, Vec<Diagnostic>> {
    compile_input(input, filename, SourceMode::Script, false, limits, None)
}
pub fn compile_module_input_with_limits(
    input: &SourceInput,
    filename: &str,
    limits: &CompileLimits,
) -> Result<ir::Program, Vec<Diagnostic>> {
    compile_input(input, filename, SourceMode::Module, false, limits, None)
}
pub fn compile_eval_program_input_with_limits(
    input: &SourceInput,
    filename: &str,
    inherited_strict: bool,
    limits: &CompileLimits,
) -> Result<ir::Program, Vec<Diagnostic>> {
    compile_input(
        input,
        filename,
        SourceMode::Script,
        inherited_strict,
        limits,
        None,
    )
}
pub fn compile_program_input_with_limits_and_observer(
    input: &SourceInput,
    filename: &str,
    limits: &CompileLimits,
    observer: &mut dyn FnMut(ParsePhase),
) -> Result<ir::Program, Vec<Diagnostic>> {
    compile_input(
        input,
        filename,
        SourceMode::Script,
        false,
        limits,
        Some(observer),
    )
}
pub fn compile_module_input_with_limits_and_observer(
    input: &SourceInput,
    filename: &str,
    limits: &CompileLimits,
    observer: &mut dyn FnMut(ParsePhase),
) -> Result<ir::Program, Vec<Diagnostic>> {
    compile_input(
        input,
        filename,
        SourceMode::Module,
        false,
        limits,
        Some(observer),
    )
}
pub fn compile_eval_program_input_with_limits_and_observer(
    input: &SourceInput,
    filename: &str,
    inherited_strict: bool,
    limits: &CompileLimits,
    observer: &mut dyn FnMut(ParsePhase),
) -> Result<ir::Program, Vec<Diagnostic>> {
    compile_eval_frame_program(
        input,
        filename,
        inherited_strict,
        false,
        &[],
        limits,
        observer,
    )
}
/// Eval code for a caller frame; `new_target` admits top-level `new.target`,
/// and `outer_private` names (without `#`) resolve to private ids `0..n`.
pub fn compile_eval_frame_program(
    input: &SourceInput,
    filename: &str,
    inherited_strict: bool,
    new_target: bool,
    outer_private: &[String],
    limits: &CompileLimits,
    observer: &mut dyn FnMut(ParsePhase),
) -> Result<ir::Program, Vec<Diagnostic>> {
    compile_input_in(
        input,
        filename,
        SourceMode::Script,
        inherited_strict,
        new_target,
        outer_private,
        limits,
        Some(observer),
    )
}
