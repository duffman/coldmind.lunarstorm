//! Load-time transforms that turn non-ESM files into ES modules: TypeScript type
//! stripping, JSON modules and the CommonJS wrapper.
//!
//! Ported from jetbunny (`src/platform/package/transform.rs`); the wrapper's
//! internal names carry the `lunarstorm` prefix instead.
//!
//! Decisions:
//! - TypeScript is stripped, never type-checked: types removed, enums and
//!   namespaces lowered, nothing else changed.
//! - Every printed line is padded down to its original line, so stack traces and
//!   breakpoints name the `.ts` file's lines without a source map.
//! - A CommonJS file is two modules: a *body* (the file itself) that runs the code
//!   once when first required and exports only its loader, and a *facade*
//!   (`<file>#esm`) that an `import` reaches and that re-exports `module.exports`
//!   as `default` plus every export name found statically.
//! - Fix: the body was named `<file>#cjs`, and the engine names stack frames after
//!   the module, so traces of CommonJS code read `main.ts#cjs:8`. Decision: the
//!   body, which holds the user's code, carries the file's own name; the one-line
//!   facade carries the suffix.

use crate::Error;
use oxc_allocator::Allocator;
use oxc_ast::ast;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::SourceType;
use std::path::Path;

/// The import prefix of a `require()` request inside a CommonJS body: the loader
/// resolves what follows with `require` rules from the requiring file.
pub const REQUIRE: &str = "lunarstorm-require:";

/// The name suffix of a CommonJS file's facade module, the one an `import` reaches.
pub const FACADE: &str = "#esm";

/// # Returns
/// A failure naming the file and what went wrong.
fn failure(path: &str, what: &str, message: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("{path}: {what}: {message}"))
}

/// The ES module of a JSON file: its value as the default export.
///
/// # Errors
/// The text is not JSON.
pub fn json_module(path: &str, text: &str) -> Result<String, Error> {
    serde_json::from_str::<serde_json::Value>(text).map_err(|e| failure(path, "invalid JSON", e))?;
    let literal = serde_json::to_string(text).map_err(|e| failure(path, "invalid JSON", e))?;
    Ok(format!("export default JSON.parse({literal});\n"))
}

/// Whether a `.js`/`.ts` file with no declared `type` is an ES module: it has
/// `import`/`export` declarations, `import.meta` or top-level `await` (Node's
/// syntax detection).
///
/// # Arguments
/// * `path` - the file, for TypeScript detection by extension.
/// * `text` - its source.
///
/// # Returns
/// `true` for module syntax; `false` for a script, or text that does not parse
/// (the CommonJS path then reports the syntax error).
pub fn has_module_syntax(path: &str, text: &str) -> bool {
    let allocator = Allocator::default();
    let base = if path.ends_with(".ts") { SourceType::ts() } else { SourceType::mjs() };
    let parsed = Parser::new(&allocator, text, base.with_unambiguous(true)).parse();
    parsed.diagnostics.errors().next().is_none() && parsed.program.source_type.is_module()
}

/// JavaScript for a TypeScript file, with every line at its original line number.
///
/// # Arguments
/// * `path` - the file; `.cts` parses as CommonJS.
/// * `text` - its source.
///
/// # Errors
/// A syntax error, or a construct the transformer refuses.
pub fn strip_types(path: &str, text: &str) -> Result<String, Error> {
    strip_types_mapped(path, text).map(|(code, _)| code)
}

