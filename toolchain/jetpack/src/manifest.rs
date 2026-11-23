//! Read and update package.json without losing unrelated fields.
use crate::registry::PeerMeta;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, fs, path::Path};

/// In-memory `package.json` preserving unrelated user-defined JSON fields.
#[derive(Clone)]
pub struct Manifest {
    value: Value,
}

impl Manifest {
    /// The manifest `add` starts in a directory that has none yet.
    ///
    /// Fix: this was an empty object, and the runtime refused the project `add` had just
    /// created ("package.json requires name"). Decision: the started manifest is one the
    /// runtime runs as it is: a name from the directory (lowercased, characters a package
    /// name cannot hold become `-`; `app` when nothing usable is left), version `0.0.0`,
    /// and `"type": "module"`, since a new jetbunny program is ES modules.
    ///
    /// # Arguments
    /// * `directory` - The project directory.
    ///
    /// # Returns
    /// The manifest, not yet written.
    pub fn started(directory: &Path) -> Self {
        let name: String = directory
            .canonicalize()
            .ok()
            .as_deref()
            .unwrap_or(directory)
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let name = name.trim_matches(['-', '_']);
        let mut fields = Map::new();
        fields.insert(
            "name".into(),
            if name.is_empty() { "app" } else { name }.into(),
        );
        fields.insert("version".into(), "0.0.0".into());
        fields.insert("type".into(), "module".into());
        Self {
            value: Value::Object(fields),
        }
    }

    /// Read and validate the package manifest, rejecting unsupported workspaces.
    pub fn open(path: &Path) -> Result<Self> {
        let content =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        let value: Value = serde_json::from_str(&content).context("invalid package.json")?;
        if !value.is_object() {
            bail!("package.json must contain a JSON object");
        }
        if value.get("workspaces").is_some() {
            bail!("npm workspaces are not yet supported");
        }
        Ok(Self { value })
    }

    /// Extract a named dependency group (`dependencies`, `devDependencies` or peers).
    pub fn dependencies(&self, field: &str) -> Result<BTreeMap<String, String>> {
        let Some(value) = self.value.get(field) else {
            return Ok(BTreeMap::new());
        };
        let object = value
            .as_object()
            .ok_or_else(|| anyhow!("{field} must be an object"))?;
        object
            .iter()
            .map(|(name, spec)| {
                validate_name(name)?;
                let spec = spec
                    .as_str()
                    .ok_or_else(|| anyhow!("dependency {name} must have a string version"))?;
                Ok((name.clone(), spec.to_string()))
            })
            .collect()
    }

    /// Parse npm's optional peer metadata, rejecting invalid flags and dangling entries.
    pub fn peer_metadata(&self) -> Result<BTreeMap<String, PeerMeta>> {
        let peers = self.dependencies("peerDependencies")?;
        let Some(meta) = self.value.get("peerDependenciesMeta") else {
            return Ok(BTreeMap::new());
        };
        let map = meta
            .as_object()
            .context("peerDependenciesMeta must be an object")?;
        let mut result = BTreeMap::new();
        for (name, entry) in map {
            if !peers.contains_key(name) {
                bail!("peerDependenciesMeta references undeclared peer {name}");
            }
            let parsed: PeerMeta = serde_json::from_value(entry.clone())
                .with_context(|| format!("invalid peerDependenciesMeta entry for {name}"))?;
            result.insert(name.to_owned(), parsed);
        }
        Ok(result)
    }

    /// Set one dependency range in a specific manifest section.
    pub fn set_dependency(&mut self, field: &str, name: &str, spec: &str) -> Result<()> {
        validate_name(name)?;
        let root = self.value.as_object_mut().expect("validated at load");
        if !root.contains_key(field) {
            root.insert(field.to_owned(), Value::Object(Map::new()));
        }
        root.get_mut(field)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| anyhow!("{field} must be an object"))?
            .insert(name.to_owned(), Value::String(spec.to_owned()));
        Ok(())
    }

    /// Remove a direct package from production, development and optional dependencies.
    pub fn remove_dependency(&mut self, name: &str) -> Result<()> {
        validate_name(name)?;
        let root = self.value.as_object_mut().expect("validated at load");
        for field in ["dependencies", "devDependencies", "optionalDependencies"] {
            if let Some(group) = root.get_mut(field) {
                group
                    .as_object_mut()
                    .ok_or_else(|| anyhow!("{field} must be an object"))?
                    .remove(name);
            }
        }
        Ok(())
    }

    /// Atomically rewrite the manifest while retaining unrelated fields.
    pub fn write(&self, path: &Path) -> Result<()> {
        crate::lockfile::atomic_json_write(path, &self.value)
    }
}

/// Reject path traversal and names that cannot safely map to node_modules paths.
pub fn validate_name(name: &str) -> Result<()> {
    let valid_part = |s: &str| -> bool {
        !s.is_empty()
            && s != "."
            && s != ".."
            && !s.starts_with('.')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '~'))
    };
    let valid = if let Some(scoped) = name.strip_prefix('@') {
        let mut parts = scoped.split('/');
        matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None) if valid_part(a) && valid_part(b))
    } else {
        valid_part(name)
    };
    if !valid {
        bail!("invalid or unsafe npm package name: {name}");
    }
    Ok(())
}

/// Parse package names with scopes and optional version ranges.
/// Parse CLI syntax such as `@scope/pkg@^2` without mistaking the scope for a version.
pub fn parse_package_spec(input: &str) -> Result<(String, Option<String>)> {
    let separator = input.rfind('@').filter(|&i| i > 0);
    let (name, spec) = match separator {
        Some(i) => (&input[..i], Some(&input[i + 1..])),
        None => (input, None),
    };
    validate_name(name)?;
    if matches!(spec, Some("")) {
        bail!("empty version after @ in {input}");
    }
    Ok((name.to_owned(), spec.map(str::to_owned)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scoped_names_and_specs() {
        assert_eq!(
            parse_package_spec("@scope/tool@^2").unwrap(),
            ("@scope/tool".into(), Some("^2".into()))
        );
        assert_eq!(
            parse_package_spec("lodash").unwrap(),
            ("lodash".into(), None)
        );
        assert_eq!(
            parse_package_spec("lodash@~4.1").unwrap().1,
            Some("~4.1".into())
        );
    }

    #[test]
    fn rejects_traversal() {
        for name in ["../oops", "@scope/../oops", "a/b", "", ".bin"] {
            assert!(validate_name(name).is_err(), "accepted {name:?}");
        }
    }

    #[test]
    fn removing_a_package_clears_optional_root_requirements() {
        let mut manifest = Manifest {
            value: serde_json::json!({
                "dependencies": {"addon":"^1"},
                "optionalDependencies": {"addon":"^2"}
            }),
        };
        manifest.remove_dependency("addon").unwrap();
        assert!(manifest.dependencies("dependencies").unwrap().is_empty());
        assert!(manifest
            .dependencies("optionalDependencies")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn preserves_unrelated_json_fields() {
        let mut manifest = Manifest {
            value: serde_json::json!({"name":"app","custom":{"a":7}}),
        };
        manifest
            .set_dependency("dependencies", "react", "^19")
            .unwrap();
        assert_eq!(manifest.value["custom"]["a"], 7);
        manifest.remove_dependency("react").unwrap();
        assert!(manifest.dependencies("dependencies").unwrap().is_empty());
    }
}
