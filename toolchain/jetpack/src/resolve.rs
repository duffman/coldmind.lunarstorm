//! Deterministic nested dependency resolver with peer dependency placement.
//!
//! Ordinary dependencies are first placed in each consumer's `node_modules` directory.
//! Required peers are instead placed alongside their consumers, where the module loader
//! finds them. All immediate siblings contribute constraints before nested scopes are
//! processed. Conflicting requirements return an error, not warnings. The finished graph
//! is deduplicated by hoisting ([`Lockfile::dedupe`]).
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::{
    integrity,
    lockfile::{LockedPackage, Lockfile},
    manifest::Manifest,
    peer,
    registry::{
        self, compatible_platform, current_platform, Dist, PackageMetadata, Registry, VersionMeta,
    },
};

const MAX_DEPTH: usize = 48;
const MAX_PACKAGES: usize = 2_048;
const MAX_SCOPE_ATTEMPTS: usize = 256;
/// Scope attempts across the whole resolution. Per-scope retries nest, so without a global
/// budget a deep graph of conflicts could multiply attempts per level.
const MAX_TOTAL_ATTEMPTS: usize = 20_000;

#[derive(Clone, Debug)]
struct Requirement {
    spec: String,
    optional: bool,
}

#[derive(Clone, Debug)]
struct PeerDemand {
    consumer: String,
    consumer_name: Option<String>,
    consumer_optional: bool,
    spec: String,
    optional: bool,
    platforms: Option<PlatformDomain>,
    narrowable: bool,
}

/// The operating systems and CPU architectures on which an optional ancestry
/// can be installed. A dimension is either a nonempty allowlist or a denylist.
#[derive(Clone, Debug, Default)]
struct PlatformDomain {
    os: Vec<String>,
    cpu: Vec<String>,
}

impl PlatformDomain {
    fn intersect(&self, os: &[String], cpu: &[String]) -> Option<Self> {
        Some(Self {
            os: intersect_axis(&self.os, os)?,
            cpu: intersect_axis(&self.cpu, cpu)?,
        })
    }

    fn covered_by(&self, os: &[String], cpu: &[String]) -> bool {
        axis_covered_by(&self.os, os) && axis_covered_by(&self.cpu, cpu)
    }

    fn union(&self, other: &Self) -> Self {
        // Lockfile filters represent independent OS and CPU dimensions.
        Self {
            os: union_axis(&self.os, &other.os),
            cpu: union_axis(&self.cpu, &other.cpu),
        }
    }
}

fn allowlist(values: &[String]) -> bool {
    values.iter().any(|value| !value.starts_with('!'))
}

fn intersect_axis(left: &[String], right: &[String]) -> Option<Vec<String>> {
    let explicit = if allowlist(left) { left } else { right };
    if allowlist(explicit) {
        let allowed: BTreeSet<_> = explicit
            .iter()
            .filter(|value| {
                !value.starts_with('!')
                    && compatible_platform(left, value)
                    && compatible_platform(right, value)
            })
            .cloned()
            .collect();
        return (!allowed.is_empty()).then(|| allowed.into_iter().collect());
    }
    Some(
        left.iter()
            .chain(right)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    )
}

fn union_axis(left: &[String], right: &[String]) -> Vec<String> {
    match (allowlist(left), allowlist(right)) {
        (true, true) => left
            .iter()
            .chain(right)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        (false, false) => left
            .iter()
            .filter(|value| right.contains(value))
            .cloned()
            .collect(),
        (true, false) => union_axis(right, left),
        (false, true) => left
            .iter()
            .filter(|value| !right.iter().any(|allowed| allowed == &value[1..]))
            .cloned()
            .collect(),
    }
}

fn axis_covered_by(domain: &[String], candidate: &[String]) -> bool {
    if domain.first().is_some_and(|value| !value.starts_with('!')) {
        domain
            .iter()
            .all(|value| compatible_platform(candidate, value))
    } else {
        candidate
            .iter()
            .all(|value| value.starts_with('!') && domain.contains(value))
    }
}

struct ScopeChoices<'a> {
    excluded: &'a BTreeSet<String>,
    versions: &'a BTreeMap<String, Vec<VersionMeta>>,
    indices: &'a BTreeMap<String, usize>,
    peer_indices: &'a BTreeMap<String, usize>,
}

#[derive(Debug)]
struct PeerConflict {
    owner: Option<String>,
    names: BTreeSet<String>,
    message: String,
}

impl PeerConflict {
    fn new(owner: Option<&str>, name: &str, consumers: &[PeerDemand], message: String) -> Self {
        let mut names = BTreeSet::from([name.to_owned()]);
        names.extend(
            consumers
                .iter()
                .filter_map(|demand| demand.consumer_name.clone()),
        );
        Self {
            owner: owner.map(str::to_owned),
            names,
            message,
        }
    }
}

impl std::fmt::Display for PeerConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PeerConflict {}

/// Try changes suggested by a conflict before resuming the ordinary version order.
/// Only choices with alternatives need to be represented in a selection.
struct CandidateSearch<'a> {
    choices: Vec<(&'a str, usize)>,
    ordinary: Option<Vec<usize>>,
    preferred: VecDeque<Vec<usize>>,
    seen: BTreeSet<Vec<usize>>,
}

