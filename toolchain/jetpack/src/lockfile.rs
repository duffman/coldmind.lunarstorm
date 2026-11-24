//! Deterministic, path-addressed, offline-validated jetpack lockfile.
//!
//! V5 records registry routing, root optional dependencies and each pinned package's platform
//! filters. It is valid on every OS and CPU: optional packages for other platforms are kept in
//! the graph and skipped at install. All required dependency and peer edges are validated
//! against installed *versions*, not merely against package names.
use crate::{
    peer,
    registry::{compatible_platform, current_platform, PeerMeta},
};
use anyhow::{bail, Context, Result};
use node_semver::{Range, Version};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::fs::File;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

/// Filename for jetpack's independent (not npm package-lock) lock format.
pub const LOCK_NAME: &str = "rustnpm-lock.json";
/// 5: one graph for every platform (v4 was tied to the resolving host).
pub const LOCK_VERSION: u32 = 5;

/// An npm operating system and architecture pair.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetPlatform {
    /// npm OS identifier (e.g. linux, darwin, win32).
    pub os: String,
    /// npm CPU identifier (e.g. x64, arm64).
    pub cpu: String,
}

impl TargetPlatform {
    /// Current host's npm-compatible OS/CPU identifiers.
    pub fn current() -> Self {
        let (os, cpu) = current_platform();
        Self {
            os: os.into(),
            cpu: cpu.into(),
        }
    }
}

/// Pinned dependency graph and exact root manifest requirements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    /// Schema version. Older versions are re-resolved by `install`, rejected by `ci`.
    #[serde(rename = "lockfileVersion")]
    pub lockfile_version: u32,
    /// Non-optional root dependencies.
    pub dependencies: BTreeMap<String, String>,
    /// Root development dependencies.
    #[serde(rename = "devDependencies")]
    pub dev_dependencies: BTreeMap<String, String>,
    /// Root optional dependencies; overrides entries with the same name.
    #[serde(rename = "optionalDependencies", default)]
    pub optional_dependencies: BTreeMap<String, String>,
    /// Root peer requirements.
    #[serde(default, rename = "peerDependencies")]
    pub peer_dependencies: BTreeMap<String, String>,
    /// Root peer optionality.
    #[serde(default, rename = "peerDependenciesMeta")]
    pub peer_dependencies_meta: BTreeMap<String, PeerMeta>,
    /// Non-secret registry routing used to resolve this graph.
    #[serde(rename = "registrySources", default)]
    pub registry_sources: crate::config::RegistrySources,
    /// Installed package records, indexed by exact relative node_modules path.
    pub packages: BTreeMap<String, LockedPackage>,
}

/// Immutable package metadata required to validate a stored install graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedPackage {
    /// Registry package name (including scope).
    pub name: String,
    /// Exact pinned release.
    pub version: String,
    /// Validated tarball URL.
    pub resolved: String,
    /// Subresource Integrity hash.
    pub integrity: String,
    /// Non-optional child edges.
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    /// Optional child edges; absent nodes are allowed.
    #[serde(default, rename = "optionalDependencies")]
    pub optional_dependencies: BTreeMap<String, String>,
    /// Peer edges, supplied by a sibling or parent.
    #[serde(default, rename = "peerDependencies")]
    pub peer_dependencies: BTreeMap<String, String>,
    /// Peers that may be missing, but must be compatible if installed.
    #[serde(default, rename = "peerDependenciesMeta")]
    pub peer_dependencies_meta: BTreeMap<String, PeerMeta>,
    /// Binary commands and their relative package paths.
    #[serde(default)]
    pub bin: BTreeMap<String, String>,
    /// Package itself can be omitted on failure.
    #[serde(default)]
    pub optional: bool,
    /// Installed automatically to satisfy a peer edge.
    #[serde(default)]
    pub peer: bool,
    /// Compatible npm operating systems, including exclusions.
    #[serde(default)]
    pub os: Vec<String>,
    /// Compatible npm CPU types, including exclusions.
    #[serde(default)]
    pub cpu: Vec<String>,
}

