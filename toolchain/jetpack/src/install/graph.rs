//! Preserve the required lock closure while dropping failed optional branches.

use crate::{lockfile::Lockfile, peer};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Surviving<'a> {
    pub locations: BTreeSet<&'a str>,
    pub missing_required: Option<&'a str>,
}

#[derive(Default)]
struct Edges<'a> {
    required: Vec<&'a str>,
    optional: Vec<&'a str>,
}

pub(super) fn surviving<'a>(lock: &'a Lockfile, installed: &BTreeSet<&'a str>) -> Surviving<'a> {
    let mut graph = BTreeMap::new();
    for (location, package) in &lock.packages {
        let mut edges = Edges::default();
        if let Some(parent) = peer::parent_scope(location) {
            edges.required.push(parent);
        }
        for name in package
            .dependencies
            .keys()
            .chain(package.optional_dependencies.keys())
        {
            if let Some((provider, _)) = peer::dependency_provider(&lock.packages, location, name) {
                if package.optional_dependencies.contains_key(name) {
                    edges.optional.push(provider);
                } else {
                    edges.required.push(provider);
                }
            }
        }
        for name in package.peer_dependencies.keys() {
            if let Some((provider, _)) = peer::provider(&lock.packages, location, name) {
                if package
                    .peer_dependencies_meta
                    .get(name)
                    .is_some_and(|meta| meta.optional)
                {
                    edges.optional.push(provider);
                } else {
                    edges.required.push(provider);
                }
            }
        }
        graph.insert(location.as_str(), edges);
    }
    let mut roots = Edges::default();
    for name in lock.dependencies.keys().chain(lock.dev_dependencies.keys()) {
        if !lock.optional_dependencies.contains_key(name) {
            if let Some((path, _)) = lock.packages.get_key_value(&peer::slot(None, name)) {
                roots.required.push(path);
            }
        }
    }
    for name in lock.optional_dependencies.keys() {
        if let Some((path, _)) = lock.packages.get_key_value(&peer::slot(None, name)) {
            roots.optional.push(path);
        }
    }
    for name in lock.peer_dependencies.keys() {
        if let Some((path, _)) = lock.packages.get_key_value(&peer::slot(None, name)) {
            if lock
                .peer_dependencies_meta
                .get(name)
                .is_some_and(|meta| meta.optional)
            {
                roots.optional.push(path);
            } else {
                roots.required.push(path);
            }
        }
    }

    // Propagate unavailability through required edges, including into cycles.
    let mut dependents = BTreeMap::<&str, Vec<&str>>::new();
    for (&consumer, edges) in &graph {
        for &provider in &edges.required {
            dependents.entry(provider).or_default().push(consumer);
        }
    }
    let mut available = installed.clone();
    let mut unavailable: Vec<_> = dependents
        .keys()
        .copied()
        .filter(|provider| !installed.contains(provider))
        .collect();
    while let Some(provider) = unavailable.pop() {
        for &consumer in dependents.get(provider).into_iter().flatten() {
            if available.remove(consumer) {
                unavailable.push(consumer);
            }
        }
    }
    let required = reachable(roots.required.clone(), &graph, |_| true, false);
    let missing_required = required.difference(installed).next().copied();
    let mut candidates = roots.required;
    candidates.extend(roots.optional);
    Surviving {
        locations: reachable(candidates, &graph, |path| available.contains(path), true),
        missing_required,
    }
}

fn reachable<'a>(
    mut pending: Vec<&'a str>,
    graph: &BTreeMap<&'a str, Edges<'a>>,
    allowed: impl Fn(&str) -> bool,
    follow_optional: bool,
) -> BTreeSet<&'a str> {
    let mut seen = BTreeSet::new();
    while let Some(location) = pending.pop() {
        if !allowed(location) || !seen.insert(location) {
            continue;
        }
        if let Some(edges) = graph.get(location) {
            pending.extend(&edges.required);
            if follow_optional {
                pending.extend(&edges.optional);
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn optional_branches_lose_failed_children_or_peers_and_keep_shared_providers() {
        let lock: Lockfile = serde_json::from_value(json!({
            "lockfileVersion": crate::lockfile::LOCK_VERSION,
            "dependencies": {"app": "1.0.0"},
            "devDependencies": {},
            "optionalDependencies": {"addon": "1.0.0", "plugin": "1.0.0"},
            "packages": {
                "node_modules/app": {
                    "name": "app", "version": "1.0.0",
                    "dependencies": {"shared": "1.0.0"},
                    "resolved": "https://fixture.test/app", "integrity": "sha1-AAAAAAAAAAAAAAAAAAAAAAAAAAA="
                },
                "node_modules/addon": {
                    "name": "addon", "version": "1.0.0", "optional": true,
                    "dependencies": {"child": "1.0.0", "shared": "1.0.0"},
                    "resolved": "https://fixture.test/addon", "integrity": "sha1-AAAAAAAAAAAAAAAAAAAAAAAAAAA="
                },
                "node_modules/addon/node_modules/child": {
                    "name": "child", "version": "1.0.0",
                    "resolved": "https://fixture.test/child", "integrity": "sha1-AAAAAAAAAAAAAAAAAAAAAAAAAAA="
                },
                "node_modules/plugin": {
                    "name": "plugin", "version": "1.0.0", "optional": true,
                    "peerDependencies": {"provider": "1.0.0"},
                    "resolved": "https://fixture.test/plugin", "integrity": "sha1-AAAAAAAAAAAAAAAAAAAAAAAAAAA="
                },
                "node_modules/provider": {
                    "name": "provider", "version": "1.0.0", "optional": true,
                    "resolved": "https://fixture.test/provider", "integrity": "sha1-AAAAAAAAAAAAAAAAAAAAAAAAAAA="
                },
                "node_modules/shared": {
                    "name": "shared", "version": "1.0.0",
                    "resolved": "https://fixture.test/shared", "integrity": "sha1-AAAAAAAAAAAAAAAAAAAAAAAAAAA="
                }
            }
        }))
        .unwrap();
        lock.validate().unwrap();
        let installed = BTreeSet::from([
            "node_modules/addon",
            "node_modules/app",
            "node_modules/plugin",
            "node_modules/shared",
        ]);
        let graph = surviving(&lock, &installed);
        assert_eq!(graph.missing_required, None);
        assert_eq!(
            graph.locations,
            BTreeSet::from(["node_modules/app", "node_modules/shared"])
        );
        let installed = BTreeSet::from([
            "node_modules/addon",
            "node_modules/app",
            "node_modules/plugin",
        ]);
        assert_eq!(
            surviving(&lock, &installed).missing_required,
            Some("node_modules/shared")
        );
    }
}
