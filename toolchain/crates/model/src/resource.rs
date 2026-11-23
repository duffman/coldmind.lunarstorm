//! Reads one resource (`app.tsx` or a `*.view.tsx`) from its AST into an
//! [`Element`] tree, enforcing the parse rules of spec 2.2.
//!
//! Decisions:
//! - The file is parsed, never executed and never type-checked here; `tsc` runs
//!   out of band (spec 4, "Typecheck").
//! - A resource is a configuration file, so its top level may hold only imports,
//!   `declare` statements, type declarations and the default export. Anything
//!   that would run is a compile error rather than silently ignored.
//! - Every error in a file is reported, not only the first: an element with a bad
//!   prop is still walked so its children are checked too.
//! - A prop without a value is `true`, as in JSX. A template literal without
//!   substitutions is a string literal. A signed number is a numeric literal
//!   (`itemIndex={-1}` must be writable).

use std::collections::HashMap;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrayExpressionElement, ExportDefaultDeclarationKind, Expression, JSXAttributeItem, JSXAttributeName,
    JSXAttributeValue, JSXChild, JSXElement, JSXElementName, JSXExpression, Statement,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::operator::UnaryOperator;

use crate::model::{Code, Diagnostic, Element, Position, Prop, Value};
use crate::schema::{self, Children, Kind};

/// Collects the diagnostics of one file and turns byte offsets into positions.
pub(crate) struct Report<'a> {
    /// The file, relative to the project root.
    file: &'a Path,
    /// The file's text, for offset to line/column conversion.
    source: &'a str,
    /// Where the diagnostics go.
    out: &'a mut Vec<Diagnostic>,
}

impl<'a> Report<'a> {
    /// Starts a report for one file.
    ///
    /// # Arguments
    /// * `file` - the file, relative to the project root.
    /// * `source` - the file's text.
    /// * `out` - the list diagnostics are appended to.
    ///
    /// # Returns
    /// The report.
    pub(crate) fn new(file: &'a Path, source: &'a str, out: &'a mut Vec<Diagnostic>) -> Self {
        Self { file, source, out }
    }

    /// Converts a byte offset into a 1-based line and column.
    ///
    /// # Arguments
    /// * `offset` - byte offset into the file's text.
    ///
    /// # Returns
    /// The position; an offset past the end maps to the end.
    pub(crate) fn position(&self, offset: u32) -> Position {
        let end = (offset as usize).min(self.source.len());
        let before = &self.source[..end];
        let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
        let start = before.rfind('\n').map_or(0, |index| index + 1);
        Position { line, column: before[start..].chars().count() as u32 + 1 }
    }

    /// Records one compile error.
    ///
    /// # Arguments
    /// * `code` - the kind of error.
    /// * `span` - where it is.
    /// * `message` - the text for the developer.
    pub(crate) fn error(&mut self, code: Code, span: Span, message: impl Into<String>) {
        let at = self.position(span.start);
        self.out.push(Diagnostic { code, file: self.file.to_path_buf(), at, message: message.into() });
    }
}

/// Parses a source file and reports its syntax errors.
///
/// # Arguments
/// * `allocator` - the arena the AST lives in.
/// * `source` - the file's text.
/// * `kind` - the grammar (TSX for resources, TS for form classes).
/// * `report` - receives syntax errors.
///
/// # Returns
/// The program, or `None` when the file has a syntax error.
pub(crate) fn parse<'a>(
    allocator: &'a Allocator,
    source: &'a str,
    kind: SourceType,
    report: &mut Report<'_>,
) -> Option<oxc_ast::ast::Program<'a>> {
    let parsed = Parser::new(allocator, source, kind).parse();
    let mut failed = false;
    for diagnostic in parsed.diagnostics.iter() {
        failed = true;
        let start = diagnostic.labels.first().map_or(0, |label| label.offset());
        report.error(Code::Syntax, Span::new(start, start), diagnostic.message.to_string());
    }
    (!failed).then_some(parsed.program)
}

