//! Data-only project configuration; never starts a JavaScript runtime.
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context};
use serde_json::Value;

use crate::config::ProjectConfig;

/// Reads the native file before compatibility discovery can execute anything.
/// Missing files return `None`; read, parse, and schema errors retain the file path
/// in their error chain and must not cause fallback.
pub(crate) fn load(root: &Path) -> anyhow::Result<Option<ProjectConfig>> {
    let file = root.join(".devtools/config.toml");
    if !file
        .try_exists()
        .with_context(|| format!("checking {}", file.display()))?
    {
        return Ok(None);
    }
    load_file(&file, root).map(Some)
}

pub(crate) fn load_file(file: &Path, root: &Path) -> anyhow::Result<ProjectConfig> {
    let result = (|| {
        let source = std::fs::read_to_string(file)?;
        let value: Value = toml::from_str(&source)?;
        parse(root, &value)
    })();
    result.with_context(|| format!("loading native config {} failed", file.display()))
}

/// Validates the supported schema and normalizes only filesystem-relative aliases.
/// Bare package replacements remain package names; definition strings remain JS
/// expressions rather than being double-quoted as JSON string values.
fn parse(root: &Path, value: &Value) -> anyhow::Result<ProjectConfig> {
    fields(value, "", &["root", "server", "build", "define", "resolve"])?;
    fields(&value["server"], "server.", &["host", "port"])?;
    fields(&value["build"], "build.", &["outDir", "minify"])?;
    fields(&value["resolve"], "resolve.", &["alias"])?;
    let port = &value["server"]["port"];
    let server_port = if port.is_null() {
        None
    } else {
        Some(
            port.as_u64()
                .filter(|port| *port <= u16::MAX as u64)
                .context("server.port must be an integer from 0 to 65535")? as u16,
        )
    };
    let minify = &value["build"]["minify"];
    let build_minify = if minify.is_null() {
        None
    } else {
        Some(minify.as_bool().context("build.minify must be a boolean")?)
    };
    let mut alias = Vec::new();
    if !value["resolve"]["alias"].is_null() {
        let aliases = value["resolve"]["alias"]
            .as_object()
            .context("resolve.alias must be a table")?;
        for (name, replacement) in aliases {
            let replacement = replacement
                .as_str()
                .with_context(|| format!("resolve.alias.{name} must be a string"))?;
            let relative = replacement == "."
                || replacement == ".."
                || replacement.starts_with("./")
                || replacement.starts_with("../");
            let path = if relative {
                let mut path = root.canonicalize()?;
                for component in Path::new(replacement).components() {
                    match component {
                        std::path::Component::CurDir => {}
                        std::path::Component::ParentDir => {
                            path.pop();
                        }
                        component => path.push(component),
                    }
                }
                path.to_string_lossy().into_owned()
            } else {
                replacement.to_string()
            };
            alias.push((name.clone(), path));
        }
    }
    let mut define = Vec::new();
    if !value["define"].is_null() {
        for (name, replacement) in value["define"]
            .as_object()
            .context("define must be a table")?
        {
            define.push((
                name.clone(),
                replacement
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| replacement.to_string()),
            ));
        }
    }
    Ok(ProjectConfig {
        root: text(&value["root"], "root")?.map(|path| {
            if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                root.join(path)
            }
        }),
        define,
        alias,
        server_port,
        server_host: text(&value["server"]["host"], "server.host")?.map(str::to_string),
        build_out_dir: text(&value["build"]["outDir"], "build.outDir")?.map(PathBuf::from),
        build_minify,
    })
}

fn text<'a>(value: &'a Value, field: &str) -> anyhow::Result<Option<&'a str>> {
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(
        value
            .as_str()
            .with_context(|| format!("{field} must be a string"))?,
    ))
}

fn fields(value: &Value, prefix: &str, allowed: &[&str]) -> anyhow::Result<()> {
    if value.is_null() {
        return Ok(());
    }
    ensure!(
        value.is_object(),
        "{} must be a table",
        prefix.trim_end_matches('.')
    );
    for key in value.as_object().unwrap().keys() {
        if !allowed.contains(&key.as_str()) {
            bail!("unknown setting {prefix}{key}");
        }
    }
    Ok(())
}
