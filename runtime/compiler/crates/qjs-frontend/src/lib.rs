//! OXC parser and semantic analysis for the bounded scalar JavaScript subset.
mod budget;
pub mod source;
pub use budget::{CompileLimits, ParsePhase};
use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_span::GetSpan;
use oxc_syntax::{
    operator::{BinaryOperator as B, UnaryOperator as U},
    symbol::SymbolId,
};
use qjs_ir::*;
use qjs_ir::{Block, Span};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Capacity,
    Parse,
    Semantic,
    UnsupportedSyntax,
    UnsupportedType,
    UnsupportedSemantics,
    InvalidIr,
}
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub category: Category,
    pub filename: String,
    pub span: Span,
    pub message: String,
}
#[derive(Clone, Debug)]
pub struct SourceMetadata {
    /// Known original source and its coordinate map; None for manually supplied IR.
    pub source: Option<source::SourceInput>,
    pub filename: String,
    pub source_hash: String,
    pub source_len: u32,
    pub function_name: String,
    pub strict: bool,
    pub function_span: Span,
}
#[derive(Clone, Debug)]
pub struct FrontendOutput {
    pub function: qjs_ir::Function,
    pub metadata: SourceMetadata,
}
fn span(s: oxc_span::Span) -> Span {
    Span {
        start: s.start,
        end: s.end,
    }
}
/// Compile a JavaScript Script containing one standalone `main` function.
pub fn compile(source: &str, filename: &str) -> Result<qjs_ir::Function, Vec<Diagnostic>> {
    compile_with_metadata(source, filename).map(|x| x.function)
}
/// Scalar convenience entry with post-parse source AST limits.
pub fn compile_with_limits(
    source: &str,
    filename: &str,
    limits: &CompileLimits,
) -> Result<qjs_ir::Function, Vec<Diagnostic>> {
    compile_with_metadata_with_limits(source, filename, limits).map(|output| output.function)
}
/// Parse once, retaining owned provenance for the native backend.
pub fn compile_with_metadata(
    source: &str,
    filename: &str,
) -> Result<FrontendOutput, Vec<Diagnostic>> {
    compile_with_metadata_with_limits(source, filename, &CompileLimits::default())
}
/// Scalar entry with the same post-parse/pre-semantic source AST limits.
pub fn compile_with_metadata_with_limits(
    source: &str,
    filename: &str,
    limits: &CompileLimits,
) -> Result<FrontendOutput, Vec<Diagnostic>> {
    let input = source::SourceInput::from_utf16(&source.encode_utf16().collect::<Vec<_>>())
        .map_err(|message| {
            vec![Diagnostic {
                category: Category::Capacity,
                filename: filename.into(),
                span: Span { start: 0, end: 0 },
                message: message.into(),
            }]
        })?;
    budget::validate_limits(limits, &input, filename)?;
    let arena = Allocator::default();
    let parsed = oxc_parser::Parser::new(&arena, source, oxc_span::SourceType::cjs()).parse();
    let diagnostic = |category, s: Span, message: String| Diagnostic {
        category,
        filename: filename.into(),
        span: s,
        message,
    };
    if !parsed.diagnostics.is_empty() {
        return Err(parsed
            .diagnostics
            .iter()
            .map(|e| diagnostic(Category::Parse, error_span(e), e.to_string()))
            .collect());
    }
    budget::check(&parsed.program, &input, filename, limits).map_err(|mut errors| {
        // The legacy scalar API reports UTF-8 bytes; general owned-JS APIs
        // report original UTF-16 units. Budget traversal is shared by both.
        for error in &mut errors {
            error.span.start = input
                .parser_offset(error.span.start)
                .expect("scalar source span starts at a Unicode scalar boundary");
            error.span.end = input
                .parser_offset(error.span.end)
                .expect("scalar source span ends at a Unicode scalar boundary");
        }
        errors
    })?;
    let sem = oxc_semantic::SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&parsed.program);
    if !sem.diagnostics.is_empty() {
        return Err(sem
            .diagnostics
            .iter()
            .map(|e| diagnostic(Category::Semantic, error_span(e), e.to_string()))
            .collect());
    }
    let program = &parsed.program;
    let bad = |s, message: &str| vec![diagnostic(Category::UnsupportedSyntax, s, message.into())];
    if program.body.len() != 1 || !program.directives.is_empty() {
        return Err(bad(
            span(program.span),
            "expected exactly one function declaration named main",
        ));
    }
    let Statement::FunctionDeclaration(f) = &program.body[0] else {
        return Err(bad(span(program.span), "expected function main"));
    };
    if f.id.as_ref().is_none_or(|id| id.name != "main")
        || f.r#async
        || f.generator
        || !f.params.items.is_empty()
        || f.params.rest.is_some()
    {
        return Err(bad(
            span(f.span),
            "main must be an ordinary function without parameters",
        ));
    }
    let body = f
        .body
        .as_ref()
        .ok_or_else(|| bad(span(f.span), "missing function body"))?;
    if body.directives.iter().any(|d| d.directive != "use strict") {
        return Err(bad(
            span(body.span),
            "only use strict directive is supported",
        ));
    }
    let mut lower = Lower {
        filename,
        scoping: sem.semantic.scoping(),
        blocks: Vec::new(),
        next: 0,
        env: BTreeMap::new(),
        current: 0,
        return_type: None,
    };
    lower.new_block(span(body.span));
    // First pass checks unreachable statements too. Its IR is discarded.
    let mut checker = lower.clone();
    checker.list(&body.statements, true).map_err(|e| vec![e])?;
    let live = lower.list(&body.statements, false).map_err(|e| vec![e])?;
    if live {
        return Err(vec![diagnostic(
            Category::UnsupportedSemantics,
            span(body.span),
            "every reachable path must return a scalar".into(),
        )]);
    }
    let function = qjs_ir::Function {
        source_len: source.len() as u32,
        entry: BlockId(0),
        return_type: lower.return_type.unwrap_or(Type::Number),
        blocks: lower.blocks,
    };
    validate(&function).map_err(|e| {
        vec![diagnostic(
            if e.iter()
                .any(|error| error.kind == qjs_ir::ValidationErrorKind::Capacity)
            {
                Category::Capacity
            } else {
                Category::InvalidIr
            },
            span(f.span),
            format!("{e:?}"),
        )]
    })?;
    Ok(FrontendOutput {
        function,
        metadata: SourceMetadata {
            source: Some(input),
            filename: filename.into(),
            source_hash: format!("{:x}", Sha256::digest(source.as_bytes())),
            source_len: source.len() as u32,
            function_name: "main".into(),
            strict: body.directives.iter().any(|d| d.directive == "use strict"),
            function_span: span(f.span),
        },
    })
}
fn error_span(e: &oxc_diagnostics::OxcDiagnostic) -> Span {
    e.labels
        .first()
        .map(|l| Span {
            start: l.offset(),
            end: l.offset() + l.len(),
        })
        .unwrap_or(Span { start: 0, end: 0 })
}
#[derive(Clone, Copy)]
struct Binding {
    value: ValueId,
    ty: Type,
    mutable: bool,
}
#[derive(Clone)]
struct Lower<'a> {
    filename: &'a str,
    scoping: &'a oxc_semantic::Scoping,
    blocks: Vec<Block>,
    next: u32,
    env: BTreeMap<SymbolId, Binding>,
    current: usize,
    return_type: Option<Type>,
}
type R<T> = Result<T, Diagnostic>;
impl Lower<'_> {
    fn err(&self, category: Category, s: oxc_span::Span, message: &str) -> Diagnostic {
        Diagnostic {
            category,
            filename: self.filename.into(),
            span: span(s),
            message: message.into(),
        }
    }
    fn new_block(&mut self, s: Span) -> usize {
        let n = self.blocks.len();
        self.blocks.push(Block {
            id: BlockId(n as u32),
            params: vec![],
            instructions: vec![],
            terminator: Terminator {
                kind: TerminatorKind::Return(ValueId(0)),
                span: s,
            },
            span: s,
        });
        n
    }
    fn emit(&mut self, kind: InstructionKind, s: oxc_span::Span) -> (ValueId, Type) {
        let v = ValueId(self.next);
        self.next += 1;
        let ty = kind.result_type();
        self.blocks[self.current].instructions.push(Instruction {
            result: v,
            kind,
            span: span(s),
        });
        (v, ty)
    }
    fn symbol(&self, id: &IdentifierReference<'_>) -> R<SymbolId> {
        id.reference_id
            .get()
            .and_then(|r| self.scoping.get_reference(r).symbol_id())
            .ok_or_else(|| self.err(Category::Semantic, id.span, "unresolved identifier"))
    }
    fn read(&self, id: &IdentifierReference<'_>) -> R<Binding> {
        let s = self.symbol(id)?;
        self.env.get(&s).copied().ok_or_else(|| {
            self.err(
                Category::Semantic,
                id.span,
                "binding read before initialization",
            )
        })
    }
    fn expr(&mut self, e: &Expression<'_>) -> R<(ValueId, Type)> {
        Ok(match e {
            Expression::NumericLiteral(x) => {
                self.emit(InstructionKind::Constant(Scalar::Number(x.value)), x.span)
            }
            Expression::BooleanLiteral(x) => {
                self.emit(InstructionKind::Constant(Scalar::Bool(x.value)), x.span)
            }
            Expression::ParenthesizedExpression(x) => self.expr(&x.expression)?,
            Expression::Identifier(x) => {
                let b = self.read(x)?;
                (b.value, b.ty)
            }
            Expression::UnaryExpression(x) => {
                let (v, t) = self.expr(&x.argument)?;
                let (op, ty) = match x.operator {
                    U::UnaryNegation => (UnaryOp::Negate, Type::Number),
                    U::LogicalNot => (UnaryOp::Not, Type::Bool),
                    _ => {
                        return Err(self.err(
                            Category::UnsupportedSyntax,
                            x.span,
                            "unsupported unary operator",
                        ));
                    }
                };
                if t != ty {
                    return Err(self.err(
                        Category::UnsupportedType,
                        x.span,
                        "unary operand has wrong scalar type",
                    ));
                }
                self.emit(InstructionKind::Unary { op, operand: v }, x.span)
            }
            Expression::BinaryExpression(x) => {
                let (a, ta) = self.expr(&x.left)?;
                let (b, tb) = self.expr(&x.right)?;
                let op = match x.operator {
                    B::Addition => BinaryOp::Add,
                    B::Subtraction => BinaryOp::Subtract,
                    B::Multiplication => BinaryOp::Multiply,
                    B::Division => BinaryOp::Divide,
                    B::Remainder => BinaryOp::Remainder,
                    B::LessThan => BinaryOp::LessThan,
                    B::LessEqualThan => BinaryOp::LessEqual,
                    B::GreaterThan => BinaryOp::GreaterThan,
                    B::GreaterEqualThan => BinaryOp::GreaterEqual,
                    B::StrictEquality | B::StrictInequality => BinaryOp::Equal,
                    _ => {
                        return Err(self.err(
                            Category::UnsupportedSyntax,
                            x.span,
                            "unsupported binary operator",
                        ));
                    }
                };
                if ta != tb || (op != BinaryOp::Equal && ta != Type::Number) {
                    return Err(self.err(
                        Category::UnsupportedType,
                        x.span,
                        "binary operands have incompatible scalar types",
                    ));
                }
                let result = self.emit(
                    InstructionKind::Binary {
                        op,
                        left: a,
                        right: b,
                    },
                    x.span,
                );
                if x.operator == B::StrictInequality {
                    self.emit(
                        InstructionKind::Unary {
                            op: UnaryOp::Not,
                            operand: result.0,
                        },
                        x.span,
                    )
                } else {
                    result
                }
            }
            _ => {
                return Err(self.err(
                    Category::UnsupportedSyntax,
                    e.span(),
                    "unsupported expression",
                ));
            }
        })
    }
    fn list(&mut self, list: &oxc_allocator::Vec<'_, Statement<'_>>, check_dead: bool) -> R<bool> {
        let mut live = true;
        for stmt in list {
            if !live {
                if !check_dead {
                    continue;
                }
                self.current = self.new_block(span(stmt.span()));
            }
            let next = self.stmt(stmt, check_dead)?;
            live = live && next;
        }
        Ok(live)
    }
    fn stmt(&mut self, s: &Statement<'_>, check_dead: bool) -> R<bool> {
        match s {
            Statement::BlockStatement(x) => {
                let outer = self.env.clone();
                let live = self.list(&x.body, check_dead)?;
                self.env.retain(|k, _| outer.contains_key(k));
                Ok(live)
            }
            Statement::VariableDeclaration(x) => {
                if !matches!(
                    x.kind,
                    VariableDeclarationKind::Let | VariableDeclarationKind::Const
                ) {
                    return Err(self.err(
                        Category::UnsupportedSyntax,
                        x.span,
                        "only let and const declarations supported",
                    ));
                }
                for d in &x.declarations {
                    let BindingPattern::BindingIdentifier(id) = &d.id else {
                        return Err(self.err(
                            Category::UnsupportedSyntax,
                            d.span,
                            "destructuring is unsupported",
                        ));
                    };
                    let init = d.init.as_ref().ok_or_else(|| {
                        self.err(
                            Category::UnsupportedSemantics,
                            d.span,
                            "initializer required",
                        )
                    })?;
                    let (value, ty) = self.expr(init)?;
                    self.env.insert(
                        id.symbol_id.get().expect("semantic binding"),
                        Binding {
                            value,
                            ty,
                            mutable: x.kind == VariableDeclarationKind::Let,
                        },
                    );
                }
                Ok(true)
            }
            Statement::ExpressionStatement(x) => {
                let Expression::AssignmentExpression(a) = &x.expression else {
                    return Err(self.err(
                        Category::UnsupportedSyntax,
                        x.span,
                        "only assignment expression statements supported",
                    ));
                };
                if a.operator != oxc_syntax::operator::AssignmentOperator::Assign {
                    return Err(self.err(
                        Category::UnsupportedSyntax,
                        a.span,
                        "compound assignment unsupported",
                    ));
                }
                let AssignmentTarget::AssignmentTargetIdentifier(id) = &a.left else {
                    return Err(self.err(
                        Category::UnsupportedSyntax,
                        a.span,
                        "only local assignment supported",
                    ));
                };
                let old = self.read(id)?;
                if !old.mutable {
                    return Err(self.err(
                        Category::Semantic,
                        id.span,
                        "cannot assign const binding",
                    ));
                }
                let (value, ty) = self.expr(&a.right)?;
                if ty != old.ty {
                    return Err(self.err(
                        Category::UnsupportedType,
                        a.span,
                        "assignment changes scalar type",
                    ));
                }
                self.env.insert(self.symbol(id)?, Binding { value, ..old });
                Ok(true)
            }
            Statement::ReturnStatement(x) => {
                let e = x.argument.as_ref().ok_or_else(|| {
                    self.err(Category::UnsupportedType, x.span, "return scalar required")
                })?;
                let (value, ty) = self.expr(e)?;
                if !check_dead && self.return_type.is_some_and(|t| t != ty) {
                    return Err(self.err(Category::UnsupportedType, x.span, "return types differ"));
                }
                self.return_type = Some(ty);
                self.blocks[self.current].terminator = Terminator {
                    kind: TerminatorKind::Return(value),
                    span: span(x.span),
                };
                Ok(false)
            }
            Statement::IfStatement(x) => {
                let (condition, ty) = self.expr(&x.test)?;
                if ty != Type::Bool {
                    return Err(self.err(
                        Category::UnsupportedType,
                        x.test.span(),
                        "if condition must be Bool",
                    ));
                }
                let parent = self.current;
                let env = self.env.clone();
                let then = self.new_block(span(x.consequent.span()));
                self.current = then;
                let tl = self.stmt(&x.consequent, check_dead)?;
                let tend = self.current;
                let tenv = self.env.clone();
                self.env = env.clone();
                let otherwise = self.new_block(span(x.span));
                self.current = otherwise;
                let el = if let Some(s) = &x.alternate {
                    self.stmt(s, check_dead)?
                } else {
                    true
                };
                let eend = self.current;
                let eenv = self.env.clone();
                self.blocks[parent].terminator = Terminator {
                    kind: TerminatorKind::Branch {
                        condition,
                        then_edge: Edge {
                            target: BlockId(then as u32),
                            arguments: vec![],
                        },
                        else_edge: Edge {
                            target: BlockId(otherwise as u32),
                            arguments: vec![],
                        },
                    },
                    span: span(x.span),
                };
                if !tl && !el {
                    return Ok(false);
                }
                let join = self.new_block(span(x.span));
                let mut ta = vec![];
                let mut ea = vec![];
                self.env = env.clone();
                for (k, old) in env {
                    let t = tenv.get(&k).copied().unwrap_or(old);
                    let e = eenv.get(&k).copied().unwrap_or(old);
                    let b = if tl && el && t.value != e.value {
                        let v = ValueId(self.next);
                        self.next += 1;
                        self.blocks[join].params.push(Parameter {
                            value: v,
                            ty: t.ty,
                            span: span(x.span),
                        });
                        ta.push(t.value);
                        ea.push(e.value);
                        Binding { value: v, ..t }
                    } else if tl {
                        t
                    } else {
                        e
                    };
                    self.env.insert(k, b);
                }
                if tl {
                    self.blocks[tend].terminator = Terminator {
                        kind: TerminatorKind::Jump(Edge {
                            target: BlockId(join as u32),
                            arguments: ta,
                        }),
                        span: span(x.span),
                    };
                }
                if el {
                    self.blocks[eend].terminator = Terminator {
                        kind: TerminatorKind::Jump(Edge {
                            target: BlockId(join as u32),
                            arguments: ea,
                        }),
                        span: span(x.span),
                    };
                }
                self.current = join;
                Ok(true)
            }
            _ => Err(self.err(
                Category::UnsupportedSyntax,
                s.span(),
                "unsupported statement",
            )),
        }
    }
}

/// Owned general script lowering, separate from the preserved scalar entry API.
pub mod js;
pub use js::{
    SourceMode, compile_eval_frame_program, compile_eval_program_input,
    compile_eval_program_input_with_limits, compile_eval_program_input_with_limits_and_observer,
    compile_module_input, compile_module_input_with_limits,
    compile_module_input_with_limits_and_observer, compile_program, compile_program_input,
    compile_program_input_with_limits, compile_program_input_with_limits_and_observer,
    compile_program_with_limits,
};
