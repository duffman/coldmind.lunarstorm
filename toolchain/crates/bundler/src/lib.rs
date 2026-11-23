//! Production bundler: `<root>/index.html` in, a static site in `<out>`.
//!
//! - JS/TS/JSX: rolldown, from every `<script type="module" src>` in
//!   `index.html`, with tsconfig `paths`, production `import.meta.env` and
//!   content-hashed chunks under `assets/`.
//! - CSS: every imported stylesheet, in module execution order, through
//!   Tailwind 4 (`@tailwindcss/node`, resolved from the app) when the app uses
//!   it, into one hashed stylesheet. Relative `url()`s become hashed assets.
//! - Images and fonts imported from JS: hashed files under `assets/`, the
//!   import's default export is their URL.
//! - `public/` is copied as is; `index.html` points at the built files.

pub mod desktop;

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context};
use rolldown::{
    BundlerBuilder, BundlerOptions, ChunkFilenamesOutputOption, InputItem, RawMinifyOptions,
    TsConfig,
};
use rolldown_common::side_effects::HookSideEffects;
use rolldown_common::{ModuleType, Output};
use rolldown_plugin::{
    HookLoadArgs, HookLoadOutput, HookLoadReturn, HookUsage, Plugin, SharedLoadPluginContext,
};

/// Extensions emitted as files, with the import's default export as their URL.
const ASSET_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "ico", "bmp", "woff", "woff2", "ttf",
    "otf", "eot", "mp3", "mp4", "webm", "wav", "ogg", "pdf", "wasm",
];

/// Project root and explicit overrides for a native-config-aware static build.
pub struct BuildOptions {
    /// App root: `index.html`, `public/`, `tsconfig.json`, `package.json`.
    pub root: PathBuf,
    /// Explicit output override, relative to the effective app root unless absolute.
    /// `None` uses project build.outDir, then `dist`. Replaced on build.
    pub out: Option<PathBuf>,
    /// Explicit minification override; `None` uses project build.minify, then true.
    pub minify: Option<bool>,
    /// Select production environment constants and React runtime (default true).
    pub production: bool,
}

impl BuildOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            out: None,
            minify: None,
            production: true,
        }
    }
}

/// Emitted entry, stylesheet and chunk locations after a successful build.
#[derive(Debug)]
pub struct BuildReport {
    pub out: PathBuf,
    /// `(index.html src, built file)` per entry.
    pub entries: Vec<(String, String)>,
    pub stylesheet: Option<String>,
    pub chunks: usize,
}

/// Bundle an HTML frontend and its imported modules into a static output directory.
///
/// Explicit options override project configuration. Output paths are canonicalized before any
/// removal so symlinks cannot redirect cleanup onto the project or app root. The previous output
/// is cleared before compilation: a later compilation error can leave partial output and is not
/// a transaction. Missing browser dependencies fail rather than becoming unresolved imports.
pub async fn build(options: &BuildOptions) -> anyhow::Result<BuildReport> {
    let config = devermind_core::config::load(
        &options.root,
        "build",
        if options.production {
            "production"
        } else {
            "development"
        },
    )?;
    build_with_config(options, config).await
}

