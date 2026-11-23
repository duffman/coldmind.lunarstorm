//! Checks an extracted element tree against the element table: every prop is
//! one the element takes, has the right type, and every reference names an
//! element of the right kind (spec 2.3, 2.4, 2.6).
//!
//! Decisions:
//! - These are compile errors like the parse rules: a misspelled prop or an
//!   enum value the native host does not know must not reach a generator, where
//!   it would be dropped silently.
//! - References to forms (`owner`, `mainForm`) are resolved by the caller, which
//!   knows every form of the project; everything else resolves within the
//!   resource, because a name is unique within its resource only.

use std::collections::HashMap;
use std::path::Path;

use crate::model::{Code, Diagnostic, Element, Prop, Value};
use crate::schema::{self, Type};

/// Checks one resource's tree.
///
/// # Arguments
/// * `tree` - the root element.
/// * `forms` - the names of every form of the project, for `Ref("form")`.
/// * `file` - the resource, for the diagnostics.
/// * `diagnostics` - receives the errors.
pub(crate) fn tree(tree: &Element, forms: &[String], file: &Path, diagnostics: &mut Vec<Diagnostic>) {
    let mut names: HashMap<&str, &'static str> = HashMap::new();
    tree.walk(&mut |element| {
        if !element.name.is_empty() {
            names.entry(element.name.as_str()).or_insert(element.tag);
        }
    });
    tree.walk(&mut |element| {
        let Some(kind) = schema::lookup(element.tag) else { return };
        for prop in &element.props {
            let mut error = |code: Code, message: String| {
                diagnostics.push(Diagnostic { code, file: file.to_path_buf(), at: prop.at, message });
            };
            if matches!(prop.value, Value::Handler(_)) {
                if !kind.events.contains(&prop.name.as_str()) {
                    error(Code::Event, format!("<{}> has no event '{}'", kind.tag, prop.name));
                }
                continue;
            }
            let Some(spec) = kind.prop(&prop.name) else {
                error(Code::Prop, format!("<{}> has no prop '{}'", kind.tag, prop.name));
                continue;
            };
            if !spec.declared {
                let message = format!("'{}' is data the form class sets at runtime, not markup", prop.name);
                error(Code::Prop, message);
                continue;
            }
            if let Err(expected) = typed(prop, spec.ty) {
                error(Code::Type, format!("'{}' takes {expected}", prop.name));
                continue;
            }
            if let (Type::Ref(target), Value::Text(name)) = (spec.ty, &prop.value) {
                let found = if target == "form" {
                    forms.iter().any(|form| form == name).then_some("form")
                } else {
                    names.get(name.as_str()).copied()
                };
                match found {
                    Some(tag) if tag == target => {}
                    Some(tag) => error(
                        Code::Reference,
                        format!("'{}' names '{name}', which is a <{tag}>, not a <{target}>", prop.name),
                    ),
                    None => error(Code::Reference, format!("'{}' names '{name}', which is not a <{target}>", prop.name)),
                }
            }
        }
    });
}

/// Checks a prop value against its type.
///
/// # Arguments
/// * `prop` - the prop.
/// * `ty` - the type the element table gives it.
///
/// # Returns
/// `Ok` when it fits.
///
/// # Errors
/// What the prop takes, completing the message "'x' takes …".
fn typed(prop: &Prop, ty: Type) -> Result<(), String> {
    let all_text = |items: &[Value]| items.iter().all(|item| matches!(item, Value::Text(_)));
    let one_of = |choices: &[&str]| format!("one of {}", choices.join(", "));
    match (ty, &prop.value) {
        (Type::Text | Type::Ref(_), Value::Text(_)) => Ok(()),
        (Type::Text | Type::Ref(_), _) => Err("a string".to_string()),
        (Type::Number, Value::Number(number)) if number.is_finite() => Ok(()),
        (Type::Number, _) => Err("a number".to_string()),
        (Type::Bool, Value::Bool(_)) => Ok(()),
        (Type::Bool, _) => Err("true or false".to_string()),
        (Type::Texts, Value::List(items)) if all_text(items) => Ok(()),
        (Type::Texts, _) => Err("an array of strings".to_string()),
        (Type::Choice(choices), Value::Text(text)) if choices.contains(&text.as_str()) => Ok(()),
        (Type::Choice(choices), _) => Err(one_of(choices)),
        (Type::Choices(choices), Value::List(items))
            if items.iter().all(|item| matches!(item, Value::Text(text) if choices.contains(&text.as_str()))) =>
        {
            Ok(())
        }
        (Type::Choices(choices), _) => Err(format!("an array of {}", choices.join(", "))),
        (Type::Color, Value::Text(text)) if is_color(text) => Ok(()),
        (Type::Color, _) => Err("#rrggbb or #rrggbbaa".to_string()),
        (Type::Rows | Type::Nodes, _) => Err("runtime data".to_string()),
    }
}

/// Decides whether a string is `#rrggbb` or `#rrggbbaa`.
///
/// # Arguments
/// * `text` - the candidate.
///
/// # Returns
/// `true` when it is a colour.
fn is_color(text: &str) -> bool {
    text.strip_prefix('#')
        .is_some_and(|hex| matches!(hex.len(), 6 | 8) && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}
