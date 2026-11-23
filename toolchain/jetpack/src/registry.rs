//! Read abbreviated npm package metadata and select compatible versions.
use crate::{
    config::RegistryConfig,
    transport::{fetch_all, Fetch, Transport},
};
use anyhow::{anyhow, bail, Context, Result};
use node_semver::{Range, Version};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};
use url::Url;

/// Largest accepted packument.
const MAX_METADATA: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_METADATA: u64 = 256 * 1024 * 1024;
const METADATA_BATCH: usize = 16;
/// Prefetch up to 64 MiB of response bodies per batch. Larger packuments are
/// read individually via `metadata`.
const PREFETCH_METADATA_LIMIT: u64 = MAX_METADATA / METADATA_BATCH as u64;
const ACCEPT_INSTALL: &str = "application/vnd.npm.install-v1+json";

/// Download metadata attached to a particular npm package release.
#[derive(Clone, Debug, Deserialize)]
pub struct Dist {
    pub tarball: String,
    pub integrity: Option<String>,
    pub shasum: Option<String>,
}

/// Registry metadata for one pinned npm version, including peer requirements.
#[derive(Clone, Debug, Deserialize)]
pub struct VersionMeta {
    pub version: String,
    pub dist: Dist,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies")]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "peerDependencies")]
    pub peer_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "peerDependenciesMeta")]
    pub peer_dependencies_meta: BTreeMap<String, PeerMeta>,
    #[serde(default)]
    pub bin: Value,
    #[serde(default)]
    pub os: Vec<String>,
    #[serde(default)]
    pub cpu: Vec<String>,
}

/// npm's `peerDependenciesMeta[name].optional` flag. Unknown metadata is ignored.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PeerMeta {
    #[serde(default)]
    pub optional: bool,
}

/// Registry packument containing dist-tags and all published releases.
#[derive(Clone, Debug, Deserialize)]
pub struct PackageMetadata {
    #[serde(default, rename = "dist-tags")]
    pub dist_tags: HashMap<String, String>,
    pub versions: HashMap<String, VersionMeta>,
}

/// The packument as sent: each version is parsed on its own, so one malformed release
/// does not make the whole package unusable.
#[derive(Deserialize)]
struct RawMetadata {
    #[serde(default, rename = "dist-tags")]
    dist_tags: HashMap<String, String>,
    #[serde(default)]
    versions: HashMap<String, Value>,
}

/// npm registry client with an in-memory metadata cache, over an embedder's transport.
pub struct Registry {
    transport: Arc<dyn Transport>,
    config: RegistryConfig,
    cache: HashMap<String, PackageMetadata>,
    metadata_bytes: u64,
}

impl Registry {
    /// Construct a registry client for a single URL. HTTP is permitted only
    /// for loopback hosts; external registries must use HTTPS.
    pub fn new(url: &str, transport: Arc<dyn Transport>) -> Result<Self> {
        Self::with_config(RegistryConfig::single(url)?, transport)
    }

    /// Construct a client from `.npmrc` registry routes and scoped credentials.
    pub fn with_config(config: RegistryConfig, transport: Arc<dyn Transport>) -> Result<Self> {
        config.sources().validate()?;
        Ok(Self {
            transport,
            config,
            cache: HashMap::new(),
            metadata_bytes: 0,
        })
    }