impl Lockfile {
    /// Backwards-compatible constructor for fixtures without root optional dependencies.
    pub fn empty(
        dependencies: BTreeMap<String, String>,
        dev_dependencies: BTreeMap<String, String>,
        peer_dependencies: BTreeMap<String, String>,
        peer_dependencies_meta: BTreeMap<String, PeerMeta>,
    ) -> Self {
        Self::empty_with_optional(
            dependencies,
            dev_dependencies,
            BTreeMap::new(),
            peer_dependencies,
            peer_dependencies_meta,
        )
    }

    /// Create an empty lockfile with exact manifest groups.
    pub fn empty_with_optional(
        dependencies: BTreeMap<String, String>,
        dev_dependencies: BTreeMap<String, String>,
        optional_dependencies: BTreeMap<String, String>,
        peer_dependencies: BTreeMap<String, String>,
        peer_dependencies_meta: BTreeMap<String, PeerMeta>,
    ) -> Self {
        Self {
            lockfile_version: LOCK_VERSION,
            dependencies,
            dev_dependencies,
            optional_dependencies,
            peer_dependencies,
            peer_dependencies_meta,
            registry_sources: crate::config::RegistrySources::default(),
            packages: BTreeMap::new(),
        }
    }

    /// Locations of optional packages that do not support `platform`. Install skips them
    /// and everything nested inside them.
    pub fn skipped_on(&self, platform: &TargetPlatform) -> BTreeSet<String> {
        let mut skipped = BTreeSet::new();
        for (location, package) in &self.packages {
            if package.optional
                && !(compatible_platform(&package.os, &platform.os)
                    && compatible_platform(&package.cpu, &platform.cpu))
            {
                skipped.insert(location.clone());
            }
        }
        skipped
    }

    /// Hoists packages to the highest `node_modules` scope that keeps the graph valid:
    /// a package moves (with its nested packages) to an ancestor slot that is free, or is
    /// dropped when that slot holds the same artifact. Each move must preserve peer
    /// provider bindings and pass [`Lockfile::validate`]. Locations are visited in order.
    pub fn dedupe(&mut self) {
        loop {
            let mut changed = false;
            let locations: Vec<String> = self.packages.keys().cloned().collect();
            for location in locations {
                let Some(package) = self.packages.get(&location).cloned() else {
                    continue;
                };
                let mut owner = peer::parent_scope(&location);
                // Ancestor scopes, highest first.
                let mut scopes = Vec::new();
                while let Some(scope) = owner {
                    // If this optional ancestor is skipped, the installer must
                    // be able to skip its dependencies with it.
                    if self.packages.get(scope).is_some_and(|pkg| pkg.optional) {
                        break;
                    }
                    owner = peer::parent_scope(scope);
                    scopes.push(owner);
                }
                for scope in scopes.into_iter().rev() {
                    let slot = peer::slot(scope, &package.name);
                    let mut candidate = self.packages.clone();
                    let subtree: Vec<String> = candidate
                        .keys()
                        .filter(|key| **key == location || key.starts_with(&format!("{location}/")))
                        .cloned()
                        .collect();
                    let same = candidate.get(&slot).is_some_and(|existing| {
                        same_artifact(existing, &package)
                            && (!existing.optional || package.optional)
                            && existing.os == package.os
                            && existing.cpu == package.cpu
                    });
                    if candidate.contains_key(&slot) && !same {
                        continue;
                    }
                    let moved: Vec<(String, LockedPackage)> = subtree
                        .iter()
                        .map(|key| (key.clone(), candidate.remove(key).expect("subtree key")))
                        .collect();
                    if !same {
                        for (key, record) in moved {
                            candidate.insert(format!("{slot}{}", &key[location.len()..]), record);
                        }
                    }
                    let previous = std::mem::replace(&mut self.packages, candidate);
                    if self.preserves_peer_bindings(&previous, &location, &slot)
                        && self.validate().is_ok()
                    {
                        changed = true;
                        break;
                    }
                    self.packages = previous;
                }
            }
            if !changed {
                return;
            }
        }
    }

