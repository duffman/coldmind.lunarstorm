//! Define stage: scope-aware global constants (`__MODE__`, `process.env.NODE_ENV`,
//! `import.meta.env.MODE`). Only real references to the global are replaced: strings, comments,
//! property names, and locally declared bindings with the same name are left alone. An ambient
//! `declare const __MODE__: string` still counts as the global.

use oxc_allocator::Allocator;
use oxc_ast::ast::{Expression, IdentifierReference, ObjectProperty, Program};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::Scoping;
use oxc_span::{GetSpan, SourceType};
use oxc_syntax::symbol::SymbolFlags;

use crate::source_edits::Edits;

#[derive(Debug, PartialEq)]
enum Root {
    Global(String),
    ImportMeta,
}

struct Define {
    root: Root,
    path: Vec<String>,
    value: String,
}

/// Validated constants for one plugin instance.
pub struct DefineConfig {
    defines: Vec<Define>,
    /// Cheap pre-check: a file containing none of these cannot match.
    needles: Vec<String>,
}

impl DefineConfig {
    /// Validates `(key, value)` pairs: keys are dotted identifier paths, values JS expressions.
    pub fn new(
        pairs: impl IntoIterator<Item = (String, String)>,
    ) -> std::result::Result<Self, String> {
        let mut defines = Vec::new();
        let mut needles: Vec<String> = Vec::new();
        for (key, value) in pairs {
            let (root, path) = parse_key(&key)?;
            let value = prepare_value(&key, &value)?;
            let needle = match &root {
                Root::Global(name) => name.clone(),
                Root::ImportMeta => "import.meta".to_string(),
            };
            if !needles.contains(&needle) {
                needles.push(needle);
            }
            defines.push(Define { root, path, value });
        }
        Ok(Self { defines, needles })
    }

    /// False when `source` cannot contain a match, so it need not be parsed for this stage.
    pub fn may_match(&self, source: &str) -> bool {
        self.needles
            .iter()
            .any(|needle| source.contains(needle.as_str()))
    }

    /// Edits for every matching reference in `program`.
    pub fn edits(&self, program: &Program<'_>, scoping: &Scoping) -> Edits {
        let mut visitor = DefineVisitor {
            defines: &self.defines,
            scoping,
            edits: Edits::default(),
        };
        visitor.visit_program(program);
        visitor.edits
    }
}

fn is_identifier(segment: &str) -> bool {
    let mut chars = segment.chars();
    matches!(chars.next(), Some(c) if c == '_' || c == '$' || c.is_alphabetic())
        && chars.all(|c| c == '_' || c == '$' || c.is_alphanumeric())
}

fn parse_key(key: &str) -> std::result::Result<(Root, Vec<String>), String> {
    let segments: Vec<&str> = key.split('.').collect();
    if segments.iter().any(|segment| !is_identifier(segment)) {
        return Err(format!(
            "define key `{key}` must be a dotted identifier path"
        ));
    }
    if segments.len() >= 2 && segments[0] == "import" && segments[1] == "meta" {
        return Ok((
            Root::ImportMeta,
            segments[2..].iter().map(|s| s.to_string()).collect(),
        ));
    }
    Ok((
        Root::Global(segments[0].to_string()),
        segments[1..].iter().map(|s| s.to_string()).collect(),
    ))
}

/// Validates `value` as an expression; parenthesizes anything but literals and member paths
/// so it keeps its meaning wherever it is spliced in.
fn prepare_value(key: &str, value: &str) -> std::result::Result<String, String> {
    let allocator = Allocator::default();
    let expression = Parser::new(&allocator, value, SourceType::mjs())
        .parse_expression()
        .map_err(|_| format!("define value for `{key}` is not a JS expression: {value}"))?;
    let simple = matches!(
        expression,
        Expression::StringLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::Identifier(_)
            | Expression::StaticMemberExpression(_)
    );
    Ok(if simple {
        value.trim().to_string()
    } else {
        format!("({})", value.trim())
    })
}

struct DefineVisitor<'s> {
    defines: &'s [Define],
    scoping: &'s Scoping,
    edits: Edits,
}

impl<'s> DefineVisitor<'s> {
    /// True when `ident` refers to the global of that name (unbound, or bound only by `declare`).
    fn is_global(&self, ident: &IdentifierReference) -> bool {
        let Some(reference_id) = ident.reference_id.get() else {
            return true;
        };
        match self.scoping.get_reference(reference_id).symbol_id() {
            None => true,
            Some(symbol) => self
                .scoping
                .symbol_flags(symbol)
                .contains(SymbolFlags::Ambient),
        }
    }