    /// Prefetches uncached packuments in bounded concurrent batches. Unsuccessful
    /// requests and large packuments are retried by [`Registry::metadata`].
    pub fn prefetch<'n>(&mut self, names: impl IntoIterator<Item = &'n str>) {
        let names: BTreeSet<&str> = names
            .into_iter()
            .filter(|name| {
                !self.cache.contains_key(*name) && crate::manifest::validate_name(name).is_ok()
            })
            .collect();
        let mut fetches = Vec::new();
        let mut wanted = Vec::new();
        for name in names {
            if let Ok(url) = self.metadata_url(name) {
                fetches.push(Fetch {
                    url: url.to_string(),
                    accept: Some(ACCEPT_INSTALL),
                    limit: PREFETCH_METADATA_LIMIT,
                });
                wanted.push(name);
            }
        }
        if fetches.len() < 2 {
            return; // a single name gains nothing over metadata()
        }
        for (names, fetches) in wanted
            .chunks(METADATA_BATCH)
            .zip(fetches.chunks(METADATA_BATCH))
        {
            if self.metadata_bytes + MAX_METADATA > MAX_TOTAL_METADATA {
                break; // Use the remaining budget for demanded packuments.
            }
            let bodies = fetch_all(self.transport.as_ref(), Some(&self.config), fetches);
            for (&name, body) in names.iter().zip(bodies) {
                if let Ok(metadata) = body.and_then(|bytes| self.parse_metadata(name, &bytes)) {
                    self.cache.insert(name.to_owned(), metadata);
                }
            }
        }
    }

    fn metadata_url(&self, name: &str) -> Result<Url> {
        let base = Url::parse(self.config.sources().for_package(name))?;
        Ok(base.join(&name.replace('/', "%2F"))?)
    }

    /// Parses a packument, keeping the releases that are well formed and resolving relative
    /// tarball URLs against the package's registry base (not the metadata URL).
    fn parse_metadata(&mut self, name: &str, bytes: &[u8]) -> Result<PackageMetadata> {
        self.metadata_bytes += bytes.len() as u64;
        if self.metadata_bytes > MAX_TOTAL_METADATA {
            bail!("registry metadata exceeds the total resolution budget");
        }
        let raw: RawMetadata = serde_json::from_slice(bytes)
            .with_context(|| format!("invalid npm registry metadata for {name}"))?;
        let base = Url::parse(self.config.sources().for_package(name))?;
        let mut versions = HashMap::new();
        for (number, value) in raw.versions {
            let Ok(mut version) = serde_json::from_value::<VersionMeta>(value) else {
                continue;
            };
            if version.dist.tarball.starts_with("//") {
                continue;
            }
            let Ok(target) = base.join(&version.dist.tarball) else {
                continue;
            };
            if validate_url(target.as_str()).is_err() {
                continue;
            }
            version.dist.tarball = target.to_string();
            versions.insert(number, version);
        }
        Ok(PackageMetadata {
            dist_tags: raw.dist_tags,
            versions,
        })
    }

    /// The non-secret source selection recorded in the lockfile.
    pub fn sources(&self) -> &crate::config::RegistrySources {
        self.config.sources()
    }

    /// Fetch a name's abbreviated registry packument, using the in-memory
    /// cache. Scoped packages go to their configured registry, including
    /// custom loopback registries on nonstandard ports or URL prefixes.
    pub fn metadata(&mut self, name: &str) -> Result<PackageMetadata> {
        crate::manifest::validate_name(name)?;
        if let Some(meta) = self.cache.get(name) {
            return Ok(meta.clone());
        }
        let limit = MAX_METADATA.min(MAX_TOTAL_METADATA.saturating_sub(self.metadata_bytes));
        if limit == 0 {
            bail!("registry metadata exceeds the total resolution budget");
        }
        let url = self.metadata_url(name)?;
        let fetch = Fetch {
            url: url.to_string(),
            accept: Some(ACCEPT_INSTALL),
            limit,
        };
        let bytes = fetch_all(self.transport.as_ref(), Some(&self.config), &[fetch])
            .pop()
            .expect("one result per fetch")
            .with_context(|| format!("failed to fetch npm metadata for {name}"))?;
        let metadata = self.parse_metadata(name, &bytes)?;
        self.cache.insert(name.to_owned(), metadata.clone());
        Ok(metadata)
    }

    /// Select a normal dependency by semver range or npm dist-tag.
    pub fn select(&mut self, name: &str, spec: &str) -> Result<VersionMeta> {
        let metadata = self.metadata(name)?;
        select_from_metadata(&metadata, name, spec)
    }

    /// All installable releases for an ordinary requirement, newest first.
    /// A dist-tag pins a single version; unsupported ranges fail closed.
    pub fn candidates(&mut self, name: &str, spec: &str) -> Result<Vec<VersionMeta>> {
        let metadata = self.metadata(name)?;
        candidates_from_metadata(&metadata, name, spec)
    }

    /// As [`Registry::candidates`], for any platform: optional packages built for other
    /// platforms are kept in the lockfile and skipped at install.
    pub fn candidates_any_platform(&mut self, name: &str, spec: &str) -> Result<Vec<VersionMeta>> {
        let metadata = self.metadata(name)?;
        candidates_for(&metadata, name, spec, None)
    }

    /// Choose the highest version satisfying *every* peer semver range in a scope.
    /// Registry tags and non-registry specifiers are not accepted as peer constraints.
    pub fn select_peer(&mut self, name: &str, ranges: &[String]) -> Result<VersionMeta> {
        self.peer_candidates(name, ranges)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("no compatible peer version found for {name}"))
    }

    /// All releases satisfying the intersection of the requested peer ranges.
    /// Results are sorted newest first, for bounded resolution backtracking.
    pub fn peer_candidates(&mut self, name: &str, ranges: &[String]) -> Result<Vec<VersionMeta>> {
        let metadata = self.metadata(name)?;
        peer_candidates_from_metadata(&metadata, name, ranges)
    }
}