    fn preserves_peer_bindings(
        &self,
        previous: &BTreeMap<String, LockedPackage>,
        from: &str,
        to: &str,
    ) -> bool {
        for name in self.peer_dependencies.keys() {
            let slot = peer::slot(None, name);
            if previous.contains_key(&slot) != self.packages.contains_key(&slot) {
                return false;
            }
        }
        for (location, package) in previous {
            if package.peer_dependencies.is_empty() {
                continue;
            }
            let consumer = relocated(location, from, to);
            let Some(replacement) = self.packages.get(consumer.as_ref()) else {
                return false;
            };
            if !same_artifact(package, replacement)
                || package.peer_dependencies != replacement.peer_dependencies
                || package.peer_dependencies_meta != replacement.peer_dependencies_meta
            {
                return false;
            }
            for name in package.peer_dependencies.keys() {
                match (
                    peer::provider(previous, location, name),
                    peer::provider(&self.packages, &consumer, name),
                ) {
                    (None, None) => {}
                    (Some((old_slot, old)), Some((new_slot, new)))
                        if relocated(old_slot, from, to) == new_slot && same_artifact(old, new) => {
                    }
                    _ => return false,
                }
            }
        }
        true
    }

    /// Compare every root dependency group with package.json.
    pub fn matches(
        &self,
        deps: &BTreeMap<String, String>,
        dev: &BTreeMap<String, String>,
        optional: &BTreeMap<String, String>,
        peers: &BTreeMap<String, String>,
        peer_meta: &BTreeMap<String, PeerMeta>,
    ) -> bool {
        self.lockfile_version == LOCK_VERSION
            && &self.dependencies == deps
            && &self.dev_dependencies == dev
            && &self.optional_dependencies == optional
            && &self.peer_dependencies == peers
            && &self.peer_dependencies_meta == peer_meta
    }