    /// The define whose path equals `expression`, if any.
    fn lookup(&self, expression: &Expression) -> Option<&'s Define> {
        let mut path: Vec<&str> = Vec::new();
        let mut current = expression;
        let root = loop {
            match current {
                Expression::StaticMemberExpression(member) if !member.optional => {
                    path.push(member.property.name.as_str());
                    current = &member.object;
                }
                Expression::Identifier(ident) if self.is_global(ident) => {
                    break Root::Global(ident.name.to_string())
                }
                Expression::ImportMeta(_) => break Root::ImportMeta,
                _ => return None,
            }
        };
        path.reverse();
        self.defines.iter().find(|define| {
            define.root == root
                && define
                    .path
                    .iter()
                    .map(String::as_str)
                    .eq(path.iter().copied())
        })
    }
}

impl<'a> Visit<'a> for DefineVisitor<'_> {
    fn visit_expression(&mut self, expression: &Expression<'a>) {
        if let Some(define) = self.lookup(expression) {
            let span = expression.span();
            self.edits
                .replace(span.start, span.end, define.value.as_str());
            return;
        }
        walk::walk_expression(self, expression);
    }

    fn visit_object_property(&mut self, property: &ObjectProperty<'a>) {
        if property.shorthand {
            if let (Some(define), Expression::Identifier(ident)) =
                (self.lookup(&property.value), &property.value)
            {
                let span = property.value.span();
                self.edits.replace(
                    span.start,
                    span.end,
                    format!("{}: {}", ident.name, define.value),
                );
                return;
            }
        }
        walk::walk_object_property(self, property);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bun_hook::with_program;
    use crate::bun_hook::Loader;

    fn define(pairs: &[(&str, &str)], source: &str) -> String {
        let config =
            DefineConfig::new(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string()))).unwrap();
        assert!(config.may_match(source));
        with_program(source, Loader::Ts, |program, scoping| {
            config.edits(program, scoping)
        })
        .unwrap_or_default()
        .apply(source)
    }

    #[test]
    fn replaces_global_references_only() {
        let source = r#"const a = __MODE__; const s = "__MODE__"; // __MODE__
const o = { __MODE__: 1 }; o.__MODE__; function f(__MODE__: string) { return __MODE__; }"#;
        let expected = r#"const a = "dev"; const s = "__MODE__"; // __MODE__
const o = { __MODE__: 1 }; o.__MODE__; function f(__MODE__: string) { return __MODE__; }"#;
        assert_eq!(define(&[("__MODE__", r#""dev""#)], source), expected);
    }

    #[test]
    fn treats_ambient_declarations_as_the_global() {
        let source = "declare const __MODE__: string;\nexport const m = __MODE__;";
        assert_eq!(
            define(&[("__MODE__", "1")], source),
            "declare const __MODE__: string;\nexport const m = 1;"
        );
    }

    #[test]
    fn replaces_member_paths_and_import_meta() {
        let source = "if (process.env.NODE_ENV === 'x') import.meta.env.MODE; process.env.OTHER;";
        let output = define(
            &[
                ("process.env.NODE_ENV", r#""production""#),
                ("import.meta.env.MODE", "1 + 1"),
            ],
            source,
        );
        assert_eq!(
            output,
            r#"if ("production" === 'x') (1 + 1); process.env.OTHER;"#
        );
    }

    #[test]
    fn expands_shorthand_properties() {
        assert_eq!(
            define(&[("__MODE__", "1")], "const o = { __MODE__ };"),
            "const o = { __MODE__: 1 };"
        );
    }

    #[test]
    fn leaves_syntax_errors_to_bun() {
        assert_eq!(
            define(&[("__MODE__", "1")], "const = __MODE__"),
            "const = __MODE__"
        );
    }

    #[test]
    fn skips_sources_without_a_root_name() {
        let config = DefineConfig::new([("process.env.X".to_string(), "1".to_string())]).unwrap();
        assert!(!config.may_match("const a = 1;"));
    }

    #[test]
    fn rejects_invalid_keys_and_values() {
        assert!(parse_key("a..b").is_err());
        assert!(parse_key("a-b").is_err());
        assert!(prepare_value("k", "1 +").is_err());
    }
}
