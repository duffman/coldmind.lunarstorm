//! Inject stage: generates cminject's `$inject` from constructor parameter types.
//!
//! ```ts
//! @injectable() class Orders { constructor(private repo: Repo, log?: Logger) {} }
//! // becomes
//! @injectable() class Orders { static get $inject() { return [Repo, { token: Logger, optional: true }]; } constructor(...) {} }
//! ```
//!
//! A getter, not a field: it is read when the container resolves the class, so classes declared
//! later in the module or reached through circular imports are initialized by then.
//!
//! Selected classes: those with a decorator or a `static $injectable` (or every class in `all`
//! mode), with a constructor, without their own `$inject`. A parameter type must be a reference
//! to a value: a class in scope or a value import (`import { Repo }`, `ns.Repo`). Primitives,
//! interfaces, type aliases, type-only imports, unresolved names, and rest parameters skip the
//! class; for selected marked classes a warning names the parameter. A decorated parameter
//! (legacy `@Inject(token)`) contributes `undefined` so the decorator's token applies.
//! Syntax cannot tell an interface imported without `type` from a class: such an import must
//! use `import type` (Bun then reports a missing export instead of a silent wrong token).

use oxc_ast::ast::{
    BindingPattern, Class, ClassElement, FormalParameter, MethodDefinitionKind, Program,
    PropertyKey, TSType, TSTypeName,
};
use oxc_ast_visit::{walk, Visit};
use oxc_semantic::Scoping;
use oxc_syntax::symbol::SymbolFlags;

use crate::source_edits::Edits;

/// Inject stage settings for one plugin instance.
pub struct InjectConfig {
    /// Also select classes without a decorator or `static $injectable`; those are skipped silently
    /// when a parameter cannot be resolved.
    pub all: bool,
}

impl InjectConfig {
    /// False when `source` has no constructor, so it need not be parsed for this stage.
    pub fn may_match(&self, source: &str) -> bool {
        source.contains("constructor")
    }

    /// Edits adding `$inject` getters, plus a warning per selected marked class left unwired.
    pub fn edits(&self, program: &Program<'_>, scoping: &Scoping) -> (Edits, Vec<String>) {
        let mut visitor = InjectVisitor {
            all: self.all,
            scoping,
            edits: Edits::default(),
            warnings: Vec::new(),
        };
        visitor.visit_program(program);
        (visitor.edits, visitor.warnings)
    }
}

fn key_is(key: &PropertyKey, name: &str) -> bool {
    matches!(key, PropertyKey::StaticIdentifier(id) if id.name == name)
}

/// True when the class body declares a static member `name` (field, accessor or method).
fn has_static(class: &Class, name: &str) -> bool {
    class.body.body.iter().any(|element| match element {
        ClassElement::PropertyDefinition(p) => p.r#static && key_is(&p.key, name),
        ClassElement::MethodDefinition(m) => m.r#static && key_is(&m.key, name),
        ClassElement::AccessorProperty(a) => a.r#static && key_is(&a.key, name),
        _ => false,
    })
}

fn parameter_name(parameter: &FormalParameter) -> String {
    match &parameter.pattern {
        BindingPattern::BindingIdentifier(id) => format!("`{}`", id.name),
        _ => "(destructured)".to_string(),
    }
}

struct InjectVisitor<'s> {
    all: bool,
    scoping: &'s Scoping,
    edits: Edits,
    warnings: Vec<String>,
}

impl InjectVisitor<'_> {
    /// Value expression for a type name, or why it has none.
    fn token(&self, name: &TSTypeName) -> std::result::Result<String, &'static str> {
        match name {
            TSTypeName::IdentifierReference(ident) => {
                let symbol = ident
                    .reference_id
                    .get()
                    .and_then(|id| self.scoping.get_reference(id).symbol_id())
                    .ok_or("its type is not declared or imported in this module")?;
                let flags = self.scoping.symbol_flags(symbol);
                if flags.contains(SymbolFlags::TypeImport) {
                    Err("its type is a type-only import")
                } else if flags.contains(SymbolFlags::Ambient) {
                    Err("its type is an ambient declaration")
                } else if flags.intersects(SymbolFlags::Class | SymbolFlags::Import) {
                    Ok(ident.name.to_string())
                } else {
                    Err("its type is not a class (interface, type alias, enum or variable)")
                }
            }
            TSTypeName::QualifiedName(qualified) => {
                let left = match &qualified.left {
                    TSTypeName::IdentifierReference(ident) => {
                        let symbol = ident
                            .reference_id
                            .get()
                            .and_then(|id| self.scoping.get_reference(id).symbol_id())
                            .ok_or("its namespace is not declared or imported in this module")?;
                        let flags = self.scoping.symbol_flags(symbol);
                        if flags.contains(SymbolFlags::TypeImport)
                            || !flags.intersects(SymbolFlags::Import | SymbolFlags::ValueModule)
                        {
                            return Err("its namespace is not a value import");
                        }
                        ident.name.to_string()
                    }
                    nested => self.token(nested)?,
                };
                Ok(format!("{left}.{}", qualified.right.name))
            }
            TSTypeName::ThisExpression(_) => Err("its type is `this`"),
        }
    }

    /// `$inject` entry for one parameter.
    fn entry(&self, parameter: &FormalParameter) -> std::result::Result<String, &'static str> {
        if !parameter.decorators.is_empty() {
            return Ok("undefined".to_string());
        }
        let annotation = parameter
            .type_annotation
            .as_ref()
            .ok_or("it has no type annotation")?;
        let TSType::TSTypeReference(reference) = &annotation.type_annotation else {
            return Err("its type is not a class reference");
        };
        let token = self.token(&reference.type_name)?;
        Ok(if parameter.optional || parameter.initializer.is_some() {
            format!("{{ token: {token}, optional: true }}")
        } else {
            token
        })
    }

    fn check(&mut self, class: &Class) {
        let marked = !class.decorators.is_empty() || has_static(class, "$injectable");
        if !(marked || self.all) || has_static(class, "$inject") {
            return;
        }
        let Some(constructor) = class.body.body.iter().find_map(|element| match element {
            ClassElement::MethodDefinition(m) if m.kind == MethodDefinitionKind::Constructor => {
                Some(m)
            }
            _ => None,
        }) else {
            return;
        };
        let parameters = &constructor.value.params;
        if parameters.items.is_empty() && parameters.rest.is_none() {
            return;
        }
        let class_name = class
            .id
            .as_ref()
            .map_or("anonymous class", |id| id.name.as_str());
        let skip = |reason: String, visitor: &mut Self| {
            if marked {
                visitor.warnings.push(format!(
                    "devermind inject: {class_name} not wired: {reason}; declare `static $inject`"
                ));
            }
        };
        if parameters.rest.is_some() {
            return skip("its constructor has a rest parameter".to_string(), self);
        }
        let mut entries = Vec::with_capacity(parameters.items.len());
        for parameter in &parameters.items {
            match self.entry(parameter) {
                Ok(entry) => entries.push(entry),
                Err(reason) => {
                    return skip(
                        format!(
                            "constructor parameter {} cannot be resolved: {reason}",
                            parameter_name(parameter)
                        ),
                        self,
                    )
                }
            }
        }
        self.edits.insert(
            class.body.span.start + 1,
            format!(
                " static get $inject() {{ return [{}]; }}",
                entries.join(", ")
            ),
        );
    }
}