/// Build using an explicitly supplied native configuration; never discovers JS config.
pub async fn build_with_config(
    options: &BuildOptions,
    config: Option<devermind_core::config::ProjectConfig>,
) -> anyhow::Result<BuildReport> {
    let app_root = config
        .as_ref()
        .and_then(|c| c.root.as_ref())
        .unwrap_or(&options.root);
    let root = app_root
        .canonicalize()
        .with_context(|| format!("app root {}", app_root.display()))?;
    let out_option = options
        .out
        .clone()
        .or_else(|| config.as_ref().and_then(|c| c.build_out_dir.clone()))
        .unwrap_or_else(|| PathBuf::from("dist"));
    let minify = options
        .minify
        .or_else(|| config.as_ref().and_then(|c| c.build_minify))
        .unwrap_or(true);
    let out = if out_option.is_absolute() {
        out_option
    } else {
        root.join(out_option)
    };
    let mut defines = build_defines(options.production);
    if let Some(config) = &config {
        defines.extend(config.define.iter().cloned());
    }
    let aliases = config.as_ref().map(|c| {
        c.alias
            .iter()
            .map(|(key, value)| (key.clone(), vec![Some(value.clone())]))
            .collect()
    });
    let out = prospective_path(&out)?;
    let project_root = options.root.canonicalize()?;
    if root.starts_with(&out) || project_root.starts_with(&out) {
        bail!("output {} would replace the app root", out.display());
    }
    let html_path = root.join("index.html");
    let html = std::fs::read_to_string(&html_path)
        .with_context(|| format!("cannot read {}", html_path.display()))?;
    let scripts = module_scripts(&html);
    if scripts.is_empty() {
        bail!(
            "{} has no <script type=\"module\" src>",
            html_path.display()
        );
    }

    // Build output only: the previous build is replaced, never merged.
    if out.exists() {
        std::fs::remove_dir_all(&out).with_context(|| format!("cannot clear {}", out.display()))?;
    }
    let assets_dir = out.join("assets");
    std::fs::create_dir_all(&assets_dir)?;

    let styles = Arc::new(Mutex::new(Vec::<String>::new()));
    let plugin = AppFiles {
        assets: assets_dir.clone(),
        styles: styles.clone(),
    };

    let mut inputs = Vec::new();
    for script in &scripts {
        let rel = script.src.trim_start_matches('/');
        let name = Path::new(rel)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "index".into());
        inputs.push(InputItem {
            name: Some(name),
            import: format!("./{rel}"),
        });
    }
    let input_names: Vec<String> = inputs.iter().filter_map(|i| i.name.clone()).collect();

    let tsconfig = root.join("tsconfig.json");
    let mut bundler = BundlerBuilder::default()
        .with_plugins(vec![rolldown_plugin::__inner::Pluginable::new_shared(
            plugin,
        )])
        .with_options(BundlerOptions {
            input: Some(inputs),
            cwd: Some(root.clone()),
            dir: Some(out.display().to_string()),
            entry_filenames: Some(ChunkFilenamesOutputOption::from(
                "assets/[name]-[hash].js".to_string(),
            )),
            chunk_filenames: Some(ChunkFilenamesOutputOption::from(
                "assets/[name]-[hash].js".to_string(),
            )),
            minify: Some(RawMinifyOptions::from(minify)),
            tsconfig: Some(if tsconfig.is_file() {
                TsConfig::Manual(tsconfig)
            } else {
                TsConfig::Auto(true)
            }),
            define: Some(defines.into_iter().collect()),
            resolve: Some(rolldown::ResolveOptions {
                alias: aliases,
                // Linked source packages may live outside the application tree.
                // Resolve their peer dependencies from the app after local resolution.
                modules: Some(vec![
                    "node_modules".into(),
                    root.join("node_modules").display().to_string(),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        })
        .build()
        .map_err(|e| anyhow!("bundler setup failed: {e:?}"))?;
    // Debug summaries omit source labels; render diagnostics so CLI failures
    // identify the broken file and location rather than only an error category.
    let output = bundler.write().await.map_err(|errors| {
        let diagnostics = errors
            .iter()
            .map(|error| error.to_diagnostic().convert_to_string(false))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow!("bundle failed:\n{diagnostics}")
    })?;
    bundler
        .close()
        .await
        .map_err(|e| anyhow!("bundler close failed: {e:?}"))?;
    for warning in &output.warnings {
        if warning.kind().to_string() == "UNRESOLVED_IMPORT" {
            bail!("unresolved browser dependency: {warning:?}");
        }
    }

    // Entry file per index.html script, and stylesheets in execution order:
    // entry chunks first, then the rest, each in its module order.
    let mut entry_files = std::collections::HashMap::new();
    let mut chunks: Vec<&rolldown_common::OutputChunk> = Vec::new();
    for asset in &output.assets {
        if let Output::Chunk(chunk) = asset {
            if chunk.is_entry {
                entry_files.insert(chunk.name.to_string(), chunk.filename.to_string());
            }
            chunks.push(chunk);
        }
    }
    chunks.sort_by_key(|c| !c.is_entry);
    let loaded: Vec<String> = styles.lock().unwrap().clone();
    let mut ordered: Vec<String> = Vec::new();
    for chunk in &chunks {
        for id in &chunk.module_ids {
            let id = strip_query(id.as_str()).to_string();
            if loaded.contains(&id) && !ordered.contains(&id) {
                ordered.push(id);
            }
        }
    }
    for id in loaded {
        if !ordered.contains(&id) {
            ordered.push(id);
        }
    }

    let stylesheet = if ordered.is_empty() {
        None
    } else {
        let css = compile_styles(&root, &ordered, &assets_dir, minify).await?;
        let name = format!("assets/style-{}.css", short_hash(css.as_bytes()));
        std::fs::write(out.join(&name), css)?;
        Some(name)
    };

    let public = root.join("public");
    if public.is_dir() {
        copy_dir(&public, &out)?;
    }

    let mut page = html.clone();
    let mut entries = Vec::new();
    for (script, name) in scripts.iter().rev().zip(input_names.iter().rev()) {
        let file = entry_files
            .get(name)
            .ok_or_else(|| anyhow!("no output chunk for entry {}", script.src))?;
        page.replace_range(script.src_range.clone(), &format!("/{file}"));
        entries.push((script.src.clone(), file.clone()));
    }
    entries.reverse();
    if let Some(sheet) = &stylesheet {
        let link = format!("    <link rel=\"stylesheet\" href=\"/{sheet}\">\n  ");
        match page.find("</head>") {
            Some(at) => page.insert_str(at, &link),
            None => page.insert_str(0, &link),
        }
    }
    std::fs::write(out.join("index.html"), page)?;

    Ok(BuildReport {
        out,
        entries,
        stylesheet,
        chunks: chunks.len(),
    })
}

/// Resolve existing symlinks before checking a directory that will be cleared.
fn prospective_path(path: &Path) -> anyhow::Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("invalid output directory {}", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("ambiguous output directory {}", path.display()))?;
    Ok(prospective_path(parent)?.join(name))
}

/// `import.meta.env` as Vite defines it for a production client build, plus
/// `VITE_*` variables from the environment.
fn build_defines(production: bool) -> Vec<(String, String)> {
    let mut env = serde_json::Map::new();
    env.insert(
        "MODE".into(),
        if production {
            "production"
        } else {
            "development"
        }
        .into(),
    );
    env.insert("DEV".into(), (!production).into());
    env.insert("PROD".into(), production.into());
    env.insert("SSR".into(), false.into());
    env.insert("BASE_URL".into(), "/".into());
    let mut vars: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| k.starts_with("VITE_"))
        .collect();
    vars.sort();
    for (key, value) in vars {
        env.insert(key, value.into());
    }
    let mut defines: Vec<(String, String)> = env
        .iter()
        .map(|(key, value)| (format!("import.meta.env.{key}"), value.to_string()))
        .collect();
    defines.push((
        "import.meta.env".into(),
        serde_json::Value::Object(env).to_string(),
    ));
    defines.push(("import.meta.hot".into(), "undefined".into()));
    defines.push((
        "process.env.NODE_ENV".into(),
        if production {
            "\"production\""
        } else {
            "\"development\""
        }
        .into(),
    ));
    defines
}

/// Stylesheets and asset files imported from JS.
#[derive(Debug)]
struct AppFiles {
    assets: PathBuf,
    styles: Arc<Mutex<Vec<String>>>,
}

impl Plugin for AppFiles {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("devermind:app-files")
    }

    async fn load(&self, _ctx: SharedLoadPluginContext, args: &HookLoadArgs<'_>) -> HookLoadReturn {
        let path = strip_query(args.id);
        let ext = Path::new(path)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if ext == "css" {
            let mut styles = self.styles.lock().unwrap();
            if !styles.iter().any(|s| s == path) {
                styles.push(path.to_string());
            }
            return Ok(Some(HookLoadOutput {
                code: "export default \"\";\n".to_string().into(),
                side_effects: Some(HookSideEffects::NoTreeshake),
                module_type: Some(ModuleType::Js),
                ..Default::default()
            }));
        }
        if ASSET_EXTS.contains(&ext.as_str()) {
            let url = emit_asset(Path::new(path), &self.assets).map_err(|e| anyhow!("{e:#}"))?;
            return Ok(Some(HookLoadOutput {
                code: format!("export default {};\n", serde_json::Value::from(url)).into(),
                module_type: Some(ModuleType::Js),
                ..Default::default()
            }));
        }
        Ok(None)
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::Load
    }
}

