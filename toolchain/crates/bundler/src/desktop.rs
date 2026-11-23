//! Selects the desktop shell from project configuration and generates its React entry.

use std::{fs, path::Path};

use anyhow::Context;
use anyhow::Result;
use serde_json::Value;

pub const DESKTOP_ENTRY: &str = "build/generated/desktop-entry.tsx";
const BUILTIN_SHELL: &str = "@desktop/shell";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildMode {
    Development,
    Debug,
    Production,
}

impl BuildMode {
    pub fn is_development(self) -> bool {
        self != Self::Production
    }

    fn name(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Debug => "debug",
            Self::Production => "production",
        }
    }
}

/// A mode-specific shell wins over `default`; an omitted shell uses the built-in wrapper.
pub fn selected_shell(config: &Value, mode: BuildMode) -> Result<&str> {
    let Some(shell) = config.get("shell") else {
        return Ok(BUILTIN_SHELL);
    };
    match shell {
        Value::String(specifier) => validate_specifier(specifier, "shell"),
        Value::Object(modes) => {
            for (name, value) in modes {
                let known = matches!(
                    name.as_str(),
                    "default" | "development" | "debug" | "production"
                );
                anyhow::ensure!(
                    known,
                    "unknown shell setting {name:?}; expected default, development, debug or production"
                );
                let specifier = value
                    .as_str()
                    .with_context(|| format!("shell.{name} must be a module specifier string"))?;
                validate_specifier(specifier, &format!("shell.{name}"))?;
            }
            Ok(modes
                .get(mode.name())
                .or_else(|| modes.get("default"))
                .and_then(Value::as_str)
                .unwrap_or(BUILTIN_SHELL))
        }
        _ => anyhow::bail!("shell must be a module specifier string or an object of build modes"),
    }
}

fn validate_specifier<'a>(specifier: &'a str, setting: &str) -> Result<&'a str> {
    anyhow::ensure!(
        !specifier.trim().is_empty() && !specifier.chars().any(char::is_control),
        "{setting} must contain a nonempty module specifier without control characters"
    );
    Ok(specifier)
}

/// Prepares the entry before typechecking and bundling. Returns whether its source changed.
pub fn prepare_desktop_frontend(generated: &Path, config: &Value, mode: BuildMode) -> Result<bool> {
    let shell = selected_shell(config, mode)?;
    fs::create_dir_all(generated)?;
    let shell_directory = generated.join("shell");
    fs::create_dir_all(&shell_directory)?;
    for (name, source) in [
        (
            "desktop-shell.tsx",
            include_str!("../../../react/packages/shell/src/desktop-shell.tsx"),
        ),
        (
            "error-boundary.tsx",
            include_str!("../../../react/packages/shell/src/error-boundary.tsx"),
        ),
        (
            "error-store.ts",
            include_str!("../../../react/packages/shell/src/error-store.ts"),
        ),
    ] {
        write_changed(&shell_directory.join(name), source)?;
    }
    let import = if shell == BUILTIN_SHELL {
        "import { ShellWrapper } from './shell/desktop-shell';".to_owned()
    } else {
        let specifier = if let Some(relative) = shell.strip_prefix("./") {
            format!("../../{relative}")
        } else if shell.starts_with("../") {
            format!("../../{shell}")
        } else {
            shell.to_owned()
        };
        format!(
            "import ShellWrapper from {};",
            serde_json::to_string(&specifier)?
        )
    };
    let mut entry = String::new();
    if mode.is_development() {
        write_changed(
            &generated.join("refresh.js"),
            "import Refresh from 'react-refresh/runtime'; Refresh.injectIntoGlobalHook(globalThis); globalThis.__zebraRefresh = Refresh; export default Refresh;\n",
        )?;
        entry.push_str("import './refresh.js';\n");
    }
    entry.push_str(&include_str!("desktop/desktop-entry.tsx").replace("__SHELL_IMPORT__", &import));
    write_changed(&generated.join("desktop-entry.tsx"), &entry)
}

fn write_changed(path: &Path, source: &str) -> Result<bool> {
    if fs::read(path).is_ok_and(|bytes| bytes == source.as_bytes()) {
        return Ok(false);
    }
    fs::write(path, source).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn selects_each_mode_and_its_fallback() -> Result<()> {
        let config = json!({"shell": {
            "default": "@example/base",
            "development": "./shells/dev.tsx",
            "debug": "./shells/debug.tsx",
            "production": "@example/production"
        }});
        assert_eq!(
            selected_shell(&config, BuildMode::Development)?,
            "./shells/dev.tsx"
        );
        assert_eq!(
            selected_shell(&config, BuildMode::Debug)?,
            "./shells/debug.tsx"
        );
        assert_eq!(
            selected_shell(&config, BuildMode::Production)?,
            "@example/production"
        );
        assert_eq!(
            selected_shell(&json!({"shell": {"default": "base"}}), BuildMode::Debug)?,
            "base"
        );
        assert_eq!(
            selected_shell(&json!({"shell": "single"}), BuildMode::Production)?,
            "single"
        );
        assert_eq!(selected_shell(&json!({}), BuildMode::Debug)?, BUILTIN_SHELL);
        assert_eq!(
            selected_shell(&json!({"shell": {}}), BuildMode::Production)?,
            BUILTIN_SHELL
        );
        Ok(())
    }

    #[test]
    fn rejects_malformed_shell_settings() {
        for shell in [
            json!(null),
            json!(false),
            json!(""),
            json!({"debug": 12}),
            json!({"unknown": "x"}),
            json!("bad\nmodule"),
        ] {
            assert!(selected_shell(&json!({"shell": shell}), BuildMode::Production).is_err());
        }
    }

    #[test]
    fn emits_selected_shell_and_tracks_entry_changes() -> Result<()> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("devermind-shell-{}-{nonce}", std::process::id()));
        let generated = directory.join("build/generated");
        let config =
            json!({"shell": {"debug": "./shells/debug.tsx", "production": "@example/prod"}});
        assert!(prepare_desktop_frontend(
            &generated,
            &config,
            BuildMode::Debug
        )?);
        let debug = fs::read_to_string(generated.join("desktop-entry.tsx"))?;
        assert!(debug.contains("import ShellWrapper from \"../../shells/debug.tsx\";"));
        assert!(debug.contains("import './refresh.js';"));
        assert!(debug.contains("<ShellWrapper connection={desktopConnection}"));
        assert!(!prepare_desktop_frontend(
            &generated,
            &config,
            BuildMode::Debug
        )?);
        assert!(prepare_desktop_frontend(
            &generated,
            &config,
            BuildMode::Production
        )?);
        let production = fs::read_to_string(generated.join("desktop-entry.tsx"))?;
        assert!(production.contains("import ShellWrapper from \"@example/prod\";"));
        assert!(!production.contains("import './refresh.js';"));
        assert!(prepare_desktop_frontend(
            &generated,
            &json!({}),
            BuildMode::Development
        )?);
        let builtin = fs::read_to_string(generated.join("desktop-entry.tsx"))?;
        assert!(builtin.contains("import { ShellWrapper } from './shell/desktop-shell';"));
        assert!(generated.join("shell/desktop-shell.tsx").is_file());
        fs::remove_dir_all(directory)?;
        Ok(())
    }
}