/// [`strip_types`], with a Source Map v3 from the output back to `text`, for the
/// debugger.
///
/// Decision: the map's one source is the file's name relative to the generated
/// source, which the debugger names `<file URI>.js`, so it resolves to the file.
///
/// # Arguments
/// * `path` - the file; `.cts` parses as CommonJS.
/// * `text` - its source.
///
/// # Returns
/// The JavaScript and its map (JSON); no map when the printer made none.
///
/// # Errors
/// A syntax error, or a construct the transformer refuses.
pub fn strip_types_mapped(path: &str, text: &str) -> Result<(String, Option<String>), Error> {
    let allocator = Allocator::default();
    let source_type = if path.ends_with(".cts") { SourceType::ts().with_commonjs(true) } else { SourceType::ts() };
    let parsed = Parser::new(&allocator, text, source_type).parse();
    if let Some(e) = parsed.diagnostics.errors().next() {
        return Err(failure(path, "TypeScript syntax error", e.message.as_ref()));
    }
    let mut program = parsed.program;
    let scoping = oxc_semantic::SemanticBuilder::new().with_enum_eval(true).build(&program).semantic.into_scoping();
    let options = oxc_transformer::TransformOptions::default();
    let transformed =
        oxc_transformer::Transformer::new(&allocator, Path::new(path), &options).build_with_scoping(scoping, &mut program);
    if let Some(e) = transformed.diagnostics.errors().next() {
        return Err(failure(path, "TypeScript transform failed", e.message.as_ref()));
    }
    // Fix: lowered constructs (an `enum` becomes several lines) pushed every later
    // line below its original one, because lines were only ever padded. Decision:
    // comments are not printed, so any printed line may be joined onto the one
    // before it, except a line that starts inside a template literal; the output
    // is padded where it is behind the original and joined where it is ahead.
    let printed = oxc_codegen::Codegen::new()
        .with_options(oxc_codegen::CodegenOptions {
            source_map_path: Some(path.into()),
            comments: oxc_codegen::CommentOptions::disabled(),
            ..oxc_codegen::CodegenOptions::default()
        })
        .build(&program);
    // The original line of each printed line's first mapped token.
    let mut original = Vec::<Option<(u32, u32)>>::new();
    if let Some(map) = &printed.map {
        for token in map.get_tokens() {
            let line = token.get_dst_line() as usize;
            if original.len() <= line {
                original.resize(line + 1, None);
            }
            let entry = (token.get_dst_col(), token.get_src_line());
            if original[line].is_none_or(|(col, _)| entry.0 < col) {
                original[line] = Some(entry);
            }
        }
    }
    let literal = literal_lines(&printed.code);
    let mut out = String::with_capacity(printed.code.len());
    // The output line the last pushed line is on.
    let mut current = 0u32;
    // Where each printed line landed: output line and column shift (UTF-16 units),
    // so the printer's map can be moved onto the output; `None` when dropped.
    let mut placed = Vec::<Option<(u32, i64)>>::new();
    for (index, line) in printed.code.split('\n').enumerate() {
        if index == 0 {
            let target = original.first().copied().flatten().map_or(0, |(_, line)| line);
            for _ in 0..target {
                out.push('\n');
            }
            current = target;
            out.push_str(line);
            placed.push(Some((current, 0)));
            continue;
        }
        if literal.contains(&index) {
            out.push('\n');
            current += 1;
            out.push_str(line);
            placed.push(Some((current, 0)));
            continue;
        }
        match original.get(index).copied().flatten() {
            Some((_, target)) if target > current => {
                for _ in current..target {
                    out.push('\n');
                }
                current = target;
                out.push_str(line);
                placed.push(Some((current, 0)));
            }
            // At or ahead of the original: joined onto the current line.
            _ if !line.trim().is_empty() => {
                out.push(' ');
                let start = out.rfind('\n').map_or(0, |at| at + 1);
                let column = out[start..].encode_utf16().count() as i64;
                let trimmed = line.trim_start();
                let indent = line[..line.len() - trimmed.len()].encode_utf16().count() as i64;
                out.push_str(trimmed);
                placed.push(Some((current, column - indent)));
            }
            _ => placed.push(None),
        }
    }
    out.push('\n');
    let map = printed.map.as_ref().map(|map| {
        let mut tokens: Vec<(u32, u32, u32, u32)> = map
            .get_tokens()
            .filter_map(|token| {
                let (line, shift) = placed.get(token.get_dst_line() as usize).copied().flatten()?;
                let column = u32::try_from(i64::from(token.get_dst_col()) + shift).ok()?;
                Some((line, column, token.get_src_line(), token.get_src_col()))
            })
            .collect();
        tokens.sort_unstable();
        tokens.dedup_by_key(|token| (token.0, token.1));
        let file = path.rsplit('/').next().unwrap_or(path).replace('%', "%25").replace('#', "%23").replace('?', "%3F");
        let mut builder = oxc_sourcemap::SourceMapBuilder::default();
        let source = builder.add_source_and_content(&file, text);
        for (line, column, src_line, src_col) in tokens {
            builder.add_token(line, column, src_line, src_col, Some(source), None);
        }
        builder.into_sourcemap().to_json_string()
    });
    Ok((out, map))
}