/// Copies `file` to `assets/<stem>-<hash>.<ext>` and returns its URL.
fn emit_asset(file: &Path, assets: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    let stem = file.file_stem().unwrap_or_default().to_string_lossy();
    let ext = file.extension().unwrap_or_default().to_string_lossy();
    let name = format!("{stem}-{}.{ext}", short_hash(&bytes));
    let target = assets.join(&name);
    if !target.exists() {
        std::fs::write(&target, &bytes)?;
    }
    Ok(format!("/assets/{name}"))
}

/// Compile imported stylesheets and emit their relative assets before combining the CSS.
///
/// Plain CSS URLs are relative to each original stylesheet, not the app root or output file.
/// Tailwind rebases imported URLs against the project root, so its combined output uses that
/// base instead. Emitted asset URLs are absolute; remote, data and public URLs remain unchanged.
async fn compile_styles(
    root: &Path,
    files: &[String],
    assets: &Path,
    minify: bool,
) -> anyhow::Result<String> {
    let uses_tailwind = files.iter().any(|f| {
        std::fs::read_to_string(f).is_ok_and(|css| {
            css.contains("@import \"tailwindcss") || css.contains("@import 'tailwindcss")
        })
    });
    if !uses_tailwind {
        let mut css = String::new();
        for file in files {
            let path = Path::new(file);
            let source = std::fs::read_to_string(path)?;
            css.push_str(&rebase_urls(
                &source,
                path.parent().unwrap_or(root),
                assets,
            )?);
            css.push('\n');
        }
        return Ok(css);
    }
    let css = run_tailwind(root, files, minify).await?;
    rebase_urls(&css, root, assets)
}