/// Reject non-HTTPS URLs except an *actual* loopback host (not a misleading
/// `localhost:port@evil` user-info string). Redirects are disabled separately.
pub fn validate_url(value: &str) -> Result<()> {
    let url = Url::parse(value).context("invalid package URL")?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        bail!("package URL contains credentials or a fragment");
    }
    let is_local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && is_local) {
        bail!("package URL must use HTTPS (HTTP allowed only on loopback)");
    }
    Ok(())
}

/// Select registry candidates ordered by descending semver, filtering incompatible
/// platforms *before* a version is pinned. Prereleases follow `node-semver`.
pub fn candidates_from_metadata(
    meta: &PackageMetadata,
    name: &str,
    spec: &str,
) -> Result<Vec<VersionMeta>> {
    candidates_for(meta, name, spec, Some(current_platform()))
}

/// Candidates for `platform`, or for every platform when `None`.
fn candidates_for(
    meta: &PackageMetadata,
    name: &str,
    spec: &str,
    platform: Option<(&str, &str)>,
) -> Result<Vec<VersionMeta>> {
    let fits = |pkg: &VersionMeta| {
        platform.is_none_or(|(os, cpu)| {
            compatible_platform(&pkg.os, os) && compatible_platform(&pkg.cpu, cpu)
        })
    };
    let target = platform.map_or_else(
        || "any platform".to_owned(),
        |(os, cpu)| format!("{os}/{cpu}"),
    );
    matching_candidates(meta, name, spec, fits, &target)
}