/// The printed lines that start inside a template literal: the newline before
/// them is part of the string, so they must stay on their own line.
///
/// # Arguments
/// * `code` - printed JavaScript.
///
/// # Returns
/// Zero-based line indices.
fn literal_lines(code: &str) -> std::collections::HashSet<usize> {
    /// Collects the spans of template literal text.
    struct Quasis(Vec<(u32, u32)>);
    impl<'a> Visit<'a> for Quasis {
        fn visit_template_element(&mut self, it: &ast::TemplateElement<'a>) {
            self.0.push((it.span.start, it.span.end));
        }
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, code, SourceType::mjs().with_unambiguous(true)).parse();
    let mut quasis = Quasis(Vec::new());
    quasis.visit_program(&parsed.program);
    let mut lines = std::collections::HashSet::new();
    if quasis.0.is_empty() {
        return lines;
    }
    for (index, (offset, _)) in code.match_indices('\n').enumerate() {
        let start = offset as u32 + 1;
        if quasis.0.iter().any(|(from, to)| *from < start && start <= *to) {
            lines.insert(index + 1);
        }
    }
    lines
}

/// The specifiers an ES module imports: static imports, re-exports, and dynamic
/// `import()` of a string literal. A computed `import()` is not listed.
///
/// # Arguments
/// * `path` - the module's name, for errors.
/// * `text` - an ES module (already transformed: no types, no CommonJS).
///
/// # Returns
/// Specifiers as written, sorted and unique.
///
/// # Errors
/// A syntax error.
pub fn imports(path: &str, text: &str) -> Result<Vec<String>, Error> {
    /// Collects specifiers.
    struct Imports(Vec<String>);
    impl<'a> Visit<'a> for Imports {
        fn visit_import_declaration(&mut self, it: &ast::ImportDeclaration<'a>) {
            self.0.push(it.source.value.display().to_string());
            walk::walk_import_declaration(self, it);
        }
        fn visit_export_from_declaration(&mut self, it: &ast::ExportFromDeclaration<'a>) {
            self.0.push(it.source.value.display().to_string());
            walk::walk_export_from_declaration(self, it);
        }
        fn visit_export_all_declaration(&mut self, it: &ast::ExportAllDeclaration<'a>) {
            self.0.push(it.source.value.display().to_string());
            walk::walk_export_all_declaration(self, it);
        }
        fn visit_import_expression(&mut self, it: &ast::ImportExpression<'a>) {
            if let ast::Expression::StringLiteral(source) = &it.source {
                self.0.push(source.value.display().to_string());
            }
            walk::walk_import_expression(self, it);
        }
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, SourceType::mjs()).parse();
    if let Some(error) = parsed.diagnostics.errors().next() {
        return Err(failure(path, "parse", error));
    }
    let mut found = Imports(Vec::new());
    found.visit_program(&parsed.program);
    found.0.sort();
    found.0.dedup();
    Ok(found.0)
}

/// What a CommonJS file asks for and offers.
#[derive(Default, Debug)]
pub struct CommonJs {
    /// Literal `require()` specifiers, sorted and unique.
    pub requires: Vec<String>,
    /// Export names found statically, so ES modules can import them by name.
    pub exports: Vec<String>,
}

/// Scans a CommonJS file for its literal requires and static export names.
///
/// # Errors
/// A syntax error.
pub fn scan_commonjs(path: &str, text: &str) -> Result<CommonJs, Error> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, SourceType::cjs()).parse();
    if let Some(e) = parsed.diagnostics.errors().next() {
        return Err(failure(path, "syntax error", e.message.as_ref()));
    }
    let mut scan = Scan::default();
    scan.visit_program(&parsed.program);
    let mut out = scan.found;
    for list in [&mut out.requires, &mut out.exports] {
        list.sort();
        list.dedup();
    }
    out.exports.retain(|name| name != "default" && name != "__esModule" && identifier(name));
    Ok(out)
}

/// The AST visitor behind [`scan_commonjs`].
#[derive(Default)]
struct Scan {
    /// What was found so far.
    found: CommonJs,
}

/// Whether `e` is the identifier `name`.
fn is_identifier(e: &ast::Expression<'_>, name: &str) -> bool {
    matches!(e, ast::Expression::Identifier(i) if i.name == name)
}

/// Whether `e` is `exports` or `module.exports`.
fn is_exports(e: &ast::Expression<'_>) -> bool {
    is_identifier(e, "exports")
        || matches!(e, ast::Expression::StaticMemberExpression(m)
            if m.property.name == "exports" && is_identifier(&m.object, "module"))
}