impl<'a> CandidateSearch<'a> {
    fn new(versions: &'a BTreeMap<String, Vec<VersionMeta>>, excluded: &BTreeSet<String>) -> Self {
        let choices: Vec<_> = versions
            .iter()
            .filter(|(name, versions)| versions.len() > 1 && !excluded.contains(*name))
            .map(|(name, versions)| (name.as_str(), versions.len()))
            .collect();
        Self {
            ordinary: Some(vec![0; choices.len()]),
            choices,
            preferred: VecDeque::new(),
            seen: BTreeSet::new(),
        }
    }

    fn indices(&self, selection: &[usize]) -> BTreeMap<String, usize> {
        self.choices
            .iter()
            .zip(selection)
            .filter(|(_, index)| **index != 0)
            .map(|((name, _), index)| ((*name).to_owned(), *index))
            .collect()
    }

    fn prioritize(&mut self, selection: &[usize], conflict: &PeerConflict) {
        for (position, (name, count)) in self.choices.iter().enumerate().rev() {
            if self.preferred.len() >= MAX_SCOPE_ATTEMPTS {
                break;
            }
            if conflict.names.contains(*name) && selection[position] + 1 < *count {
                let mut alternative = selection.to_vec();
                alternative[position] += 1;
                if self.seen.insert(alternative.clone()) {
                    self.preferred.push_back(alternative);
                }
            }
        }
    }

    fn next(&mut self) -> Option<Vec<usize>> {
        if let Some(selection) = self.preferred.pop_front() {
            return Some(selection);
        }
        while let Some(selection) = self.ordinary.take() {
            let mut following = selection.clone();
            for (index, (_, count)) in following.iter_mut().zip(&self.choices).rev() {
                *index += 1;
                if *index < *count {
                    self.ordinary = Some(following);
                    break;
                }
                *index = 0;
            }
            if self.seen.insert(selection.clone()) {
                return Some(selection);
            }
        }
        None
    }
}

/// Resolve the root manifest into a fully pinned, peer-validated lock graph.
///
/// The resolver installs required peer dependencies automatically, honours
/// `peerDependenciesMeta.optional`, and rejects incompatible host versions.
/// Version conflicts across independent nested subtrees are isolated by scope.
/// It intentionally does not implement npm Arborist's hoisting/backtracking.
pub fn resolve(manifest: &Manifest, registry: &mut Registry) -> Result<Lockfile> {
    let dependencies = manifest.dependencies("dependencies")?;
    let dev_dependencies = manifest.dependencies("devDependencies")?;
    let optional_dependencies = manifest.dependencies("optionalDependencies")?;
    let peer_dependencies = manifest.dependencies("peerDependencies")?;
    let peer_metadata = manifest.peer_metadata()?;
    warn_duplicates(&dependencies, &dev_dependencies, &optional_dependencies);
    for (name, spec) in &peer_dependencies {
        peer::validate_requirement(name, spec)?;
    }

    resolve_with_lock_parts(
        dependencies,
        dev_dependencies,
        optional_dependencies,
        peer_dependencies,
        peer_metadata,
        registry,
        None,
    )
}

/// Resolve against an existing lockfile, retaining compatible pinned versions.
/// Used by `add`/`remove` to avoid unrelated upgrades. A pinned candidate is
/// replaced only when it no longer satisfies the new dependency graph.
pub fn resolve_with_lock(
    manifest: &Manifest,
    registry: &mut Registry,
    previous: Option<&Lockfile>,
) -> Result<Lockfile> {
    let dependencies = manifest.dependencies("dependencies")?;
    let dev = manifest.dependencies("devDependencies")?;
    let optional = manifest.dependencies("optionalDependencies")?;
    let peers = manifest.dependencies("peerDependencies")?;
    let peer_meta = manifest.peer_metadata()?;
    warn_duplicates(&dependencies, &dev, &optional);
    resolve_with_lock_parts(
        dependencies,
        dev,
        optional,
        peers,
        peer_meta,
        registry,
        previous,
    )
}

/// A name in both `dependencies` and `devDependencies` resolves to the `dependencies` entry.
fn warn_duplicates(
    dependencies: &BTreeMap<String, String>,
    dev: &BTreeMap<String, String>,
    optional: &BTreeMap<String, String>,
) {
    for name in dependencies.keys() {
        if dev.contains_key(name) && !optional.contains_key(name) {
            eprintln!(
                "warning: {name} is in both dependencies and devDependencies; using dependencies"
            );
        }
    }
}

