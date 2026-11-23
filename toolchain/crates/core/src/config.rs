//! Native-first configuration shared by development, production, and embedded hosts.
use std::path::{Path, PathBuf};

/// Validated project settings before host/CLI overrides are applied.
///
/// Paths are resolved relative to the supplied project root, not the hidden config
/// directory. Optional values preserve the distinction between omitted settings and
/// explicit overrides, including `false` and port zero.
#[derive(Debug, Default, Clone)]
pub struct ProjectConfig {
    pub root: Option<PathBuf>,
    pub define: Vec<(String, String)>,
    pub alias: Vec<(String, String)>,
    pub server_port: Option<u16>,
    pub server_host: Option<String>,
    pub build_out_dir: Option<PathBuf>,
    pub build_minify: Option<bool>,
}

/// Loads `.devermind/config.toml`, or the optional Vite adapter only when it is absent.
///
/// Native configuration never executes JavaScript. A present but invalid file is an
/// error, not permission to fall back. Callers own explicit CLI/host override policy.
pub fn load(root: &Path, command: &str, mode: &str) -> anyhow::Result<Option<ProjectConfig>> {
    load_with_preview(root, command, mode, false)
}

/// Resolves preview settings using production mode and the adapter's preview flag.
pub fn load_preview(root: &Path, mode: &str) -> anyhow::Result<Option<ProjectConfig>> {
    load_with_preview(root, "serve", mode, true)
}

fn load_with_preview(
    root: &Path,
    command: &str,
    mode: &str,
    preview: bool,
) -> anyhow::Result<Option<ProjectConfig>> {
    if let Some(config) = crate::toml_config::load(root)? {
        return Ok(Some(config));
    }
    crate::vite_compat::load(root, command, mode, preview)
}

/// Parse an explicitly selected native TOML file, without compatibility discovery.
pub fn load_native(file: &Path, root: &Path) -> anyhow::Result<ProjectConfig> {
    crate::toml_config::load_file(file, root)
}

/// Legacy configuration type name retained for callers of `core::vite`.
pub type ViteConfig = ProjectConfig;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_toml_precedes_vite_and_uses_project_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".devermind")).unwrap();
        std::fs::write(
            dir.path().join("vite.config.js"),
            "throw Error('Vite config must not execute')",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".devermind/config.toml"),
            r#"