pub(crate) fn matching_candidates(
    meta: &PackageMetadata,
    name: &str,
    spec: &str,
    fits: impl Fn(&VersionMeta) -> bool,
    target: &str,
) -> Result<Vec<VersionMeta>> {
    if let Some(tag_version) = meta.dist_tags.get(spec) {
        let pkg = meta
            .versions
            .get(tag_version)
            .ok_or_else(|| anyhow!("tag {name}@{spec} points to missing version {tag_version}"))?;
        if !fits(pkg) {
            bail!("{name}@{tag_version} does not support {target}");
        }
        return Ok(vec![pkg.clone()]);
    }
    let range: Range = spec.parse().with_context(||
        format!("unsupported dependency range '{spec}' for {name}; registry semver ranges and tags only"))?;
    let mut candidates = meta
        .versions
        .iter()
        .filter_map(|(number, pkg)| {
            let version = number.parse::<Version>().ok()?;
            (version.satisfies(&range) && fits(pkg) && pkg.version == *number)
                .then_some((version, pkg.clone()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.version.cmp(&a.1.version)));
    if candidates.is_empty() {
        bail!("no compatible npm release of {name} satisfies {spec} on {target}");
    }
    Ok(candidates.into_iter().map(|(_, pkg)| pkg).collect())
}

/// Select the highest compatible release satisfying a normal npm requirement.
pub fn select_from_metadata(meta: &PackageMetadata, name: &str, spec: &str) -> Result<VersionMeta> {
    candidates_from_metadata(meta, name, spec)?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no release of {name} satisfies {spec}"))
}

/// All compatible releases satisfying an intersection of peer semver ranges.
/// Sorted newest first, independent of registry object ordering.
pub fn peer_candidates_from_metadata(
    meta: &PackageMetadata,
    name: &str,
    specs: &[String],
) -> Result<Vec<VersionMeta>> {
    let (os, cpu) = current_platform();
    matching_peer_candidates(meta, name, specs, |pkg| {
        compatible_platform(&pkg.os, os) && compatible_platform(&pkg.cpu, cpu)
    })
}

pub(crate) fn matching_peer_candidates(
    meta: &PackageMetadata,
    name: &str,
    specs: &[String],
    fits: impl Fn(&VersionMeta) -> bool,
) -> Result<Vec<VersionMeta>> {
    let ranges: Vec<Range> = specs
        .iter()
        .map(|spec| {
            spec.parse().with_context(|| {
                format!("unsupported peer range '{spec}' for {name}: expected npm semver")
            })
        })
        .collect::<Result<_>>()?;
    let mut matching: Vec<_> = meta
        .versions
        .iter()
        .filter_map(|(key, meta)| {
            let version = key.parse::<Version>().ok()?;
            (fits(meta)
                && ranges.iter().all(|range| version.satisfies(range))
                && meta.version == *key)
                .then_some((version, meta))
        })
        .collect();
    matching.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.version.cmp(&a.1.version)));
    if matching.is_empty() {
        bail!(
            "no release of {name} satisfies all peer ranges: {}",
            specs.join(", ")
        );
    }
    Ok(matching
        .into_iter()
        .map(|(_, meta)| (*meta).clone())
        .collect())
}

/// Select the highest compatible release meeting all peer ranges.
pub fn select_peer_from_metadata(
    meta: &PackageMetadata,
    name: &str,
    specs: &[String],
) -> Result<VersionMeta> {
    peer_candidates_from_metadata(meta, name, specs)?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no compatible peer version found for {name}"))
}

/// Test npm's allowlist/negation rules against one OS or architecture value.
pub fn compatible_platform(spec: &[String], value: &str) -> bool {
    let excluded = spec.iter().any(|part| part == &format!("!{value}"));
    let positive = spec
        .iter()
        .filter(|part| !part.starts_with('!'))
        .collect::<Vec<_>>();
    !excluded && (positive.is_empty() || positive.iter().any(|part| part.as_str() == value))
}