fn resolve_with_lock_parts(
    dependencies: BTreeMap<String, String>,
    dev_dependencies: BTreeMap<String, String>,
    optional_dependencies: BTreeMap<String, String>,
    peer_dependencies: BTreeMap<String, String>,
    peer_metadata: BTreeMap<String, crate::registry::PeerMeta>,
    registry: &mut Registry,
    previous: Option<&Lockfile>,
) -> Result<Lockfile> {
    for (name, spec) in &peer_dependencies {
        peer::validate_requirement(name, spec)?;
    }
    let mut requirements = BTreeMap::new();
    // dependencies override devDependencies of the same name.
    for (name, spec) in dev_dependencies.iter().chain(dependencies.iter()) {
        requirements.insert(
            name.clone(),
            Requirement {
                spec: spec.clone(),
                optional: false,
            },
        );
    }
    // In npm, an optional dependency overrides the same name in dependencies.
    for (name, spec) in &optional_dependencies {
        requirements.insert(
            name.clone(),
            Requirement {
                spec: spec.clone(),
                optional: true,
            },
        );
    }
    let sources = registry.sources().clone();
    let previous = previous.filter(|lock| lock.registry_sources == sources);
    if let Some(previous) = previous {
        previous
            .validate()
            .context("cannot reuse an invalid previous lockfile")?;
    }
    let pins = previous
        .map(|lock| lock.packages.clone())
        .unwrap_or_default();
    let preferred = previous
        .map(|lock| {
            lock.packages
                .iter()
                .map(|(location, pkg)| (location.clone(), pkg.version.clone()))
                .collect()
        })
        .unwrap_or_default();
    // Deduplication moves packages, so a pin is also looked up by name.
    let mut preferred_by_name = BTreeMap::<String, BTreeSet<String>>::new();
    for package in previous.iter().flat_map(|lock| lock.packages.values()) {
        preferred_by_name
            .entry(package.name.clone())
            .or_default()
            .insert(package.version.clone());
    }
    let mut lock = Lockfile::empty_with_optional(
        dependencies,
        dev_dependencies,
        optional_dependencies,
        peer_dependencies,
        peer_metadata,
    );
    lock.registry_sources = sources;
    let mut builder = Builder {
        registry,
        lock,
        preferred,
        preferred_by_name,
        pins,
        attempts: 0,
    };
    builder.resolve_scope(None, &requirements, &[], 0)?;
    builder.lock.validate()?;
    builder.lock.dedupe();
    Ok(builder.lock)
}

struct Builder<'a> {
    registry: &'a mut Registry,
    lock: Lockfile,
    preferred: BTreeMap<String, String>,
    preferred_by_name: BTreeMap<String, BTreeSet<String>>,
    pins: BTreeMap<String, LockedPackage>,
    attempts: usize,
}