root = "client"
[server]
host = "127.0.0.2"
port = 8123
[build]
outDir = "site"
minify = false
[resolve.alias]
"@value" = "./client/value.ts"
[define]
__NAME__ = '"native"'
__ENABLED__ = true
"#,
        )
        .unwrap();
        let config = load(dir.path(), "build", "production").unwrap().unwrap();
        assert_eq!(config.root, Some(dir.path().join("client")));
        assert_eq!(config.server_port, Some(8123));
        assert_eq!(config.server_host.as_deref(), Some("127.0.0.2"));
        assert_eq!(config.build_out_dir, Some("site".into()));
        assert_eq!(config.build_minify, Some(false));
        assert_eq!(
            config.alias,
            vec![(
                "@value".into(),
                dir.path()
                    .canonicalize()
                    .unwrap()
                    .join("client/value.ts")
                    .to_string_lossy()
                    .into_owned()
            )]
        );
        assert!(config
            .define
            .contains(&("__NAME__".into(), "\"native\"".into())));
        assert!(config
            .define
            .contains(&("__ENABLED__".into(), "true".into())));
        assert_eq!(
            load_preview(dir.path(), "production")
                .unwrap()
                .unwrap()
                .server_port,
            Some(8123)
        );
    }

    #[test]
    fn empty_custom_directory_falls_back_but_invalid_toml_does_not() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".devermind")).unwrap();
        assert!(load(dir.path(), "serve", "development").unwrap().is_none());
        std::fs::write(
            dir.path().join("vite.config.js"),
            "export default {server:{port:8123}}",
        )
        .unwrap();
        assert_eq!(
            load(dir.path(), "serve", "development")
                .unwrap()
                .unwrap()
                .server_port,
            Some(8123)
        );
        let file = dir.path().join(".devermind/config.toml");
        for source in [
            "[server",
            "[server]\nport = -1",
            "[server]\nport = 65536",
            "[server]\nport = '5173'",
            "[build]\nminify = 'false'",
            "[server]\nprot = 5173",
            "unknown = true",
            "[resolve.alias]\nfoo = 1",
        ] {
            std::fs::write(&file, source).unwrap();
            let error = load(dir.path(), "serve", "development")
                .unwrap_err()
                .to_string();
            assert!(error.contains(".devermind/config.toml"), "{error}");
        }
    }

    #[test]
    fn native_toml_does_not_need_javascript_runtime() {
        let dir = tempfile::tempdir().unwrap();
        if std::env::var_os("DEVERMIND_NO_RUNTIME_TEST_CHILD").is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "config::tests::native_toml_does_not_need_javascript_runtime",
                ])
                .env("PATH", dir.path())
                .env("DEVERMIND_NO_RUNTIME_TEST_CHILD", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        std::fs::create_dir(dir.path().join(".devermind")).unwrap();
        std::fs::write(
            dir.path().join(".devermind/config.toml"),
            "[server]\nport = 0",
        )
        .unwrap();
        let config = load(dir.path(), "serve", "development").unwrap().unwrap();
        assert_eq!(config.server_port, Some(0));
    }

    #[test]
    fn discovers_all_extensions_and_evaluates_config() {
        for ext in ["js", "mjs", "ts", "cjs", "mts", "cts"] {
            let dir = tempfile::tempdir().unwrap();
            let code = if matches!(ext, "cjs" | "cts") {
                "module.exports = async ({command,mode}) => ({define:{__COMMAND__:JSON.stringify(command),__MODE__:JSON.stringify(mode)},server:{port:0},build:{outDir:'site',minify:false}})"
            } else {
                "export default async ({command,mode}) => ({define:{__COMMAND__:JSON.stringify(command),__MODE__:JSON.stringify(mode)},server:{port:0},build:{outDir:'site',minify:false}})"
            };
            std::fs::write(dir.path().join(format!("vite.config.{ext}")), code).unwrap();
            let config = load(dir.path(), "build", "production").unwrap().unwrap();
            assert!(config
                .define
                .contains(&("__COMMAND__".into(), "\"build\"".into())));
            assert!(config
                .define
                .contains(&("__MODE__".into(), "\"production\"".into())));
            assert_eq!(config.build_out_dir, Some("site".into()));
            assert_eq!(config.build_minify, Some(false));
            assert_eq!(config.server_port, Some(0));
        }
    }
    #[test]
    fn absence_and_malformed_config() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path(), "serve", "development").unwrap().is_none());
        std::fs::write(dir.path().join("vite.config.ts"), "export default {").unwrap();
        assert!(load(dir.path(), "serve", "development")
            .unwrap_err()
            .to_string()
            .contains("vite.config.ts"));
    }
    #[test]
    fn imports_helper_and_resolves_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("helper.ts"),
            "export const port:number=8123",
        )
        .unwrap();
        std::fs::write(dir.path().join("vite.config.ts"), "import {port} from './helper.ts'; export default {root:'client',server:{port},resolve:{alias:{'@':'/src'}}}").unwrap();
        let config = load(dir.path(), "serve", "development").unwrap().unwrap();
        assert_eq!(config.server_port, Some(8123));
        assert_eq!(config.root, Some(dir.path().join("client")));
        assert_eq!(config.alias, vec![("@".into(), "/src".into())]);
    }
    #[test]
    fn precedence_and_preview_environment() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("vite.config.ts"),
            "export default {server:{port:99}}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("vite.config.js"),
            "export default ({isPreview}) => ({server:{port:isPreview ? 81 : 80}})",
        )
        .unwrap();
        assert_eq!(
            load(dir.path(), "serve", "development")
                .unwrap()
                .unwrap()
                .server_port,
            Some(80)
        );
        assert_eq!(
            load_preview(dir.path(), "production")
                .unwrap()
                .unwrap()
                .server_port,
            Some(81)
        );
    }

    #[test]
    fn rejects_relative_alias_and_invalid_exports() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("vite.config.js");
        for alias in [".", "..", "./src", "../src"] {
            std::fs::write(
                &file,
                format!("export default {{resolve:{{alias:{{'@':'{alias}'}}}}}}"),
            )
            .unwrap();
            assert!(load(dir.path(), "serve", "development")
                .unwrap_err()
                .to_string()
                .contains("absolute paths"));
        }
        std::fs::write(&file, "export default 42").unwrap();
        assert!(load(dir.path(), "serve", "development").is_err());
    }
}