    /// Read only a lockfile's version to allow explicit schema migration.
    pub fn read_version(path: &Path) -> Result<u32> {
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(path)?).context("invalid jetpack lockfile")?;
        let raw = value
            .get("lockfileVersion")
            .and_then(|v| v.as_u64())
            .context("lockfileVersion is missing or invalid")?;
        u32::try_from(raw).context("lockfileVersion is out of range")
    }

    /// Load and validate a lock graph without contacting the registry.
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        let lock: Self = serde_json::from_slice(&bytes).context("invalid jetpack lockfile")?;
        lock.validate()?;
        Ok(lock)
    }

    /// Validate and atomically serialize the lockfile.
    pub fn write(&self, path: &Path) -> Result<()> {
        self.validate()?;
        atomic_json_write(path, self)
    }

    /// Validate syntax, layout, all version constraints, platform filters and
    /// reachability. Registry dist-tags remain pinned to exact stored versions;
    /// their *current* registry target cannot be checked offline. A required package
    /// must support this host, unless it is nested in an optional package that is skipped here.
    pub fn validate(&self) -> Result<()> {
        if self.lockfile_version != LOCK_VERSION {
            bail!(
                "unsupported lockfileVersion {} (expected {LOCK_VERSION})",
                self.lockfile_version
            );
        }
        self.registry_sources.validate()?;
        let host = TargetPlatform::current();
        let skipped = self.skipped_on(&host);
        if self.packages.len() > 2_048 {
            bail!("lockfile contains too many packages");
        }
        for name in self.peer_dependencies_meta.keys() {
            if !self.peer_dependencies.contains_key(name) {
                bail!("root peerDependenciesMeta refers to undeclared peer {name}");
            }
        }
        // Validate the package records before evaluating any lookup edge.
        for (location, package) in &self.packages {
            crate::manifest::validate_name(&package.name)?;
            if !valid_location(location, &package.name) {
                bail!("unsafe or mismatched lockfile path: {location}");
            }
            let _: Version = package.version.parse().with_context(|| {
                format!("invalid locked version {} for {location}", package.version)
            })?;
            if !package.optional
                && !(compatible_platform(&package.os, &host.os)
                    && compatible_platform(&package.cpu, &host.cpu))
                && !skipped
                    .iter()
                    .any(|s| location.starts_with(&format!("{s}/")))
            {
                bail!(
                    "required {location} does not support {}/{}",
                    host.os,
                    host.cpu
                );
            }
            crate::registry::validate_url(&package.resolved)?;
            crate::integrity::canonical(Some(&package.integrity), None)
                .with_context(|| format!("invalid integrity for {location}"))?;
            for (bin, path) in &package.bin {
                crate::manifest::validate_name(bin)?;
                crate::install::validate_member_path(path)?;
            }
            for name in package.peer_dependencies_meta.keys() {
                if !package.peer_dependencies.contains_key(name) {
                    bail!("{location} peerDependenciesMeta refers to undeclared peer {name}");
                }
            }
            for name in package
                .dependencies
                .keys()
                .chain(package.optional_dependencies.keys())
            {
                crate::manifest::validate_name(name)?;
            }
        }
        self.validate_edges(&self.packages)?;
        self.validate_reachability()
    }

    /// Check dependency and peer edges against the actual package locations in staging.
    /// Call after validating the lock and processing platform and optional-package skips.
    pub fn validate_installed<'a>(
        &self,
        installed: impl IntoIterator<Item = &'a String>,
    ) -> Result<()> {
        let mut packages = BTreeMap::new();
        for location in installed {
            let package = self
                .packages
                .get(location)
                .with_context(|| format!("staged package is absent from lockfile: {location}"))?;
            packages.insert(location.clone(), package.clone());
        }
        self.validate_edges(&packages)
            .context("staged node_modules does not satisfy the dependency graph")
    }

    fn validate_edges(&self, packages: &BTreeMap<String, LockedPackage>) -> Result<()> {
        // Root optional requirements override ordinary entries of same name.
        // dependencies override devDependencies of the same name.
        let mut root = self.dev_dependencies.clone();
        root.extend(self.dependencies.clone());
        for name in self.optional_dependencies.keys() {
            root.remove(name);
        }
        for (name, spec) in root {
            crate::manifest::validate_name(&name)?;
            let target = packages
                .get(&peer::slot(None, &name))
                .ok_or_else(|| anyhow::anyhow!("missing required root dependency {name}"))?;
            check_range("<root>", &name, &spec, &target.version)?;
        }
        for (name, spec) in &self.optional_dependencies {
            crate::manifest::validate_name(name)?;
            if let Some(pkg) = packages.get(&peer::slot(None, name)) {
                check_range("<root optional>", name, spec, &pkg.version)?;
            }
        }
        for (name, spec) in &self.peer_dependencies {
            peer::validate_requirement(name, spec)?;
            check_peer(
                "<root>",
                name,
                spec,
                self.peer_dependencies_meta.get(name),
                packages.get(&peer::slot(None, name)),
            )?;
        }
        for (location, package) in packages {
            for (name, range) in &package.peer_dependencies {
                peer::validate_requirement(name, range)?;
                if packages.contains_key(&peer::slot(Some(location), name)) {
                    bail!("{location} has peer {name} installed inside its own node_modules");
                }
                let found = peer::provider(packages, location, name).map(|(_, pkg)| pkg);
                check_peer(
                    location,
                    name,
                    range,
                    package.peer_dependencies_meta.get(name),
                    found,
                )?;
            }
            for (name, spec) in &package.dependencies {
                if package.optional_dependencies.contains_key(name) {
                    continue;
                }
                let found =
                    peer::dependency_provider(packages, location, name).ok_or_else(|| {
                        anyhow::anyhow!("missing dependency {name}@{spec} for {location}")
                    })?;
                check_range(location, name, spec, &found.1.version)?;
            }
            for (name, spec) in &package.optional_dependencies {
                if let Some((_, found)) = peer::dependency_provider(packages, location, name) {
                    check_range(location, name, spec, &found.version)?;
                }
            }
        }
        Ok(())
    }

    fn validate_reachability(&self) -> Result<()> {
        // Unreachable records might be stale or malicious: do not extract them.
        let mut reachable = BTreeSet::new();
        let mut pending = VecDeque::new();
        for name in self
            .dependencies
            .keys()
            .chain(self.dev_dependencies.keys())
            .chain(self.optional_dependencies.keys())
            .chain(self.peer_dependencies.keys())
        {
            let location = peer::slot(None, name);
            if self.packages.contains_key(&location) {
                pending.push_back(location);
            }
        }
        while let Some(location) = pending.pop_front() {
            if !reachable.insert(location.clone()) {
                continue;
            }
            let package = self
                .packages
                .get(&location)
                .expect("reachability location is indexed");
            for name in package
                .dependencies
                .keys()
                .chain(package.optional_dependencies.keys())
            {
                if let Some((provider, _)) =
                    peer::dependency_provider(&self.packages, &location, name)
                {
                    pending.push_back(provider.to_owned());
                }
            }
            for name in package.peer_dependencies.keys() {
                if let Some((provider, _)) = peer::provider(&self.packages, &location, name) {
                    pending.push_back(provider.to_owned());
                }
            }
        }
        if let Some(orphan) = self.packages.keys().find(|path| !reachable.contains(*path)) {
            bail!("orphan package in lockfile: {orphan}");
        }
        Ok(())
    }
}