/// Reads one resource into its element tree.
///
/// # Arguments
/// * `source` - the file's text.
/// * `root` - the tag the default export must be (`application` or `form`).
/// * `report` - receives every compile error in the file.
///
/// # Returns
/// The root element, or `None` when the file has no usable root. A returned tree
/// may still have errors in `report`; the caller discards the model then.
pub(crate) fn read(source: &str, root: &'static str, report: &mut Report<'_>) -> Option<Element> {
    let allocator = Allocator::default();
    let program = parse(&allocator, source, SourceType::tsx(), report)?;
    let mut exported = None;
    for statement in &program.body {
        match statement {
            Statement::ImportDeclaration(_)
            | Statement::TSTypeAliasDeclaration(_)
            | Statement::TSInterfaceDeclaration(_) => {}
            Statement::VariableDeclaration(declaration) if declaration.declare => {}
            Statement::ExportDefaultDeclaration(export) => exported = Some(export),
            other => report.error(
                Code::Statement,
                other.span(),
                "a resource holds only imports, declarations and its default export; it is never executed",
            ),
        }
    }
    let expected = format!("a resource default-exports one <{root}> element");
    let Some(export) = exported else {
        report.error(Code::Root, Span::new(0, 0), expected);
        return None;
    };
    let element = match &export.declaration {
        ExportDefaultDeclarationKind::FunctionDeclaration(_)
        | ExportDefaultDeclarationKind::ClassDeclaration(_)
        | ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => None,
        other => match other.to_expression().without_parentheses() {
            Expression::JSXElement(element) => Some(element),
            _ => None,
        },
    };
    let Some(element) = element else {
        report.error(Code::Root, export.span, expected);
        return None;
    };
    let mut names = HashMap::new();
    let tree = Reader { report, names: &mut names }.element(element, None)?;
    if tree.tag != root {
        report.error(Code::Root, element.span, expected);
        return None;
    }
    Some(tree)
}

/// Walks the JSX of one resource.
struct Reader<'r, 'a> {
    /// Receives compile errors.
    report: &'r mut Report<'a>,
    /// Names already used in this resource, for the uniqueness rule (spec 2.1).
    names: &'r mut HashMap<String, Position>,
}

impl Reader<'_, '_> {
    /// Reads one element and its subtree.
    ///
    /// # Arguments
    /// * `node` - the JSX element.
    /// * `parent` - the containing element's row, or `None` at the root.
    ///
    /// # Returns
    /// The element, or `None` when its tag is not part of the model; its subtree
    /// is then skipped, because nesting cannot be judged under an unknown tag.
    fn element(&mut self, node: &JSXElement<'_>, parent: Option<&Kind>) -> Option<Element> {
        let opening = &node.opening_element;
        let kind = match &opening.name {
            JSXElementName::Identifier(identifier) => match schema::lookup(identifier.name.as_str()) {
                Some(kind) => kind,
                None => {
                    let message = format!("<{}> is not an element of the model", identifier.name);
                    self.report.error(Code::Tag, identifier.span, message);
                    return None;
                }
            },
            other => {
                self.report.error(
                    Code::Tag,
                    other.span(),
                    "only lower-case element names are valid; a resource has no components",
                );
                return None;
            }
        };
        match parent {
            Some(parent) if !schema::accepts(parent, kind) => {
                let message = match parent.children {
                    Children::None => format!("<{}> cannot have children", parent.tag),
                    _ => format!("<{}> cannot sit inside <{}>", kind.tag, parent.tag),
                };
                self.report.error(Code::Nesting, opening.name.span(), message);
            }
            None if kind.place != schema::Place::Root => {
                // Reported by `read` as a wrong root; nothing more to say here.
            }
            _ => {}
        }

        let at = self.report.position(node.span.start);
        let mut element = Element { tag: kind.tag, name: String::new(), props: Vec::new(), children: Vec::new(), at };
        let mut named = false;
        for item in &opening.attributes {
            let attribute = match item {
                JSXAttributeItem::Attribute(attribute) => attribute,
                JSXAttributeItem::SpreadAttribute(spread) => {
                    self.report.error(Code::Value, spread.span, "a spread is not a prop; write each prop out");
                    continue;
                }
            };
            let name = match &attribute.name {
                JSXAttributeName::Identifier(identifier) => identifier.name.as_str(),
                JSXAttributeName::NamespacedName(name) => {
                    self.report.error(Code::Value, name.span, "a namespaced prop name is not valid");
                    continue;
                }
            };
            let Some(value) = self.value(attribute.value.as_ref(), attribute.span) else { continue };
            // The application's `name` is its display name (spec 2.6), an ordinary prop.
            if name == "name" && kind.named {
                named = true;
                self.name(&mut element, value, attribute.span);
                continue;
            }
            let event = is_event(name);
            match (&value, event) {
                (Value::Handler(_), false) => {
                    let message = format!("'{name}' is not an event; only an event prop names a handler");
                    self.report.error(Code::Event, attribute.span, message);
                    continue;
                }
                (Value::Handler(_), true) => {}
                (_, true) => {
                    let message = format!("'{name}' is an event; its value is form.<member>");
                    self.report.error(Code::Event, attribute.span, message);
                    continue;
                }
                (_, false) => {}
            }
            if element.props.iter().any(|prop| prop.name == name) {
                self.report.error(Code::Repeated, attribute.span, format!("'{name}' is set twice"));
                continue;
            }
            let at = self.report.position(attribute.span.start);
            element.props.push(Prop { name: name.to_string(), value, at });
        }
        if kind.named && !named {
            let message = format!("<{}> needs a name", kind.tag);
            self.report.error(Code::Name, opening.name.span(), message);
        }

        for child in &node.children {
            match child {
                JSXChild::Element(child) => {
                    if let Some(child) = self.element(child, Some(kind)) {
                        element.children.push(child);
                    }
                }
                JSXChild::Text(text) if text.value.as_str().trim().is_empty() => {}
                // `{/* a comment */}` is the only way to comment between elements.
                JSXChild::ExpressionContainer(container)
                    if matches!(container.expression, JSXExpression::EmptyExpression(_)) => {}
                other => self.report.error(
                    Code::Nesting,
                    other.span(),
                    "only elements are valid children; there is no text, expression, fragment or loop syntax",
                ),
            }
        }
        Some(element)
    }

