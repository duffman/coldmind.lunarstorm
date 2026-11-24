//! Optional adapter for executable Vite configuration; not the native project contract.
use crate::config::ProjectConfig;
use anyhow::{bail, Context};
use serde_json::Value;
use std::path::{Path, PathBuf};

const LOADER: &str = r#"
import { pathToFileURL } from 'node:url';
const [file, command, mode, preview] = process.argv.slice(1);
let config = (await import(pathToFileURL(file).href)).default;
if (typeof config === 'function') config = await config({command, mode, isSsrBuild:false, isPreview:preview === 'true'});
config = await config;
if (!config || typeof config !== 'object' || Array.isArray(config)) throw Error('Vite config must export an object or a function returning an object');
const supported = new Set(['root','define','resolve','server','build']);
for (const key of Object.keys(config)) if (!supported.has(key)) console.error(`[devtools] Vite option ${key} is not supported; ${key === 'plugins' ? 'Vite plugin hooks are not executed' : 'option ignored'}`);
for (const [key, keys] of Object.entries({resolve:['alias'],server:['port','host'],build:['outDir','minify']})) {
  for (const field of Object.keys(config[key] ?? {})) if (!keys.includes(field)) console.error(`[devtools] Vite option ${key}.${field} is not supported; option ignored`);
}
const alias = Array.isArray(config.resolve?.alias) ? config.resolve.alias : Object.entries(config.resolve?.alias ?? {}).map(([find,replacement])=>({find,replacement}));
for (const entry of alias) if (typeof entry.find !== 'string' || typeof entry.replacement !== 'string') throw Error('Only string Vite resolve.alias entries are supported');
for (const entry of alias) if (/^\.{1,2}(?:[\\/]|$)/.test(entry.replacement)) throw Error('Vite filesystem alias replacements must be absolute paths, not relative paths');
const define = Object.entries(config.define ?? {}).map(([k,v])=>[k,typeof v === 'string' ? v : JSON.stringify(v)]);
for (const [key,value] of [['root',config.root],['build.outDir',config.build?.outDir]]) if (value !== undefined && typeof value !== 'string') throw Error(`${key} must be a string`);
const port = config.server?.port;
if (port !== undefined && (!Number.isInteger(port) || port < 0 || port > 65535)) throw Error('server.port must be a valid port');
const host = config.server?.host;
if (host !== undefined && typeof host !== 'string' && typeof host !== 'boolean') throw Error('server.host must be a string or boolean');
const minify = config.build?.minify;
if (typeof minify === 'string') console.error('[devtools] build.minify selects enable/disable only; requested JavaScript minifier backend is not used');
if (minify !== undefined && ![true,false,'esbuild','terser','oxc'].includes(minify)) throw Error('Unsupported build.minify value');
console.log('__DEVTOOLS_CONFIG__' + JSON.stringify({root:config.root,define,alias:alias.map(a=>[a.find,a.replacement]),server_port:port,server_host:host === true ? '0.0.0.0' : host === false ? '127.0.0.1' : host,build_out_dir:config.build?.outDir,build_minify:minify === undefined ? undefined : minify !== false}));
"#;

/// Evaluates only the selected fallback file, with Bun isolated from automatic env loading.
/// Evaluation and schema errors propagate; no missing or invalid result becomes defaults.
pub(crate) fn load(
    root: &Path,
    command: &str,
    mode: &str,
    preview: bool,
) -> anyhow::Result<Option<ProjectConfig>> {
    let file = ["js", "mjs", "ts", "cjs", "mts", "cts"]
        .iter()
        .map(|ext| root.join(format!("vite.config.{ext}")))
        .find(|p| p.is_file());
    let Some(file) = file else { return Ok(None) };
    let file = file.canonicalize()?;
    let output = std::process::Command::new("bun")
        .args(["--no-env-file", "--eval", LOADER])
        .arg(&file)
        .arg(command)
        .arg(mode)
        .arg(preview.to_string())
        .current_dir(root)
        .output()
        .with_context(|| format!("loading {} requires Bun on PATH", file.display()))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!("loading Vite config {} failed: {stderr}", file.display());
    }
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = stdout
        .lines()
        .rev()
        .find_map(|s| s.strip_prefix("__DEVTOOLS_CONFIG__"))
        .context("Vite config loader returned no configuration")?;
    let value: Value = serde_json::from_str(json)?;
    let path = |key: &str| value[key].as_str().map(PathBuf::from);
    let pairs = |key: &str| -> anyhow::Result<Vec<(String, String)>> {
        Ok(serde_json::from_value(value[key].clone())?)
    };
    Ok(Some(ProjectConfig {
        root: path("root").map(|p| if p.is_absolute() { p } else { root.join(p) }),
        define: pairs("define")?,
        alias: pairs("alias")?,
        server_port: value["server_port"].as_u64().map(|v| v as u16),
        server_host: value["server_host"].as_str().map(str::to_string),
        build_out_dir: path("build_out_dir"),
        build_minify: value["build_minify"].as_bool(),
    }))
}