/// `@tailwindcss/node` and `@tailwindcss/oxide`, resolved from the app, run the
/// way `@tailwindcss/vite` runs them for a build.
const TAILWIND_SCRIPT: &str = r#"
const { createRequire } = require("node:module");
const path = require("node:path");
(async () => {
  let input = "";
  for await (const chunk of process.stdin) input += chunk;
  const { root, files, minify } = JSON.parse(input);
  const appRequire = createRequire(path.join(root, "package.json"));
  const tw = appRequire("@tailwindcss/node");
  const { Scanner } = appRequire("@tailwindcss/oxide");
  const source = files.map((f) => `@import ${JSON.stringify(f)};`).join("\n");
  const compiler = await tw.compile(source, { base: root, shouldRewriteUrls: true, onDependency() {} });
  const sources = (compiler.root === "none" ? [] : compiler.root === null
    ? [{ base: root, pattern: "**/*", negated: false }]
    : [{ ...compiler.root, negated: false }]).concat(compiler.sources);
  const scanner = new Scanner({ sources });
  let css = compiler.build(scanner.scan());
  if (minify) css = tw.optimize(css, { minify: true }).code;
  process.stdout.write(css);
})().catch((error) => {
  process.stderr.write(String(error && error.stack || error));
  process.exit(1);
});
"#;