fn relocated<'a>(location: &'a str, from: &str, to: &str) -> Cow<'a, str> {
    match location.strip_prefix(from) {
        Some(suffix) if suffix.is_empty() || suffix.starts_with('/') => {
            Cow::Owned(format!("{to}{suffix}"))
        }
        _ => Cow::Borrowed(location),
    }
}

fn same_artifact(left: &LockedPackage, right: &LockedPackage) -> bool {
    left.name == right.name
        && left.version == right.version
        && left.resolved == right.resolved
        && left.integrity == right.integrity
}

fn check_range(consumer: &str, name: &str, spec: &str, version: &str) -> Result<()> {
    match spec.parse::<Range>() {
        Ok(range) => {
            let parsed: Version = version.parse()?;
            if !parsed.satisfies(&range) {
                bail!("lockfile version conflict: {consumer} needs {name}@{spec}, found {version}");
            }
        }
        Err(_) if is_dist_tag(spec) => (),
        Err(_) => bail!("unsupported locked version spec {name}@{spec} for {consumer}"),
    }
    Ok(())
}

fn is_dist_tag(spec: &str) -> bool {
    !spec.is_empty()
        && spec.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && spec
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn check_peer(
    consumer: &str,
    name: &str,
    range: &str,
    meta: Option<&PeerMeta>,
    provider: Option<&LockedPackage>,
) -> Result<()> {
    match provider {
        Some(pkg) if peer::satisfies(&pkg.version, range)? => Ok(()),
        Some(pkg) => bail!(
            "peer conflict: {consumer} needs {name}@{range}, found {}@{}",
            name,
            pkg.version
        ),
        None if meta.is_some_and(|m| m.optional) => Ok(()),
        None => bail!("missing required peer {name}@{range} for {consumer}"),
    }
}

fn valid_location(location: &str, name: &str) -> bool {
    let mut pieces = location.split('/');
    let last = loop {
        if pieces.next() != Some("node_modules") {
            return false;
        }
        let Some(part) = pieces.next() else {
            return false;
        };
        let current = if part.starts_with('@') {
            let Some(pkg) = pieces.next() else {
                return false;
            };
            format!("{part}/{pkg}")
        } else {
            part.to_owned()
        };
        if crate::manifest::validate_name(&current).is_err() {
            return false;
        }
        if pieces.clone().next().is_none() {
            break current;
        }
    };
    last == name
}

/// Write JSON by syncing a temporary sibling then atomically replacing the
/// destination. The old file is never removed before a successful rename.
pub fn atomic_json_write<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    use std::sync::atomic::Ordering;
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    let filename = path
        .file_name()
        .context("missing output file name")?
        .to_string_lossy();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_file_name(format!("{filename}.tmp.{}.{}", std::process::id(), id));
    let outcome = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)
            .with_context(|| format!("cannot atomically replace {}", path.display()))?;
        #[cfg(unix)]
        {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&temp);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_locks_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(LOCK_NAME);
        std::fs::write(
            &path,
            br#"{"lockfileVersion":2,"dependencies":{},"devDependencies":{},"packages":{}}"#,
        )
        .unwrap();
        assert_eq!(Lockfile::read_version(&path).unwrap(), 2);
        assert!(Lockfile::open(&path).is_err());
    }
    #[test]
    fn only_package_paths_inside_node_modules() {
        assert!(valid_location("node_modules/a", "a"));
        assert!(valid_location("node_modules/@scope/a/node_modules/x", "x"));
        assert!(!valid_location("node_modules/../bad", "bad"));
        assert!(!valid_location("node_modules/a/other/x", "x"));
        assert!(!valid_location("node_modules/a", "b"));
    }
    #[test]
    fn atomic_write_replaces_existing_json() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("saved.json");
        atomic_json_write(&path, &serde_json::json!({"old":true})).unwrap();
        atomic_json_write(&path, &serde_json::json!({"new":true})).unwrap();
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "{\n  \"new\": true\n}\n"
        );
    }
}
