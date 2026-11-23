//! On-demand compile of one request: app source, dependency files, and the JS wrappers for CSS,
//! JSON and asset imports. Every compiled module registers its imports and HMR acceptance in
//! the module graph; output is cached per URL until the file or an import's HMR stamp changes.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::module_urls::{ext, has_flag, is_bare, split_query, url_of, COMPILABLE};
use crate::State;

/// A compiled response body.
pub struct Module {
    pub content_type: &'static str,
    pub code: String,
}

/// One cache entry: the inputs the output was built from.
pub(crate) struct Cached {
    modified: SystemTime,
    len: u64,
    imports_stamp: u64,
    query: String,
    code: String,
    pub(crate) scans_sources: bool,
}

const JS: &str = "text/javascript";

/// Whether a request is a module request this pipeline answers (as opposed to a static file).
pub fn is_module_request(path: &str, query: &str) -> bool {
    COMPILABLE.contains(&ext(path).as_str()) || has_flag(query, "import")
}

/// Compiles the module at `path` (URL path, `file` on disk) for a request carrying `query`.
///
/// Reuses cache entries only when source metadata, import stamps, and timestamp
/// query agree. Binary assets become URL modules without UTF-8 decoding. Successful
/// compilation updates the module graph before caching; errors leave no new entry.
pub fn load(state: &State, path: &str, query: &str, file: &Path) -> Result<Module, String> {
    let meta = fs::metadata(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let imports_stamp = state
        .graph
        .lock()
        .unwrap()
        .imports_timestamp(Path::new(path));
    let key_query = timestamp_query(query);
    if let Some(hit) = state.cache.lock().unwrap().get(path) {
        if hit.modified == modified
            && hit.len == meta.len()
            && hit.imports_stamp == imports_stamp
            && hit.query == key_query
        {
            return Ok(Module {
                content_type: JS,
                code: hit.code.clone(),
            });
        }
    }
    let e = ext(path);
    // Asset imports expose a URL, not the contents of a binary file.
    let source = if COMPILABLE.contains(&e.as_str()) || matches!(e.as_str(), "css" | "json") {
        fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?
    } else {
        String::new()
    };
    let code = if e == "css" {
        css_module(state, path, file, &source)?
    } else if e == "json" {
        state.graph.lock().unwrap().ensure_module(Path::new(path));
        oj_compiler::json::to_esm(&source, path).map_err(|err| err.to_string())?
    } else if !COMPILABLE.contains(&e.as_str()) {
        state.graph.lock().unwrap().ensure_module(Path::new(path));
        format!("export default {path:?};\n")
    } else if file.components().any(|c| c.as_os_str() == "node_modules") {
        dependency_module(state, path, file, &source)?
    } else {
        app_module(state, path, query, file, &source)?
    };
    state.cache.lock().unwrap().insert(
        path.to_string(),
        Cached {
            modified,
            len: meta.len(),
            imports_stamp,
            query: key_query,
            code: code.clone(),
            scans_sources: e == "css"
                && source.contains("@import")
                && state.root.join("node_modules/@tailwindcss/node").exists(),
        },
    );
    Ok(Module {
        content_type: JS,
        code,
    })
}

/// App source: CJS-dependency interop, oj_compiler dev compile (React Refresh), hot context and
/// refresh-boundary glue, graph registration, inline source map.
fn app_module(
    state: &State,
    path: &str,
    query: &str,
    file: &Path,
    source: &str,
) -> Result<String, String> {
    let dir = file.parent().unwrap_or(Path::new("/"));
    let interop = |spec: &str| cjs_dependency_url(state, dir, spec);
    let interopped = oj_compiler::interop::rewrite_cjs_interop(source, file, &interop);
    let unresolved = RefCell::new(None::<String>);
    let mut rewrite = |spec: &str| {
        let url = rewrite_specifier(state, dir, spec, true);
        if url.is_none() && is_bare(spec) && !spec.starts_with("node:") {
            unresolved
                .borrow_mut()
                .get_or_insert_with(|| spec.to_string());
        }
        url
    };
    let mut opts = oj_compiler::CompileOptions::dev();
    opts.env = Some(state.env.clone());
    let out = oj_compiler::compile_module(
        file,
        interopped.as_deref().unwrap_or(source),
        &opts,
        Some(&mut rewrite),
    )
    .map_err(|e| e.to_string())?;
    if let Some(spec) = unresolved.into_inner() {
        return Err(format!(
            "{}: cannot resolve import {spec:?}",
            file.display()
        ));
    }

    let self_accepting =
        out.is_refresh_boundary || out.hot_accept.as_ref().is_some_and(|h| h.self_accepting);
    register(state, path, &out, self_accepting);

    let uses_hot = out.is_refresh_boundary || out.hot_accept.is_some();
    let banner = if uses_hot {
        format!("import {{ createHotContext as __dm_createHotContext }} from \"/@devermind/client.js\";\nimport.meta.hot = __dm_createHotContext({path:?});\n")
    } else {
        String::new()
    };
    let mut code = banner.clone();
    code.push_str(&out.code);
    if out.is_refresh_boundary {
        let self_url = if query.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{query}")
        };
        code.push_str(&refresh_glue(path, &self_url));
    }
    if let Some(map) = &out.map_json {
        code.push_str("\n//# sourceMappingURL=");
        code.push_str(&oj_compiler::map_json_to_data_url(&shift_map(
            map,
            banner.matches('\n').count(),
        )));
        code.push('\n');
    }
    Ok(code)
}