/// The text of a string literal or a template literal without substitutions.
fn literal(e: &ast::Argument<'_>) -> Option<String> {
    match e {
        ast::Argument::StringLiteral(s) => Some(s.value.display().to_string()),
        ast::Argument::TemplateLiteral(t) if t.expressions.is_empty() && t.quasis.len() == 1 => {
            t.quasis[0].value.cooked.map(|s| s.display().to_string())
        }
        _ => None,
    }
}

impl<'a> Visit<'a> for Scan {
    fn visit_call_expression(&mut self, it: &ast::CallExpression<'a>) {
        if is_identifier(&it.callee, "require")
            && it.arguments.len() == 1
            && let Some(specifier) = literal(&it.arguments[0])
        {
            self.found.requires.push(specifier);
        }
        // Object.defineProperty(exports, "name", ...)
        if let ast::Expression::StaticMemberExpression(m) = &it.callee
            && m.property.name == "defineProperty"
            && is_identifier(&m.object, "Object")
            && it.arguments.len() >= 2
            && it.arguments[0].as_expression().is_some_and(is_exports)
            && let Some(name) = literal(&it.arguments[1])
        {
            self.found.exports.push(name);
        }
        walk::walk_call_expression(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &ast::AssignmentExpression<'a>) {
        match &it.left {
            // exports.name = ..., module.exports.name = ...
            ast::AssignmentTarget::StaticMemberExpression(m) if is_exports(&m.object) => {
                self.found.exports.push(m.property.name.to_string());
            }
            // module.exports = { a, b: ..., "c": ... }
            ast::AssignmentTarget::StaticMemberExpression(m)
                if m.property.name == "exports" && is_identifier(&m.object, "module") =>
            {
                if let ast::Expression::ObjectExpression(o) = &it.right {
                    for property in &o.properties {
                        if let ast::ObjectPropertyKind::ObjectProperty(p) = property
                            && !p.computed
                            && let Some(name) = p.key.static_name()
                        {
                            self.found.exports.push(name.as_js_str().display().to_string());
                        }
                    }
                }
            }
            _ => {}
        }
        walk::walk_assignment_expression(self, it);
    }
}

/// Whether `name` is a plain ASCII identifier, usable after `.`.
fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c == '_' || c == '$' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric())
}

/// How a body turns an imported namespace into `require()`'s value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Required {
    /// Another CommonJS body: its `module.exports`.
    CommonJs,
    /// A JSON module: its value.
    Json,
    /// An ES module or builtin: its namespace.
    Namespace,
}

/// The body module that runs `text` as CommonJS when first required.
///
/// It exports only its loader, `__lunarstorm_cjs()`, which returns
/// `module.exports`. `requires` are the specifiers that resolved, each imported
/// under its own import text; any other throws `MODULE_NOT_FOUND` when required.
/// The prefix is on the first line, so the body keeps its line numbers.
///
/// # Arguments
/// * `text` - the CommonJS source.
/// * `filename` - its absolute path, for `__filename` and `__dirname`.
/// * `requires` - `(specifier, import text, how its value is taken)`.
pub fn commonjs_body(text: &str, filename: &str, requires: &[(String, String, Required)]) -> String {
    let quote = |s: &str| serde_json::to_string(s).expect("strings serialize");
    let dirname = filename.rsplit_once('/').map_or("/", |(d, _)| if d.is_empty() { "/" } else { d });
    let mut out = String::new();
    let mut cases = String::new();
    for (index, (specifier, import, required)) in requires.iter().enumerate() {
        out.push_str(&format!("import * as __ls_r{index} from {};", quote(import)));
        let value = match required {
            Required::CommonJs => format!("__ls_r{index}.__lunarstorm_cjs()"),
            Required::Json => format!("__ls_r{index}.default"),
            Required::Namespace => format!("__ls_r{index}"),
        };
        cases.push_str(&format!("case {}:return {value};", quote(specifier)));
    }
    out.push_str(&format!(
        "var __ls_m;export function __lunarstorm_cjs(){{if(__ls_m!==undefined)return __ls_m.exports;\
         __ls_m={{exports:{{}},id:{f},filename:{f},loaded:false}};\
         var __ls_req=function require(s){{switch(s){{{cases}}}\
         var e=new Error(\"Cannot find module '\"+s+\"'\");e.code=\"MODULE_NOT_FOUND\";throw e}};\
         (function(exports,require,module,__filename,__dirname){{",
        f = quote(filename),
    ));
    // A shebang is only valid at the very start; inside the wrapper it becomes a comment.
    match text.strip_prefix("#!") {
        Some(rest) => {
            out.push_str("//");
            out.push_str(rest);
        }
        None => out.push_str(text),
    }
    out.push_str(&format!(
        "\n}}).call(__ls_m.exports,__ls_m.exports,__ls_req,__ls_m,{},{});\
         __ls_m.loaded=true;return __ls_m.exports}}\n",
        quote(filename),
        quote(dirname),
    ));
    out
}