impl<'a> Visit<'a> for InjectVisitor<'_> {
    fn visit_class(&mut self, class: &Class<'a>) {
        self.check(class);
        walk::walk_class(self, class);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bun_hook::with_program;
    use crate::bun_hook::Loader;

    fn inject(all: bool, source: &str) -> (String, Vec<String>) {
        let config = InjectConfig { all };
        let (edits, warnings) = with_program(source, Loader::Ts, |program, scoping| {
            config.edits(program, scoping)
        })
        .unwrap();
        (edits.apply(source), warnings)
    }

    const PRELUDE: &str = "import { Repo } from './repo';\nimport * as log from './log';\nimport type { Clock } from './clock';\ninterface Cfg { a: 1 }\nclass Local {}\n";

    #[test]
    fn wires_marked_classes() {
        let source = format!("{PRELUDE}@injectable() class A {{ constructor(private repo: Repo, l: log.Logger, x?: Local, y: Local = new Local()) {{}} }}");
        let (output, warnings) = inject(false, &source);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(output.contains(
      "class A { static get $inject() { return [Repo, log.Logger, { token: Local, optional: true }, { token: Local, optional: true }]; } constructor("
    ), "{output}");
    }

    #[test]
    fn wires_static_injectable_and_class_expressions() {
        let source = format!("{PRELUDE}export const B = class {{ static $injectable = {{}}; constructor(r: Repo) {{}} }};");
        let (output, _) = inject(false, &source);
        assert!(
            output.contains("class { static get $inject() { return [Repo]; } static $injectable"),
            "{output}"
        );
    }

    #[test]
    fn leaves_unmarked_and_already_declared_classes() {
        let source = format!("{PRELUDE}class C {{ constructor(r: Repo) {{}} }}\n@d class D {{ static $inject = []; constructor(r: Repo) {{}} }}");
        assert_eq!(inject(false, &source).0, source);
    }

    #[test]
    fn all_mode_wires_unmarked_classes_and_skips_silently() {
        let source = format!("{PRELUDE}class C {{ constructor(r: Repo) {{}} }}\nclass E {{ constructor(n: number) {{}} }}");
        let (output, warnings) = inject(true, &source);
        assert!(
            output.contains("class C { static get $inject() { return [Repo]; }"),
            "{output}"
        );
        assert!(output.contains("class E { constructor"), "{output}");
        assert!(warnings.is_empty());
    }

    #[test]
    fn warns_on_unresolvable_parameters() {
        for (parameter, reason) in [
            ("c: Clock", "type-only import"),
            ("c: Cfg", "not a class"),
            ("c: string", "not a class reference"),
            ("c", "no type annotation"),
            ("c: Missing", "not declared or imported"),
        ] {
            let source = format!("{PRELUDE}@d class F {{ constructor({parameter}) {{}} }}");
            let (output, warnings) = inject(false, &source);
            assert_eq!(output, source);
            assert_eq!(warnings.len(), 1, "{parameter}");
            assert!(
                warnings[0].contains("F not wired") && warnings[0].contains(reason),
                "{}",
                warnings[0]
            );
        }
    }

    #[test]
    fn keeps_decorated_parameters_for_their_decorator() {
        let source =
            format!("{PRELUDE}@d class G {{ constructor(@Inject(T) t: unknown, r: Repo) {{}} }}");
        assert!(inject(false, &source)
            .0
            .contains("return [undefined, Repo];"));
    }

    #[test]
    fn nested_classes_are_wired_independently() {
        let source = format!("{PRELUDE}@d class H {{ constructor(r: Repo) {{ @d class I {{ constructor(l: Local) {{}} }} }} }}");
        let output = inject(false, &source).0;
        assert!(output.contains("class H { static get $inject() { return [Repo]; }"));
        assert!(output.contains("class I { static get $inject() { return [Local]; }"));
    }
}
