//! Bun loader selection and scoped parsing shared by AST transformation stages.
//!
//! Arena-owned AST data is exposed only during the callback: no parsed node or
//! scope reference may outlive the per-file allocator. Loader selection preserves
//! TypeScript/JSX syntax; this layer is not a transpiler.

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_span::SourceType;

/// Syntax supplied to the shared transform engine. This selects parsing only:
/// transforms preserve TypeScript/JSX and do not transpile or execute source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loader {
    Js,
    Jsx,
    Ts,
    Tsx,
    /// Text-only replacements; never parse with oxc.
    Other,
}

/// Source type for a loader; `None` for loaders that are not JS/TS.
pub fn source_type(loader: Loader) -> Option<SourceType> {
    match loader {
        Loader::Js | Loader::Jsx => Some(SourceType::jsx()),
        Loader::Ts => Some(SourceType::ts()),
        Loader::Tsx => Some(SourceType::tsx()),
        Loader::Other => None,
    }
}

/// Parses `source` and builds scopes, then calls `visit`. Returns `None` for non-JS loaders and
/// on syntax errors: the file passes through unchanged and Bun reports the error itself.
pub fn with_program<R>(
    source: &str,
    loader: Loader,
    visit: impl FnOnce(&Program<'_>, &Scoping) -> R,
) -> Option<R> {
    let source_type = source_type(loader)?;
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.fatal_error || !parsed.diagnostics.is_empty() {
        return None;
    }
    let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
    Some(visit(&parsed.program, semantic.scoping()))
}