/// The ES module an `import` of a CommonJS file reaches: it runs the body imported
/// as `body`, and exports `module.exports` as `default` and each name in `exports`.
pub fn commonjs_facade(body: &str, exports: &[String]) -> String {
    let quote = serde_json::to_string(body).expect("strings serialize");
    let mut out =
        format!("import * as __ls_b from {quote};var __ls_e=__ls_b.__lunarstorm_cjs();export default __ls_e;\n");
    // `export { local as name }` takes any identifier name, reserved words included.
    for (index, name) in exports.iter().enumerate() {
        out.push_str(&format!(
            "var __ls_x{index}=__ls_e==null?undefined:__ls_e.{name};export{{__ls_x{index} as {name}}};\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_lists_static_re_exported_and_literal_dynamic_specifiers() {
        let text = "import a from './a.js';\nimport './side.js';\nexport { b } from 'b';\nexport * from \"c\";\nexport function f(x) { return [import('./d.js'), import(x)]; }\nimport again from './a.js';\n";
        assert_eq!(imports("/m.js", text).expect("parses"), ["./a.js", "./d.js", "./side.js", "b", "c"]);
        assert!(imports("/bad.js", "import from;").is_err());
    }

    #[test]
    fn types_are_stripped_and_lines_keep_their_numbers() {
        let ts = "interface A { x: number }\n\nconst a: A = { x: 1 };\nenum E { One = 1 }\nexport const b = a.x + E.One;\nthrow new Error(\"line 6\");\n";
        let js = strip_types("/m.ts", ts).expect("strips");
        assert!(!js.contains("interface") && !js.contains(": A"), "{js}");
        let line = js.lines().position(|l| l.contains("line 6")).expect("throw kept");
        assert_eq!(line, 5, "{js}");
        assert!(strip_types("/bad.ts", "const x: = 1;").is_err());
    }

    #[test]
    fn template_literal_lines_are_never_joined() {
        let ts = "enum E { A, B, C }\nconst t: string = `a\n  b ${E.A}\nc`;\n// a comment;\nexport const z = t;\n";
        let js = strip_types("/t.ts", ts).expect("strips");
        assert!(js.contains("`a\n  b ${E.A}\nc`"), "{js}");
        assert_eq!(js.lines().position(|l| l.contains("export const z")), Some(5), "{js}");
    }

    #[test]
    fn module_syntax_is_detected() {
        assert!(has_module_syntax("/a.js", "export const a = 1;"));
        assert!(has_module_syntax("/a.js", "console.log(import.meta.url);"));
        assert!(has_module_syntax("/a.js", "await Promise.resolve();"));
        assert!(!has_module_syntax("/a.js", "module.exports = 1;"));
        assert!(has_module_syntax("/a.ts", "export type T = number;"));
    }

    #[test]
    fn commonjs_requires_and_exports_are_found() {
        let scan = scan_commonjs(
            "/a.js",
            "const b = require('./b'); exports.x = 1; module.exports.y = 2; Object.defineProperty(exports, 'z', {}); require(`c`);",
        )
        .expect("scans");
        assert_eq!(scan.requires, ["./b", "c"]);
        assert_eq!(scan.exports, ["x", "y", "z"]);
        let literal = scan_commonjs("/a.js", "module.exports = { a, b: 1, 'c': 2, [d]: 3 };").expect("scans");
        assert_eq!(literal.exports, ["a", "b", "c"]);
    }

    #[test]
    fn json_becomes_a_default_export() {
        assert_eq!(json_module("/a.json", "{\"a\":1}").expect("json"), "export default JSON.parse(\"{\\\"a\\\":1}\");\n");
        assert!(json_module("/a.json", "{").is_err());
    }
}