    /// Stores an element's `name` after checking it (spec 2.1).
    ///
    /// # Arguments
    /// * `element` - the element being built.
    /// * `value` - the value of its `name` prop.
    /// * `span` - where the prop is.
    fn name(&mut self, element: &mut Element, value: Value, span: Span) {
        let Value::Text(name) = value else {
            self.report.error(Code::Name, span, "a name is a string literal");
            return;
        };
        if !is_identifier(&name) {
            let message = format!("'{name}' is not a valid name; use letters, digits and '_', not starting with a digit");
            self.report.error(Code::Name, span, message);
            return;
        }
        let at = self.report.position(span.start);
        if let Some(first) = self.names.get(&name) {
            let message = format!("the name '{name}' is already used at line {}", first.line);
            self.report.error(Code::Duplicate, span, message);
        } else {
            self.names.insert(name.clone(), at);
        }
        element.name = name;
    }

    /// Reads a prop value under the parse rules (spec 2.2).
    ///
    /// # Arguments
    /// * `value` - the JSX attribute value, absent for a bare prop.
    /// * `span` - the whole attribute, for errors without a better position.
    ///
    /// # Returns
    /// The value, or `None` after reporting why it is rejected.
    fn value(&mut self, value: Option<&JSXAttributeValue<'_>>, span: Span) -> Option<Value> {
        match value {
            None => Some(Value::Bool(true)),
            Some(JSXAttributeValue::StringLiteral(literal)) => match literal.value.as_str() {
                Some(text) => Some(Value::Text(text.to_string())),
                None => self.reject(literal.span, "a string with an unpaired surrogate"),
            },
            Some(JSXAttributeValue::ExpressionContainer(container)) => match &container.expression {
                JSXExpression::EmptyExpression(empty) => self.reject(empty.span, "an empty expression"),
                other => self.expression(other.to_expression(), true),
            },
            Some(JSXAttributeValue::Element(_) | JSXAttributeValue::Fragment(_)) => {
                self.reject(span, "an element")
            }
        }
    }