/// Map the running Rust target to npm-compatible OS and CPU identifiers.
pub fn current_platform() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let cpu = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        other => other,
    };
    (os, cpu)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> PackageMetadata {
        serde_json::from_value(serde_json::json!({
            "dist-tags": {"latest": "2.0.0", "next": "3.0.0-beta.1"},
            "versions": {
                "1.8.0": {"version":"1.8.0","dist":{"tarball":"https://example.test/1.tgz","shasum":"a"}},
                "1.9.0": {"version":"1.9.0","dist":{"tarball":"https://example.test/2.tgz","shasum":"a"}},
                "2.0.0": {"version":"2.0.0","dist":{"tarball":"https://example.test/3.tgz","shasum":"a"}},
                "3.0.0-beta.1": {"version":"3.0.0-beta.1","dist":{"tarball":"https://example.test/4.tgz","shasum":"a"}}
            }
        })).unwrap()
    }

    #[test]
    fn url_policy_rejects_userinfo_host_spoofing_and_remote_http() {
        assert!(validate_url("https://registry.npmjs.org/package.tgz").is_ok());
        assert!(validate_url("http://127.0.0.1:1234/package.tgz").is_ok());
        assert!(validate_url("http://localhost:1234@evil.example/package.tgz").is_err());
        assert!(validate_url("http://registry.npmjs.org/package.tgz").is_err());
        assert!(validate_url("https://user:secret@example.com/package.tgz").is_err());
    }

    #[test]
    fn resolves_ranges_and_dist_tags() {
        assert_eq!(
            select_from_metadata(&metadata(), "x", "^1.0")
                .unwrap()
                .version,
            "1.9.0"
        );
        assert_eq!(
            select_from_metadata(&metadata(), "x", "latest")
                .unwrap()
                .version,
            "2.0.0"
        );
        assert_eq!(
            select_from_metadata(&metadata(), "x", "next")
                .unwrap()
                .version,
            "3.0.0-beta.1"
        );
        assert_eq!(
            select_from_metadata(&metadata(), "x", "*").unwrap().version,
            "2.0.0"
        );
    }

    #[test]
    fn one_malformed_release_does_not_reject_the_package() {
        let mut registry = Registry::new("https://example.test/", Arc::new(Offline)).unwrap();
        let bytes = serde_json::to_vec(&serde_json::json!({
            "dist-tags": {"latest": "1.1.0"},
            "versions": {
                "1.0.0": {"version": "1.0.0", "dist": "not an object"},
                "1.1.0": {"version": "1.1.0", "dist": {"tarball": "https://example.test/1.1.0.tgz", "shasum": "a"}}
            }
        })).unwrap();
        let metadata = registry.parse_metadata("x", &bytes).unwrap();
        assert_eq!(
            select_from_metadata(&metadata, "x", "^1").unwrap().version,
            "1.1.0"
        );
    }

    #[test]
    fn prefetch_batches_bound_response_bytes_and_leave_large_packuments_to_metadata() {
        use crate::transport::{Request, Response};
        use std::sync::Mutex;

        #[derive(Default)]
        struct Fixture(Mutex<Vec<Vec<u64>>>);

        impl Transport for Fixture {
            fn fetch_parallel(&self, requests: Vec<Request>) -> Vec<Result<Response>> {
                self.fetch(requests)
            }

            fn fetch(&self, requests: Vec<Request>) -> Vec<Result<Response>> {
                self.0
                    .lock()
                    .unwrap()
                    .push(requests.iter().map(|request| request.limit).collect());
                requests
                    .into_iter()
                    .map(|request| {
                        if request.url.ends_with("/large") && request.limit < MAX_METADATA {
                            bail!("fixture exceeds the speculative limit");
                        }
                        Ok(Response {
                            status: 200,
                            location: None,
                            body: br#"{"versions":{}}"#.to_vec(),
                        })
                    })
                    .collect()
            }
        }

        let transport = Arc::new(Fixture::default());
        let mut registry = Registry::new("https://registry.test/", transport.clone()).unwrap();
        let names = (0..=METADATA_BATCH)
            .map(|i| format!("pkg-{i}"))
            .collect::<Vec<_>>();
        registry.prefetch(names.iter().map(String::as_str).chain(["large"]));
        for name in &names {
            registry.metadata(name).unwrap();
        }
        registry.metadata("large").unwrap();
        assert_eq!(
            *transport.0.lock().unwrap(),
            [
                vec![PREFETCH_METADATA_LIMIT; METADATA_BATCH],
                vec![PREFETCH_METADATA_LIMIT; 2],
                vec![MAX_METADATA],
            ]
        );
    }

    struct Offline;
    impl Transport for Offline {
        fn fetch(
            &self,
            batch: Vec<crate::transport::Request>,
        ) -> Vec<Result<crate::transport::Response>> {
            batch.into_iter().map(|_| Err(anyhow!("offline"))).collect()
        }
    }

    #[test]
    fn excludes_wrong_platforms() {
        assert!(!compatible_platform(&["!linux".into()], "linux"));
        assert!(compatible_platform(
            &["darwin".into(), "linux".into()],
            "linux"
        ));
        assert!(!compatible_platform(&["darwin".into()], "linux"));
    }
}
