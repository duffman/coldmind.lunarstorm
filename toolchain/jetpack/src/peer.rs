//! Peer dependency placement and visibility under Node's `node_modules` lookup rules.
//!
//! A peer must be provided *beside* the consumer or by an ancestor; putting it
//! inside the consumer's own `node_modules` does **not** satisfy a peer edge.
use anyhow::{bail, Context, Result};
use node_semver::{Range, Version};
use std::collections::BTreeMap;

use crate::lockfile::LockedPackage;

/// Return the owning package of a `node_modules` entry, or `None` for a root entry.
/// Supports both scoped and unscoped packages.
pub fn parent_scope(location: &str) -> Option<&str> {
    location
        .rsplit_once("/node_modules/")
        .map(|(parent, _)| parent)
}

/// Produce an installation path for `name` in `scope`'s node_modules directory.
pub fn slot(scope: Option<&str>, name: &str) -> String {
    match scope {
        Some(scope) => format!("{scope}/node_modules/{name}"),
        None => format!("node_modules/{name}"),
    }
}

/// Find the closest peer provider visible from the consumer's parent scope.
/// The consumer's own nested `node_modules` is deliberately not searched.
pub fn provider<'a>(
    packages: &'a BTreeMap<String, LockedPackage>,
    consumer: &str,
    name: &str,
) -> Option<(&'a str, &'a LockedPackage)> {
    provider_in_scope(packages, parent_scope(consumer), name)
}

/// Find a peer starting in `scope` and walking outwards to the project root.
pub fn provider_in_scope<'a>(
    packages: &'a BTreeMap<String, LockedPackage>,
    mut scope: Option<&str>,
    name: &str,
) -> Option<(&'a str, &'a LockedPackage)> {
    loop {
        let key = slot(scope, name);
        if let Some((location, package)) = packages.get_key_value(&key) {
            return Some((location.as_str(), package));
        }
        scope = parent_scope(scope?);
    }
}

/// Find an ordinary dependency, including the consumer's own nested dependencies.
pub fn dependency_provider<'a>(
    packages: &'a BTreeMap<String, LockedPackage>,
    consumer: &str,
    name: &str,
) -> Option<(&'a str, &'a LockedPackage)> {
    provider_in_scope(packages, Some(consumer), name)
}

/// Validate a peer requirement against a selected, semver-formatted version.
/// Non-semver peer specs are rejected rather than silently accepted.
pub fn satisfies(version: &str, requirement: &str) -> Result<bool> {
    let range: Range = requirement
        .parse()
        .with_context(|| format!("invalid peer dependency semver range '{requirement}'"))?;
    let version: Version = version
        .parse()
        .with_context(|| format!("invalid resolved peer version '{version}'"))?;
    Ok(version.satisfies(&range))
}

/// Require that the peer name and its declared semver range are valid.
pub fn validate_requirement(name: &str, requirement: &str) -> Result<()> {
    crate::manifest::validate_name(name)?;
    if requirement.parse::<Range>().is_err() {
        bail!("unsupported peer dependency {name}@{requirement}: npm semver ranges are required");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, version: &str) -> LockedPackage {
        LockedPackage {
            name: name.into(),
            version: version.into(),
            resolved: "https://example.test/a.tgz".into(),
            integrity: "sha512-dummy".into(),
            dependencies: BTreeMap::new(),
            optional_dependencies: BTreeMap::new(),
            peer_dependencies: BTreeMap::new(),
            peer_dependencies_meta: BTreeMap::new(),
            bin: BTreeMap::new(),
            optional: false,
            peer: false,
            os: Vec::new(),
            cpu: Vec::new(),
        }
    }

    #[test]
    fn scoped_and_unscoped_parent_paths() {
        assert_eq!(parent_scope("node_modules/react"), None);
        assert_eq!(parent_scope("node_modules/@scope/plugin"), None);
        assert_eq!(
            parent_scope("node_modules/a/node_modules/@scope/plugin"),
            Some("node_modules/a")
        );
    }

    #[test]
    fn peers_must_be_siblings_or_ancestors_not_children() {
        let mut graph = BTreeMap::new();
        graph.insert(
            "node_modules/app/node_modules/plugin/node_modules/react".into(),
            package("react", "19.0.0"),
        );
        assert!(provider(&graph, "node_modules/app/node_modules/plugin", "react").is_none());
        graph.insert(
            "node_modules/app/node_modules/react".into(),
            package("react", "18.3.0"),
        );
        graph.insert("node_modules/react".into(), package("react", "19.0.0"));
        assert_eq!(
            provider(&graph, "node_modules/app/node_modules/plugin", "react")
                .unwrap()
                .1
                .version,
            "18.3.0"
        );
    }

    #[test]
    fn peer_range_check_handles_semver_and_rejects_tags() {
        assert!(satisfies("18.3.0", "^18 || ^19").unwrap());
        assert!(!satisfies("17.9.0", "^18 || ^19").unwrap());
        assert!(satisfies("1.0.0", "latest").is_err());
    }
}