/// Invoke the application's installed CSS compiler without coupling the Rust bundler to Bun.
///
/// The script is passed directly to Node, with no shared temporary file. Stderr is returned on failure;
/// successful stdout is CSS only, so diagnostic output must not be written to that stream.
async fn run_tailwind(root: &Path, files: &[String], minify: bool) -> anyhow::Result<String> {
    use tokio::io::AsyncWriteExt;

    let runtime = "node";
    let mut child = tokio::process::Command::new(runtime)
        .arg("--input-type=commonjs")
        .arg("--eval")
        .arg(TAILWIND_SCRIPT)
        .current_dir(root)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot start {runtime}"))?;
    let request = serde_json::json!({ "root": root, "files": files, "minify": minify }).to_string();
    let mut stdin = child.stdin.take().expect("piped stdin");
    stdin.write_all(request.as_bytes()).await?;
    drop(stdin);
    let result = child.wait_with_output().await;
    let result = result?;
    if !result.status.success() {
        bail!(
            "tailwind failed:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    Ok(String::from_utf8(result.stdout)?)
}

/// Rewrites root-relative file `url()`s (Tailwind rebases imports onto the
/// root) to hashed copies under `assets/`. Absolute, data and remote URLs stay.
fn rebase_urls(css: &str, root: &Path, assets: &Path) -> anyhow::Result<String> {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(at) = rest.find("url(") {
        out.push_str(&rest[..at + 4]);
        rest = &rest[at + 4..];
        let Some(end) = rest.find(')') else { break };
        let raw = rest[..end].trim();
        let quote = raw.chars().next().filter(|c| *c == '"' || *c == '\'');
        let value = match quote {
            Some(q) => raw.trim_matches(q),
            None => raw,
        };
        let local = !(value.is_empty()
            || value.starts_with('/')
            || value.starts_with('#')
            || value.starts_with("data:")
            || value.contains("://"));
        let file = root.join(strip_query(value));
        if local && file.is_file() {
            out.push_str(&format!("\"{}\"", emit_asset(&file, assets)?));
        } else {
            out.push_str(&rest[..end]);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    Ok(out)
}

struct ModuleScript {
    src: String,
    /// Byte range of the `src` value in the page.
    src_range: std::ops::Range<usize>,
}

/// `<script type="module" src="...">` tags, in document order.
fn module_scripts(html: &str) -> Vec<ModuleScript> {
    let mut scripts = Vec::new();
    let mut from = 0;
    while let Some(start) = html[from..].find("<script").map(|i| i + from) {
        let Some(end) = html[start..].find('>').map(|i| i + start) else {
            break;
        };
        let tag = &html[start..end];
        from = end;
        if attr(tag, "type").map(|(v, _)| v) != Some("module") {
            continue;
        }
        if let Some((src, range)) = attr(tag, "src") {
            scripts.push(ModuleScript {
                src: src.to_string(),
                src_range: start + range.start..start + range.end,
            });
        }
    }
    scripts
}

/// A quoted attribute's value and its range within `tag`.
fn attr<'a>(tag: &'a str, name: &str) -> Option<(&'a str, std::ops::Range<usize>)> {
    let mut from = 0;
    while let Some(at) = tag[from..].find(name).map(|i| i + from) {
        from = at + name.len();
        let before = tag[..at].chars().last();
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let after = tag[from..].trim_start();
        let Some(after) = after.strip_prefix('=') else {
            continue;
        };
        let after = after.trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            continue;
        }
        let value_start = tag.len() - after.len() + 1;
        let value_end = tag[value_start..].find(quote)? + value_start;
        return Some((&tag[value_start..value_end], value_start..value_end));
    }
    None
}

fn strip_query(id: &str) -> &str {
    id.split(['?', '#']).next().unwrap_or(id)
}

fn short_hash(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:08x}", hasher.finish() as u32)
}

fn copy_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_module_scripts_and_their_src_ranges() {
        let html = r#"<script>x()</script><script type="module" src="/src/main.tsx"></script>"#;
        let scripts = module_scripts(html);
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].src, "/src/main.tsx");
        assert_eq!(&html[scripts[0].src_range.clone()], "/src/main.tsx");
    }

    #[test]
    fn rebases_only_local_urls() {
        let dir = std::env::temp_dir().join(format!("devermind-rebase-{}", std::process::id()));
        let assets = dir.join("out");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(dir.join("src/a.ttf"), b"font").unwrap();
        let css = r#"a{src:url("./src/a.ttf")}b{src:url(/fonts/x.woff2)}c{background:url(data:image/png;base64,AA)}"#;
        let out = rebase_urls(css, &dir, &assets).unwrap();
        assert!(out.contains("url(\"/assets/a-"), "{out}");
        assert!(out.contains("url(/fonts/x.woff2)"));
        assert!(out.contains("url(data:image/png;base64,AA)"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn production_env_is_defined_whole_and_by_key() {
        let defines = build_defines(true);
        assert!(defines.contains(&("import.meta.env.DEV".into(), "false".into())));
        assert!(defines
            .iter()
            .any(|(k, v)| k == "import.meta.env" && v.contains("\"PROD\":true")));
    }

    #[test]
    fn development_env_does_not_select_the_production_react_runtime() {
        let defines = build_defines(false);
        assert!(defines.contains(&("import.meta.env.DEV".into(), "true".into())));
        assert!(defines.contains(&("import.meta.env.MODE".into(), "\"development\"".into())));
        assert!(defines.contains(&("process.env.NODE_ENV".into(), "\"development\"".into())));
    }
}