impl Builder<'_> {
    /// Reuse the complete metadata attached to a locked artifact. Resolving a
    /// different root requirement must not silently reinterpret unchanged bytes.
    fn metadata(&mut self, name: &str) -> Result<PackageMetadata> {
        let mut metadata = self.registry.metadata(name)?;
        let mut retained = BTreeMap::<String, LockedPackage>::new();
        for pin in self.pins.values().filter(|pin| pin.name == name) {
            if let Some(existing) = retained.get_mut(&pin.version) {
                if existing.resolved != pin.resolved
                    || existing.integrity != pin.integrity
                    || existing.dependencies != pin.dependencies
                    || existing.optional_dependencies != pin.optional_dependencies
                    || existing.peer_dependencies != pin.peer_dependencies
                    || existing.peer_dependencies_meta != pin.peer_dependencies_meta
                    || existing.bin != pin.bin
                {
                    bail!(
                        "ambiguous artifact identity for locked {name}@{}",
                        pin.version
                    );
                }
                // A conditional copy carries its branch's eligibility. The same
                // artifact reused by multiple branches must retain their union.
                existing.os = union_axis(&existing.os, &pin.os);
                existing.cpu = union_axis(&existing.cpu, &pin.cpu);
            } else {
                retained.insert(pin.version.clone(), pin.clone());
            }
        }
        for (version, pin) in retained {
            if let Some(current) = metadata.versions.get(&version) {
                let integrity = integrity::canonical(
                    current.dist.integrity.as_deref(),
                    current.dist.shasum.as_deref(),
                );
                let mut dependencies = current.dependencies.clone();
                for optional in current.optional_dependencies.keys() {
                    dependencies.remove(optional);
                }
                if current.dist.tarball != pin.resolved
                    || integrity.as_ref().ok() != Some(&pin.integrity)
                    || dependencies != pin.dependencies
                    || current.optional_dependencies != pin.optional_dependencies
                    || current.peer_dependencies != pin.peer_dependencies
                    || current.peer_dependencies_meta != pin.peer_dependencies_meta
                    || parse_bins(name, &current.bin).as_ref().ok() != Some(&pin.bin)
                {
                    eprintln!("warning: registry metadata changed for {name}@{version}; retaining the locked artifact and dependency declarations");
                }
            }
            metadata.versions.insert(
                version,
                VersionMeta {
                    version: pin.version.clone(),
                    dist: Dist {
                        tarball: pin.resolved.clone(),
                        integrity: Some(pin.integrity.clone()),
                        shasum: None,
                    },
                    dependencies: pin.dependencies.clone(),
                    optional_dependencies: pin.optional_dependencies.clone(),
                    peer_dependencies: pin.peer_dependencies.clone(),
                    peer_dependencies_meta: pin.peer_dependencies_meta.clone(),
                    bin: serde_json::to_value(&pin.bin)?,
                    os: pin.os.clone(),
                    cpu: pin.cpu.clone(),
                },
            );
        }
        Ok(metadata)
    }

    /// Enumerate scope-local version choices and retry failed constraints. The
    /// search is bounded, deterministic, and restores the full graph on failure.
    /// This handles compatible alternative releases without Arborist hoisting.
    fn resolve_scope(
        &mut self,
        owner: Option<&str>,
        requirements: &BTreeMap<String, Requirement>,
        ancestors: &[(String, String)],
        depth: usize,
    ) -> Result<()> {
        if depth >= MAX_DEPTH {
            bail!("dependency graph exceeds {MAX_DEPTH} nested scopes");
        }
        let before = self.lock.packages.clone();
        let mut excluded = BTreeSet::new();
        let mut choices = BTreeMap::<String, Vec<VersionMeta>>::new();
        let mut peer_indices = BTreeMap::<String, usize>::new();
        let platforms = self.conditional_platforms(owner)?;
        let can_narrow = platforms.is_some() && self.narrowable_optional_ancestor(owner).is_some();
        // Populate candidates once per scope and prefer versions from the old lock.
        self.registry
            .prefetch(requirements.keys().map(String::as_str));
        for (name, requirement) in requirements {
            let location = peer::slot(owner, name);
            // Optional packages are pinned for every platform and skipped at install.
            let found = self.metadata(name).and_then(|metadata| {
                let (host_os, host_cpu) = current_platform();
                registry::matching_candidates(
                    &metadata,
                    name,
                    &requirement.spec,
                    |pkg| {
                        if let Some(platforms) = &platforms {
                            if requirement.optional || can_narrow {
                                platforms.intersect(&pkg.os, &pkg.cpu).is_some()
                            } else {
                                platforms.covered_by(&pkg.os, &pkg.cpu)
                            }
                        } else {
                            requirement.optional
                                || (compatible_platform(&pkg.os, host_os)
                                    && compatible_platform(&pkg.cpu, host_cpu))
                        }
                    },
                    "the dependency branch's targets",
                )
            });
            match found {
                Ok(mut versions) => {
                    if can_narrow && !requirement.optional {
                        let domain = platforms.as_ref().expect("optional platform domain");
                        // Prefer releases that keep the whole branch portable before
                        // trying ones that narrow its eligibility.
                        versions.sort_by_key(|pkg| !domain.covered_by(&pkg.os, &pkg.cpu));
                    }
                    let pinned = versions
                        .iter()
                        .position(|pkg| self.preferred.get(&location) == Some(&pkg.version))
                        .or_else(|| {
                            let known = self.preferred_by_name.get(name)?;
                            versions.iter().position(|pkg| known.contains(&pkg.version))
                        });
                    if let Some(index) = pinned {
                        let selected = versions.remove(index);
                        versions.insert(0, selected);
                    }
                    choices.insert(name.clone(), versions);
                }
                Err(error) if requirement.optional => {
                    eprintln!("warning: skipping optional dependency {name}: {error:#}");
                    excluded.insert(name.clone());
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("resolving {name}@{}", requirement.spec))
                }
            }
        }
        let mut search = CandidateSearch::new(&choices, &excluded);
        let mut selection = search.next().expect("initial candidate selection");
        let mut indices = search.indices(&selection);
        let mut last_error = None;
        // A later required-candidate failure must not discard an optional
        // exclusion discovered by an earlier combination.
        let mut optional_to_drop = None;
        for _ in 0..MAX_SCOPE_ATTEMPTS {
            self.attempts += 1;
            if self.attempts > MAX_TOTAL_ATTEMPTS {
                self.lock.packages = before;
                bail!("dependency resolution exceeded {MAX_TOTAL_ATTEMPTS} attempts");
            }
            self.lock.packages = before.clone();
            match self.resolve_scope_once(
                owner,
                requirements,
                ancestors,
                depth,
                ScopeChoices {
                    excluded: &excluded,
                    versions: &choices,
                    indices: &indices,
                    peer_indices: &peer_indices,
                },
            ) {
                Ok(None) => return Ok(()),
                Ok(Some(name)) if requirements.get(&name).is_some_and(|r| r.optional) => {
                    optional_to_drop = Some(name);
                    last_error = None;
                }
                Ok(Some(name)) => {
                    last_error = Some(anyhow!(
                        "optional resolver attempted to drop required package {name}"
                    ));
                }
                Err(error) => {
                    if let Some(conflict) = error
                        .downcast_ref::<PeerConflict>()
                        .filter(|conflict| conflict.owner.as_deref() == owner)
                    {
                        search.prioritize(&selection, conflict);
                    }
                    last_error = Some(error);
                }
            }
            // Try alternate peer versions before changing ordinary package
            // candidates; this also handles failures inside a peer's subtree.
            let mut peer_advanced = false;
            if let Ok(demands) = self.collect_peer_demands(owner) {
                let automatic = self
                    .children_of(owner)
                    .into_iter()
                    .filter(|(_, location)| self.lock.packages[location].peer)
                    .map(|(name, _)| name)
                    .collect::<BTreeSet<_>>();
                for name in automatic.iter().rev() {
                    let Some(consumers) = demands.get(name) else {
                        continue;
                    };
                    let location = peer::slot(owner, name);
                    if let Ok(candidates) = self.peer_candidates(&location, name, consumers) {
                        let index = peer_indices.entry(name.clone()).or_default();
                        if *index + 1 < candidates.len() {
                            *index += 1;
                            peer_advanced = true;
                            break;
                        }
                        *index = 0;
                    }
                }
            }
            if peer_advanced {
                continue;
            }
            selection = if let Some(next) = search.next() {
                next
            } else if let Some(optional) = optional_to_drop.take() {
                excluded.insert(optional);
                search = CandidateSearch::new(&choices, &excluded);
                search.next().expect("initial candidate selection")
            } else {
                self.lock.packages = before;
                return Err(last_error.unwrap_or_else(|| anyhow!("no compatible candidates")));
            };
            indices = search.indices(&selection);
            peer_indices.clear();
        }
        self.lock.packages = before;
        let exhaustion = format!(
            "dependency resolution exhausted the {MAX_SCOPE_ATTEMPTS}-attempt limit for {}",
            owner.unwrap_or("<root>")
        );
        match last_error {
            Some(error) => Err(error.context(exhaustion)),
            None => Err(anyhow!(exhaustion)),
        }
    }

    /// Return an optional dependency name to exclude and retry, or `None` on success.
    fn resolve_scope_once(
        &mut self,
        owner: Option<&str>,
        requirements: &BTreeMap<String, Requirement>,
        ancestors: &[(String, String)],
        depth: usize,
        choices: ScopeChoices<'_>,
    ) -> Result<Option<String>> {
        // Materialize ALL ordinary siblings before checking any peers, so the
        // order of package names cannot change the peer dependency decisions.
        for (name, requirement) in requirements {
            if choices.excluded.contains(name) {
                continue;
            }
            let selected = choices
                .versions
                .get(name)
                .and_then(|candidates| candidates.get(*choices.indices.get(name).unwrap_or(&0)))
                .ok_or_else(|| anyhow!("no selected candidate for {name}"))?
                .clone();
            // A cycle can reuse a previously installed *identical* ancestor.
            if ancestors.iter().any(|(ancestor_name, ancestor_version)| {
                ancestor_name == name && ancestor_version == &selected.version
            }) && peer::provider_in_scope(&self.lock.packages, owner, name)
                .is_some_and(|(_, pkg)| pkg.version == selected.version)
            {
                continue;
            }
            let location = peer::slot(owner, name);
            if !requirement.optional {
                self.narrow_optional_ancestor(owner, &selected)?;
            }
            if let Err(error) =
                self.insert_package(location, name, selected, requirement.optional, false)
            {
                if requirement.optional {
                    eprintln!("warning: skipping optional dependency {name}: {error:#}");
                    continue;
                }
                return Err(error).with_context(|| format!("placing {name}@{}", requirement.spec));
            }
        }

        if let Some(optional) = self.settle_peers(owner, choices.peer_indices)? {
            return Ok(Some(optional));
        }

        // Only after peer packages are placed do we descend into child scopes.
        // A failed optional child restarts this scope from its original graph.
        // The next breadth level's packuments are fetched together first.
        let children = self.children_of(owner);
        let next_level: BTreeSet<String> = children
            .iter()
            .flat_map(|(_, location)| {
                let package = &self.lock.packages[location];
                package
                    .dependencies
                    .keys()
                    .chain(package.optional_dependencies.keys())
                    .chain(package.peer_dependencies.keys())
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect();
        self.registry
            .prefetch(next_level.iter().map(String::as_str));
        for (name, location) in children {
            let package = self
                .lock
                .packages
                .get(&location)
                .expect("child was indexed")
                .clone();
            let mut child_reqs = BTreeMap::new();
            for (name, spec) in &package.dependencies {
                child_reqs.insert(
                    name.clone(),
                    Requirement {
                        spec: spec.clone(),
                        optional: false,
                    },
                );
            }
            // Optional dependencies supersede ordinary dependencies in npm.
            for (optional_name, spec) in &package.optional_dependencies {
                child_reqs.insert(
                    optional_name.clone(),
                    Requirement {
                        spec: spec.clone(),
                        optional: true,
                    },
                );
            }
            let mut path = ancestors.to_vec();
            path.push((package.name.clone(), package.version.clone()));
            if let Err(error) = self.resolve_scope(Some(&location), &child_reqs, &path, depth + 1) {
                if package.optional {
                    let optional = if package.peer {
                        self.collect_peer_demands(owner)?
                            .get(&name)
                            .and_then(|consumers| optional_consumer(consumers))
                    } else {
                        Some(name.clone())
                    };
                    if let Some(optional) = optional {
                        eprintln!(
                            "warning: skipping optional dependency {name}@{}: {error:#}",
                            package.version
                        );
                        return Ok(Some(optional));
                    }
                }
                return Err(error).with_context(|| format!("resolving dependencies of {location}"));
            }
        }
        Ok(None)
    }

    /// Ensure every direct child in this scope finds a compatible peer outside
    /// its own installation directory. Newly installed peers can declare peers
    /// of their own, so repeat until the local peer set reaches a fixed point.
    fn settle_peers(
        &mut self,
        owner: Option<&str>,
        peer_indices: &BTreeMap<String, usize>,
    ) -> Result<Option<String>> {
        for _ in 0..MAX_PACKAGES {
            let demands = self.collect_peer_demands(owner)?;
            let mut changed = false;

            // Drop auto-installed peer packages no longer required after a
            // candidate replacement; their child subtrees have not been built.
            for (name, location) in self.children_of(owner) {
                if self.lock.packages[&location].peer && !demands.contains_key(&name) {
                    self.lock.packages.remove(&location);
                    changed = true;
                    break;
                }
            }
            if changed {
                continue;
            }

            for (name, consumers) in &demands {
                let location = peer::slot(owner, name);
                let mut conditional = automatic_peer_platforms(consumers);
                let requested_by = consumers
                    .iter()
                    .map(|d| format!("{} requires {}@{}", d.consumer, name, d.spec))
                    .collect::<Vec<_>>()
                    .join("; ");
                if let Some(installed) = self.lock.packages.get(&location).cloned() {
                    if peers_match(&installed.version, consumers)?
                        && peer_targets_supported(&installed.os, &installed.cpu, consumers)
                        && (!installed.peer || installed.optional == conditional.is_some())
                    {
                        continue;
                    }
                    if installed.optional && !installed.peer {
                        return Ok(Some(name.clone()));
                    }
                    if !installed.peer {
                        if let Some(optional) = optional_consumer(consumers) {
                            return Ok(Some(optional));
                        }
                        return Err(PeerConflict::new(owner, name, consumers, format!(
                            "ERESOLVE peer conflict: {requested_by}; {name}@{} is already installed at {location}",
                            installed.version
                        )).into());
                    }
                }

                // A correct ancestor is preferable to adding a duplicate peer.
                let ancestor_scope = owner.and_then(peer::parent_scope);
                let visible = peer::provider_in_scope(&self.lock.packages, ancestor_scope, name);
                if !self.lock.packages.contains_key(&location) {
                    if let Some((_, pkg)) = visible {
                        if peers_match(&pkg.version, consumers)?
                            && peer_targets_supported(&pkg.os, &pkg.cpu, consumers)
                        {
                            continue;
                        }
                    }
                }
                if consumers.iter().all(|d| d.optional) {
                    let existing = self
                        .lock
                        .packages
                        .get(&location)
                        .or(visible.map(|(_, pkg)| pkg));
                    if let Some(existing) = existing {
                        if let Some(optional) = optional_consumer(consumers) {
                            return Ok(Some(optional));
                        }
                        return Err(PeerConflict::new(owner, name, consumers, format!(
                            "ERESOLVE optional peer conflict: {requested_by}; visible {name}@{} is incompatible",
                            existing.version
                        )).into());
                    }
                    // Optional peers are never auto-installed when absent.
                    continue;
                }
                let mut selected =
                    match self.select_peer_at(&location, name, consumers, peer_indices) {
                        Ok(value) => value,
                        Err(error) => {
                            if let Some(optional) = optional_consumer(consumers) {
                                eprintln!(
                                "warning: dropping optional peer consumer {optional}: {error:#}"
                            );
                                return Ok(Some(optional));
                            }
                            return Err(error).with_context(|| {
                                PeerConflict::new(
                                    owner,
                                    name,
                                    consumers,
                                    format!("ERESOLVE incompatible peers: {requested_by}"),
                                )
                            });
                        }
                    };
                // A required peer can narrow an optional consumer just as an
                // ordinary required child does. Recompute shared provider targets
                // after narrowing every consumer, before inserting the provider.
                for demand in consumers
                    .iter()
                    .filter(|demand| !demand.optional && demand.narrowable)
                {
                    if let Some(consumer) = &demand.consumer_name {
                        self.narrow_optional_ancestor(
                            Some(&peer::slot(owner, consumer)),
                            &selected,
                        )?;
                    }
                }
                if conditional.is_some() {
                    let narrowed = self.collect_peer_demands(owner)?;
                    conditional = automatic_peer_platforms(&narrowed[name]);
                }
                if let Some(platforms) = &conditional {
                    let platforms = platforms
                        .intersect(&selected.os, &selected.cpu)
                        .context("required peer has no shared compatible target")?;
                    selected.os = platforms.os;
                    selected.cpu = platforms.cpu;
                }
                self.lock.packages.remove(&location);
                self.insert_package(location, name, selected, conditional.is_some(), true)?;
                changed = true;
                break;
            }
            if !changed {
                return Ok(None);
            }
        }
        bail!("peer dependency resolution did not converge (limit {MAX_PACKAGES})")
    }

    fn collect_peer_demands(
        &self,
        owner: Option<&str>,
    ) -> Result<BTreeMap<String, Vec<PeerDemand>>> {
        let mut demands: BTreeMap<String, Vec<PeerDemand>> = BTreeMap::new();
        if owner.is_none() {
            for (name, spec) in &self.lock.peer_dependencies {
                demands.entry(name.clone()).or_default().push(PeerDemand {
                    consumer: "<root>".into(),
                    consumer_name: None,
                    consumer_optional: false,
                    spec: spec.clone(),
                    optional: self
                        .lock
                        .peer_dependencies_meta
                        .get(name)
                        .is_some_and(|p| p.optional),
                    platforms: None,
                    narrowable: false,
                });
            }
        }
        for (_, location) in self.children_of(owner) {
            let package = &self.lock.packages[&location];
            if package.peer_dependencies.is_empty() {
                continue;
            }
            let platforms = self.conditional_platforms(Some(&location))?;
            for (name, spec) in &package.peer_dependencies {
                demands.entry(name.clone()).or_default().push(PeerDemand {
                    consumer: format!("{}@{}", package.name, package.version),
                    consumer_name: Some(package.name.clone()),
                    consumer_optional: package.optional && !package.peer,
                    spec: spec.clone(),
                    optional: package
                        .peer_dependencies_meta
                        .get(name)
                        .is_some_and(|p| p.optional),
                    platforms: platforms.clone(),
                    narrowable: self.narrowable_optional_ancestor(Some(&location)).is_some(),
                });
            }
        }
        Ok(demands)
    }

    /// Peer releases are chosen deterministically, preferring the old pin and
    /// then trying remaining compatible candidates in descending version order.
    fn peer_candidates(
        &mut self,
        location: &str,
        name: &str,
        consumers: &[PeerDemand],
    ) -> Result<Vec<VersionMeta>> {
        let ranges = consumers.iter().map(|d| d.spec.clone()).collect::<Vec<_>>();
        let metadata = self.metadata(name)?;
        let mut versions = registry::matching_peer_candidates(&metadata, name, &ranges, |pkg| {
            let (os, cpu) = current_platform();
            consumers
                .iter()
                .filter(|demand| !demand.optional)
                .all(|demand| match &demand.platforms {
                    Some(platforms) if demand.narrowable => {
                        platforms.intersect(&pkg.os, &pkg.cpu).is_some()
                    }
                    Some(platforms) => platforms.covered_by(&pkg.os, &pkg.cpu),
                    None => compatible_platform(&pkg.os, os) && compatible_platform(&pkg.cpu, cpu),
                })
        })?;
        let pinned = versions
            .iter()
            .position(|pkg| self.preferred.get(location) == Some(&pkg.version))
            .or_else(|| {
                let known = self.preferred_by_name.get(name)?;
                versions.iter().position(|pkg| known.contains(&pkg.version))
            });
        if let Some(index) = pinned {
            let selected = versions.remove(index);
            versions.insert(0, selected);
        }
        Ok(versions)
    }

    fn select_peer_at(
        &mut self,
        location: &str,
        name: &str,
        consumers: &[PeerDemand],
        indices: &BTreeMap<String, usize>,
    ) -> Result<VersionMeta> {
        let candidates = self.peer_candidates(location, name, consumers)?;
        let index = *indices.get(name).unwrap_or(&0);
        candidates.get(index).cloned().ok_or_else(|| {
            anyhow!(
                "ERESOLVE no peer candidate {index} for {name}@{}",
                consumers
                    .iter()
                    .map(|d| d.spec.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }

    fn children_of(&self, owner: Option<&str>) -> Vec<(String, String)> {
        self.lock
            .packages
            .iter()
            .filter(|(location, _)| peer::parent_scope(location) == owner)
            .map(|(location, package)| (package.name.clone(), location.clone()))
            .collect()
    }

    fn conditional_platforms(&self, mut scope: Option<&str>) -> Result<Option<PlatformDomain>> {
        let mut ancestors = Vec::new();
        while let Some(location) = scope {
            ancestors.push(&self.lock.packages[location]);
            scope = peer::parent_scope(location);
        }
        if !ancestors.iter().any(|pkg| pkg.optional) {
            return Ok(None);
        }
        let domain = ancestors
            .into_iter()
            .try_fold(PlatformDomain::default(), |targets, pkg| {
                targets.intersect(&pkg.os, &pkg.cpu)
            })
            .context("optional dependency branch supports no compatible target")?;
        Ok(Some(domain))
    }

    fn narrowable_optional_ancestor<'a>(&self, mut scope: Option<&'a str>) -> Option<&'a str> {
        while let Some(location) = scope {
            let package = &self.lock.packages[location];
            if package.optional {
                // Auto peers have sibling consumers whose eligibility cannot be
                // narrowed by changing the provider in isolation.
                return (!package.peer).then_some(location);
            }
            scope = peer::parent_scope(location);
        }
        None
    }

    fn narrow_optional_ancestor(
        &mut self,
        owner: Option<&str>,
        selected: &VersionMeta,
    ) -> Result<()> {
        let Some(location) = self.narrowable_optional_ancestor(owner) else {
            return Ok(());
        };
        let platform = self
            .conditional_platforms(owner)?
            .expect("optional ancestor has a platform domain");
        if !platform.covered_by(&selected.os, &selected.cpu) {
            let narrowed = platform
                .intersect(&selected.os, &selected.cpu)
                .with_context(|| {
                    format!("{location} has no target compatible with a required dependency")
                })?;
            let branch = self
                .lock
                .packages
                .get_mut(location)
                .expect("optional ancestor is locked");
            branch.os = narrowed.os;
            branch.cpu = narrowed.cpu;
        }
        Ok(())
    }

    /// Validate package metadata before inserting the selected candidate.
    fn insert_package(
        &mut self,
        location: String,
        name: &str,
        selected: VersionMeta,
        optional: bool,
        auto_peer: bool,
    ) -> Result<()> {
        if self.lock.packages.len() >= MAX_PACKAGES {
            bail!("dependency graph exceeds {MAX_PACKAGES} packages");
        }
        if self.lock.packages.contains_key(&location) {
            bail!("duplicate lockfile path {location}");
        }
        for (peer_name, range) in &selected.peer_dependencies {
            peer::validate_requirement(peer_name, range)?;
            if selected.dependencies.contains_key(peer_name)
                || selected.optional_dependencies.contains_key(peer_name)
            {
                bail!(
                    "{name}@{} declares {peer_name} as both a peer and an ordinary dependency",
                    selected.version
                );
            }
        }
        for name in selected.peer_dependencies_meta.keys() {
            if !selected.peer_dependencies.contains_key(name) {
                bail!("{location} has peerDependenciesMeta for undeclared {name}");
            }
        }
        let mut dependencies = selected.dependencies;
        for optional in selected.optional_dependencies.keys() {
            dependencies.remove(optional);
        }
        for child in dependencies
            .keys()
            .chain(selected.optional_dependencies.keys())
        {
            crate::manifest::validate_name(child)?;
        }
        let integrity = integrity::canonical(
            selected.dist.integrity.as_deref(),
            selected.dist.shasum.as_deref(),
        )?;
        let bin = parse_bins(name, &selected.bin)?;
        self.lock.packages.insert(
            location,
            LockedPackage {
                name: name.into(),
                version: selected.version,
                resolved: selected.dist.tarball,
                integrity,
                dependencies,
                optional_dependencies: selected.optional_dependencies,
                peer_dependencies: selected.peer_dependencies,
                peer_dependencies_meta: selected.peer_dependencies_meta,
                optional,
                peer: auto_peer,
                bin,
                os: selected.os,
                cpu: selected.cpu,
            },
        );
        Ok(())
    }
}

/// Choose an optional package that can be dropped if peer constraints clash.
/// This does *not* suppress conflicts caused solely by required consumers.
fn optional_consumer(consumers: &[PeerDemand]) -> Option<String> {
    consumers
        .iter()
        .find(|d| d.consumer_optional)
        .and_then(|d| d.consumer_name.clone())
}

fn automatic_peer_platforms(consumers: &[PeerDemand]) -> Option<PlatformDomain> {
    let mut required = consumers.iter().filter(|demand| !demand.optional);
    let first = required.next()?.platforms.clone()?;
    required.try_fold(first, |domain, demand| {
        Some(domain.union(demand.platforms.as_ref()?))
    })
}

fn peer_targets_supported(os: &[String], cpu: &[String], consumers: &[PeerDemand]) -> bool {
    let (host_os, host_cpu) = current_platform();
    consumers
        .iter()
        .filter(|demand| !demand.optional)
        .all(|demand| match &demand.platforms {
            Some(platforms) => platforms.covered_by(os, cpu),
            None => compatible_platform(os, host_os) && compatible_platform(cpu, host_cpu),
        })
}

/// Return true when a resolved version satisfies every consumer's peer range.
fn peers_match(version: &str, consumers: &[PeerDemand]) -> Result<bool> {
    for demand in consumers {
        if !peer::satisfies(version, &demand.spec)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Npm supports either a binary path string or a map of executable names to paths.
fn parse_bins(name: &str, value: &Value) -> Result<BTreeMap<String, String>> {
    let mut bins = BTreeMap::new();
    match value {
        Value::Null => (),
        Value::String(path) => {
            let executable = name
                .rsplit('/')
                .next()
                .ok_or_else(|| anyhow!("invalid executable name"))?;
            bins.insert(executable.to_owned(), path.clone());
        }
        Value::Object(map) => {
            for (bin, path) in map {
                let path = path
                    .as_str()
                    .ok_or_else(|| anyhow!("invalid bin path for {name}:{bin}"))?;
                bins.insert(bin.clone(), path.to_owned());
            }
        }
        _ => bail!("invalid npm bin metadata for {name}"),
    }
    for (bin, path) in &bins {
        crate::manifest::validate_name(bin)?;
        crate::install::validate_member_path(path)?;
    }
    Ok(bins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scoped_string_bin() {
        let bins = parse_bins("@scope/compiler", &Value::String("bin/main.js".into())).unwrap();
        assert_eq!(bins["compiler"], "bin/main.js");
    }
    #[test]
    fn prevents_bin_traversal() {
        assert!(parse_bins("app", &serde_json::json!({"app":"../escape"})).is_err());
    }
}
