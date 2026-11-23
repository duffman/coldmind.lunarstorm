//! Finds the native API methods a project's code references (spec 2.6: "The
//! native API permissions are derived from what the form classes reference").
//!
//! Decisions:
//! - Native calls go through `@desktop/api` (spec 2.5), whose `native` export is
//!   `native.<namespace>.<method>(…)`. A reference is that member chain on a
//!   binding imported from `@desktop/api`; nothing is executed or resolved.
//! - Every `.ts` file under `src/` is scanned, not only the form classes: models
//!   are "application code used by form classes" (spec 2.7) and run with them.
//!   Tests and view resources are not part of the backend and are skipped.
//! - A file that does not parse contributes nothing here; `tsc` reports it.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{Expression, ImportDeclarationSpecifier, StaticMemberExpression, Statement};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_span::SourceType;

/// The module native calls go through.
const API_MODULE: &str = "@desktop/api";

/// The export of [`API_MODULE`] that holds the native namespaces.
const API_EXPORT: &str = "native";

/// Collects the native methods referenced under `root/src`.
///
/// # Arguments
/// * `root` - the project directory.
///
/// # Returns
/// The methods as `namespace.method`, sorted and without duplicates.
pub(crate) fn collect(root: &Path) -> Vec<String> {
    let mut found = BTreeSet::new();
    scan(&root.join("src"), &mut found);
    found.into_iter().collect()
}

/// Scans a directory, recursively.
///
/// # Arguments
/// * `directory` - where to look.
/// * `found` - receives the methods.
fn scan(directory: &Path, found: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name != "node_modules" {
                scan(&path, found);
            }
        } else if name.ends_with(".ts") && !name.ends_with(".test.ts") && !name.ends_with(".d.ts") {
            if let Ok(source) = fs::read_to_string(&path) {
                references(&source, found);
            }
        }
    }
}

/// Finds the native methods one source file references.
///
/// # Arguments
/// * `source` - the file's text.
/// * `found` - receives the methods.
pub(crate) fn references(source: &str, found: &mut BTreeSet<String>) {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    let mut binding = None;
    for statement in &parsed.program.body {
        let Statement::ImportDeclaration(import) = statement else { continue };
        if import.source.value.as_str() != Some(API_MODULE) {
            continue;
        }
        for specifier in import.specifiers.iter().flatten() {
            if let ImportDeclarationSpecifier::ImportSpecifier(specifier) = specifier {
                if specifier.imported.name() == API_EXPORT {
                    binding = Some(specifier.local.name.as_str().to_string());
                }
            }
        }
    }
    let Some(binding) = binding else { return };
    let mut finder = Finder { binding: &binding, found };
    finder.visit_program(&parsed.program);
}

/// Visits member expressions looking for `<binding>.<namespace>.<method>`.
struct Finder<'f> {
    /// The local name `native` was imported as.
    binding: &'f str,
    /// Receives the methods.
    found: &'f mut BTreeSet<String>,
}

impl<'a> Visit<'a> for Finder<'_> {
    /// Records the chain when this member expression is `<binding>.<ns>.<method>`.
    ///
    /// # Arguments
    /// * `member` - the member expression.
    fn visit_static_member_expression(&mut self, member: &StaticMemberExpression<'a>) {
        if let Expression::StaticMemberExpression(namespace) = &member.object {
            if matches!(&namespace.object, Expression::Identifier(object) if object.name == self.binding) {
                self.found.insert(format!("{}.{}", namespace.property.name, member.property.name));
            }
        }
        walk::walk_static_member_expression(self, member);
    }
}