/// A file under `node_modules`: ESM is compiled with the dev defines, CommonJS is wrapped as ESM.
fn dependency_module(
    state: &State,
    path: &str,
    file: &Path,
    source: &str,
) -> Result<String, String> {
    let dir = file.parent().unwrap_or(Path::new("/"));
    let interop = |spec: &str| cjs_dependency_url(state, dir, spec);
    let esm = oj_compiler::cjs::has_module_syntax_pub(file, source);
    let interopped = if esm {
        oj_compiler::interop::rewrite_cjs_interop(source, file, &interop)
    } else {
        None
    };
    let mut resolve = |spec: &str| rewrite_specifier(state, dir, spec, false);
    let out = oj_compiler::cjs::compile_dep(
        file,
        path,
        interopped.as_deref().unwrap_or(source),
        &mut resolve,
        Some(state.env.clone()),
    )
    .map_err(|e| e.to_string())?;
    register(state, path, &out, false);
    Ok(out.code)
}

/// A stylesheet imported from JS: injects a `<style>` and accepts its own updates.
fn css_module(state: &State, path: &str, file: &Path, source: &str) -> Result<String, String> {
    let (css, dependencies) = if source.contains("@import")
        && state.root.join("node_modules/@tailwindcss/node").exists()
    {
        crate::stylesheets::compile(&state.root, file)?
    } else {
        (source.to_string(), Vec::new())
    };
    let mut graph = state.graph.lock().unwrap();
    graph.ensure_module(Path::new(path));
    graph.set_self_accepting(Path::new(path), true);
    let imports = dependencies
        .iter()
        .filter(|dep| Path::new(dep.as_str()) != file)
        .map(|dep| PathBuf::from(url_of(&state.root, Path::new(dep))))
        .collect::<Vec<_>>();
    graph.set_imports(Path::new(path), &imports);
    Ok(format!(
        "import {{ createHotContext as __dm_createHotContext, updateStyle as __dm_updateStyle, removeStyle as __dm_removeStyle }} from \"/@devermind/client.js\";\n\
         import.meta.hot = __dm_createHotContext({id:?});\n\
         const __dm_css = {css};\n\
         __dm_updateStyle({path:?}, __dm_css);\n\
         import.meta.hot.accept();\n\
         import.meta.hot.prune(() => __dm_removeStyle({path:?}));\n\
         export default __dm_css;\n",
        id = format!("{path}?import"),
        css = serde_json::Value::String(css),
    ))
}

/// Records a compiled module's imports, used bindings and HMR acceptance.
fn register(state: &State, path: &str, out: &oj_compiler::CompileOutput, self_accepting: bool) {
    let key = Path::new(path);
    let module_url = |spec: &String| -> Option<PathBuf> {
        let (base, _) = split_query(spec);
        (base.starts_with('/') && !base.starts_with("/@devermind/")).then(|| PathBuf::from(base))
    };
    let imports: Vec<PathBuf> = out
        .imports
        .iter()
        .chain(&out.dynamic_imports)
        .filter_map(module_url)
        .collect();
    let bindings: Vec<(PathBuf, Vec<String>)> = out
        .import_bindings
        .iter()
        .filter_map(|(spec, names)| Some((module_url(spec)?, names.clone())))
        .collect();
    let mut graph = state.graph.lock().unwrap();
    graph.ensure_module(key);
    graph.set_imports(key, &imports);
    graph.set_imported_bindings(
        key,
        bindings
            .iter()
            .map(|(p, names)| (p.as_path(), names.as_slice())),
    );
    graph.set_self_accepting(key, self_accepting);
    if let Some(hot) = &out.hot_accept {
        let deps: Vec<PathBuf> = hot.deps.iter().filter_map(module_url).collect();
        graph.set_accepted_deps(key, &deps);
        graph.set_accepted_exports(key, hot.accepted_exports.as_deref());
    }
}

