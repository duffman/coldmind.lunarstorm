//! Reads the members of a form class, for the handler check (spec 4a).
//!
//! Decisions:
//! - The class is found by name: the form's `name` is the name of an exported
//!   class in the paired `.ts` file (spec 2.5).
//! - Only members declared on the class itself count. Resolving inherited members
//!   would mean following imports, which extraction never does; a handler is the
//!   form's own code, so it is declared on the form's own class.
//! - `this.find<T>('Name')` is resolved here, against the form's view resource
//!   (spec 4, "typed and resolved at compile time"): the name is a control of the
//!   form and `T`, when written as a plain type name, is that control's class.
//!   Only a string literal can be resolved; any other argument is an error,
//!   because a name computed at runtime cannot be checked.
//! - A handler is public: the view resource reads it as `form.<member>` and the
//!   generated dispatch calls it from outside the class.

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, CallExpression, Class, ClassElement, Declaration, ExportDefaultDeclarationKind, Expression,
    MethodDefinitionKind, PropertyKey, Statement, TSAccessibility, TSType, TSTypeName,
};
use oxc_ast_visit::{walk, Visit};
use oxc_span::{SourceType, Span};

use crate::model::{Code, Element};
use crate::resource::{parse, Report};
use crate::schema::class_name;

/// Checks every `this.find<T>('Name')` of a form class against its view.
struct Finds<'r, 'd> {
    /// The form's view resource.
    tree: &'r Element,
    /// Receives [`Code::Find`] errors.
    report: &'r mut Report<'d>,
}

impl<'a> Visit<'a> for Finds<'_, '_> {
    /// Checks one call when it is `this.find(...)`, then descends.
    ///
    /// # Arguments
    /// * `call` - a call expression in the class.
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        walk::walk_call_expression(self, call);
        let Expression::StaticMemberExpression(member) = call.callee.without_parentheses() else { return };
        if member.property.name != "find" || !matches!(member.object.without_parentheses(), Expression::ThisExpression(_)) {
            return;
        }
        let Some(Argument::StringLiteral(literal)) = call.arguments.first() else {
            self.report.error(Code::Find, call.span, "find takes the control's name as a string literal");
            return;
        };
        // A literal holding a lone surrogate is not text and names no control.
        let Some(name) = literal.value.as_str() else {
            self.report.error(Code::Find, literal.span, "find takes the control's name as a string literal");
            return;
        };
        let mut tag = None;
        self.tree.walk(&mut |element| {
            if element.name == name {
                tag = Some(element.tag);
            }
        });
        let form = &self.tree.name;
        let Some(tag) = tag else {
            self.report.error(Code::Find, literal.span, format!("the form '{form}' has no control named '{name}'"));
            return;
        };
        // `find<T>`: a plain type name is checked; anything else (a union, `any`)
        // is the developer's own widening and is left to tsc.
        let written = call.type_arguments.as_ref().and_then(|arguments| arguments.params.first());
        let Some(TSType::TSTypeReference(reference)) = written else { return };
        let TSTypeName::IdentifierReference(identifier) = &reference.type_name else { return };
        match class_name(tag) {
            Some(expected) if identifier.name != expected => {
                let message = format!("'{name}' is a <{tag}>, so its type is {expected}, not {}", identifier.name);
                self.report.error(Code::Find, reference.span, message);
            }
            Some(_) => {}
            None => self.report.error(Code::Find, literal.span, format!("'{name}' is the <{tag}> itself, not a control")),
        }
    }
}

/// One member a view resource may name as a handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Member {
    /// The member's name.
    pub(crate) name: String,
    /// Whether it is public; `private` and `protected` members are not handlers.
    pub(crate) public: bool,
}

/// Reads the instance members of the form class in a `.ts` file.
///
/// # Arguments
/// * `source` - the file's text.
/// * `tree` - the form's view resource; its name is the class's name.
/// * `report` - receives syntax errors, a [`Code::Class`] error when the class
///   is missing, not exported or extends nothing, and [`Code::Find`] errors.
///
/// # Returns
/// The members, or `None` when the class cannot be used.
pub(crate) fn members(source: &str, tree: &Element, report: &mut Report<'_>) -> Option<Vec<Member>> {
    let form = tree.name.as_str();
    let allocator = Allocator::default();
    let program = parse(&allocator, source, SourceType::ts(), report)?;
    let mut found: Option<(&Class<'_>, bool)> = None;
    for statement in &program.body {
        let (class, exported): (&Class<'_>, bool) = match statement {
            Statement::ClassDeclaration(class) => (class, false),
            Statement::ExportDeclaration(export) => match &export.declaration {
                Declaration::ClassDeclaration(class) => (class, true),
                _ => continue,
            },
            Statement::ExportDefaultDeclaration(export) => match &export.declaration {
                ExportDefaultDeclarationKind::ClassDeclaration(class) => (class, true),
                _ => continue,
            },
            _ => continue,
        };
        if class.id.as_ref().is_some_and(|id| id.name == form) {
            found = Some((class, exported));
            break;
        }
    }
    let Some((class, exported)) = found else {
        let message = format!("no class named '{form}'; a form class has its form's name");
        report.error(Code::Class, Span::new(0, 0), message);
        return None;
    };
    if !exported {
        report.error(Code::Class, class.span, format!("the form class '{form}' is not exported"));
        return None;
    }
    if class.heritage.is_none() {
        report.error(Code::Class, class.span, format!("the form class '{form}' does not extend Form"));
        return None;
    }

    Finds { tree, report }.visit_class(class);

    let mut members = Vec::new();
    for element in &class.body.body {
        let (key, is_static, accessibility) = match element {
            ClassElement::MethodDefinition(method) if method.kind == MethodDefinitionKind::Method => {
                (&method.key, method.r#static, method.accessibility)
            }
            ClassElement::PropertyDefinition(property) => (&property.key, property.r#static, property.accessibility),
            _ => continue,
        };
        // `#private` members and computed keys cannot be written as `form.<member>`.
        let PropertyKey::StaticIdentifier(identifier) = key else { continue };
        if is_static {
            continue;
        }
        let public = !matches!(accessibility, Some(TSAccessibility::Private | TSAccessibility::Protected));
        members.push(Member { name: identifier.name.as_str().to_string(), public });
    }
    Some(members)
}