    /// Reads a literal or a handler reference from an expression.
    ///
    /// # Arguments
    /// * `expression` - the expression inside `{}`.
    /// * `top` - `true` for a prop's own value; `false` inside an array, where
    ///   only scalar literals are valid.
    ///
    /// # Returns
    /// The value, or `None` after reporting why it is rejected.
    fn expression(&mut self, expression: &Expression<'_>, top: bool) -> Option<Value> {
        let expression = expression.without_parentheses();
        match expression {
            Expression::StringLiteral(literal) => match literal.value.as_str() {
                Some(text) => Some(Value::Text(text.to_string())),
                None => self.reject(literal.span, "a string with an unpaired surrogate"),
            },
            Expression::NumericLiteral(literal) => Some(Value::Number(literal.value)),
            Expression::BooleanLiteral(literal) => Some(Value::Bool(literal.value)),
            Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
                let cooked = template.quasis.first().and_then(|quasi| quasi.value.cooked).and_then(|text| text.as_str());
                match cooked {
                    Some(text) => Some(Value::Text(text.to_string())),
                    None => self.reject(template.span, "a template with an invalid escape"),
                }
            }
            Expression::TemplateLiteral(template) => self.reject(template.span, "a template with substitutions"),
            Expression::UnaryExpression(unary) => {
                let sign = match unary.operator {
                    UnaryOperator::UnaryNegation => -1.0,
                    UnaryOperator::UnaryPlus => 1.0,
                    _ => return self.reject(unary.span, "a computed value"),
                };
                match unary.argument.without_parentheses() {
                    Expression::NumericLiteral(literal) => Some(Value::Number(sign * literal.value)),
                    _ => self.reject(unary.span, "a computed value"),
                }
            }
            Expression::ArrayExpression(array) if top => {
                let mut items = Vec::with_capacity(array.elements.len());
                let mut valid = true;
                for element in &array.elements {
                    let item = match element {
                        ArrayExpressionElement::SpreadElement(spread) => self.reject(spread.span, "a spread"),
                        ArrayExpressionElement::Elision(elision) => self.reject(elision.span, "a hole in an array"),
                        other => self.expression(other.to_expression(), false),
                    };
                    match item {
                        Some(item) => items.push(item),
                        None => valid = false,
                    }
                }
                valid.then_some(Value::List(items))
            }
            Expression::ArrayExpression(array) => self.reject(array.span, "a nested array"),
            Expression::StaticMemberExpression(member) => {
                let on_form = matches!(&member.object, Expression::Identifier(object) if object.name == "form");
                if on_form && !member.optional && top {
                    Some(Value::Handler(member.property.name.as_str().to_string()))
                } else if on_form && !member.optional {
                    self.reject(member.span, "a handler inside an array")
                } else {
                    self.reject(member.span, "a member access other than form.<member>")
                }
            }
            Expression::CallExpression(call) => self.reject(call.span, "a call"),
            Expression::ConditionalExpression(conditional) => self.reject(conditional.span, "a conditional"),
            Expression::ObjectExpression(object) => self.reject(object.span, "an object; props are flat scalars"),
            Expression::ArrowFunctionExpression(function) => {
                self.reject(function.span, "a function; name a handler as form.<member>")
            }
            Expression::FunctionExpression(function) => {
                self.reject(function.span, "a function; name a handler as form.<member>")
            }
            Expression::Identifier(identifier) => self.reject(identifier.span, "a variable"),
            other => self.reject(other.span(), "a computed value"),
        }
    }

    /// Reports a rejected prop value.
    ///
    /// # Arguments
    /// * `span` - where the value is.
    /// * `what` - what was found, completing "… is not a valid prop value".
    ///
    /// # Returns
    /// Always `None`, so callers can return it directly.
    fn reject(&mut self, span: Span, what: &str) -> Option<Value> {
        let message = format!("{what} is not a valid prop value; a value is a literal or form.<member>");
        self.report.error(Code::Value, span, message);
        None
    }
}

/// Decides whether a prop name is an event: `on` followed by an upper-case letter.
///
/// # Arguments
/// * `name` - the prop name.
///
/// # Returns
/// `true` for an event prop.
pub(crate) fn is_event(name: &str) -> bool {
    name.strip_prefix("on").and_then(|rest| rest.chars().next()).is_some_and(|first| first.is_ascii_uppercase())
}

/// Decides whether a name can be a control name: an ASCII identifier, because the
/// generators emit it as a property key, a DOM id and a wire-format key.
///
/// # Arguments
/// * `name` - the candidate.
///
/// # Returns
/// `true` when it is valid.
fn is_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    characters.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}