/// The URL an import specifier is served at. Non-JS imports get `?import` (CSS, JSON, assets as
/// JS modules); `stamp` appends the imported module's HMR timestamp so an importer re-fetched
/// after an update loads the new version.
pub(crate) fn rewrite_specifier(
    state: &State,
    dir: &Path,
    spec: &str,
    stamp: bool,
) -> Option<String> {
    if spec.contains("://") || spec.starts_with("data:") || spec.starts_with("/@devermind/") {
        return None;
    }
    let (base, query) = split_query(spec);
    let url_path = if base.starts_with('/') {
        base.to_string()
    } else {
        url_of(&state.root, &state.resolve_import(dir, base)?)
    };
    let mut flags: Vec<String> = query
        .split('&')
        .filter(|kv| !kv.is_empty() && !kv.starts_with("t="))
        .map(String::from)
        .collect();
    if !COMPILABLE.contains(&ext(&url_path).as_str())
        && !flags
            .iter()
            .any(|f| f == "import" || f == "url" || f == "raw")
    {
        flags.push("import".to_string());
    }
    if stamp {
        let ts = state
            .graph
            .lock()
            .unwrap()
            .hmr_timestamp(Path::new(&url_path));
        if ts > 0 {
            flags.push(format!("t={ts}"));
        }
    }
    Some(if flags.is_empty() {
        url_path
    } else {
        format!("{url_path}?{}", flags.join("&"))
    })
}

/// For a bare import of a CommonJS file under `node_modules`: its URL, so the importer reads
/// named imports off `module.exports` (oj_compiler's interop rewrite). `None` for ESM.
fn cjs_dependency_url(state: &State, dir: &Path, spec: &str) -> Option<String> {
    if !is_bare(spec) {
        return None;
    }
    let resolved = state.resolve_import(dir, spec)?;
    if !resolved
        .components()
        .any(|c| c.as_os_str() == "node_modules")
        || !is_cjs_file(state, &resolved)
    {
        return None;
    }
    Some(url_of(&state.root, &resolved))
}

fn is_cjs_file(state: &State, path: &Path) -> bool {
    if let Some(&known) = state.cjs_files.lock().unwrap().get(path) {
        return known;
    }
    let cjs = matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("js" | "cjs")
    ) && fs::read_to_string(path)
        .is_ok_and(|src| !oj_compiler::cjs::has_module_syntax_pub(path, &src));
    state
        .cjs_files
        .lock()
        .unwrap()
        .insert(path.to_path_buf(), cjs);
    cjs
}

/// React Refresh registration for a module whose exports are all components (oj's hot glue).
fn refresh_glue(id: &str, self_url: &str) -> String {
    format!(
        r#"
import * as __dm_RefreshRuntime from "/@devermind/refresh-runtime.js";
import * as __dm_currentExports from {self_url:?};
if (import.meta.hot) {{
  if (!window.__devermind_refresh_installed__) {{
    throw new Error("devermind: Fast Refresh preamble missing; was index.html served by devermind?");
  }}
  __dm_RefreshRuntime.registerExportsForReactRefresh({id:?}, __dm_currentExports);
  import.meta.hot.accept((nextExports) => {{
    if (!nextExports) return;
    const invalidateMessage = __dm_RefreshRuntime.validateRefreshBoundaryAndEnqueueUpdate({id:?}, __dm_currentExports, nextExports);
    if (invalidateMessage) import.meta.hot.invalidate(invalidateMessage);
  }});
}}
function $RefreshReg$(type, id) {{ return __dm_RefreshRuntime.register(type, {id:?} + " " + id); }}
function $RefreshSig$() {{ return __dm_RefreshRuntime.createSignatureFunctionForTransform(); }}
"#
    )
}

/// The cache-relevant part of a query: the HMR timestamp the browser asked for.
fn timestamp_query(query: &str) -> String {
    query
        .split('&')
        .filter(|kv| kv.starts_with("t="))
        .collect::<Vec<_>>()
        .join("&")
}

/// Moves a source map down by `lines` generated lines (the prepended hot-context banner).
fn shift_map(map_json: &str, lines: usize) -> String {
    if lines == 0 {
        return map_json.to_string();
    }
    match serde_json::from_str::<serde_json::Value>(map_json) {
        Ok(mut map) => {
            if let Some(serde_json::Value::String(mappings)) = map.get_mut("mappings") {
                mappings.insert_str(0, &";".repeat(lines));
            }
            map.to_string()
        }
        Err(_) => map_json.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_map_prefixes_mappings() {
        let shifted = shift_map(r#"{"version":3,"mappings":"AAAA"}"#, 2);
        assert!(shifted.contains(r#""mappings":";;AAAA""#), "{shifted}");
    }

    #[test]
    fn timestamp_query_keeps_only_t() {
        assert_eq!(timestamp_query("import&t=5"), "t=5");
        assert_eq!(timestamp_query(""), "");
    }
}
