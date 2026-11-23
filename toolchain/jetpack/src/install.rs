//! Verified package downloads and contained tarball extraction into a staging tree.
//! Lifecycle scripts are deliberately never executed.
mod graph;
mod transaction;

use crate::{
    config::RegistryConfig,
    registry::{compatible_platform, current_platform},
    transport::{fetch_all, BodyTooLarge, Fetch, Transport},
};
use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use tar::Archive;

use crate::{
    integrity,
    lockfile::{LockedPackage, Lockfile},
    manifest::Manifest,
};
use transaction::Output;

// Conservative safety budgets; notably high enough for large npm tarballs.
const MAX_COMPRESSED: u64 = 256 * 1024 * 1024;
const MAX_UNPACKED: u64 = 4 * 1024 * 1024 * 1024;
const MAX_FILES: usize = 100_000;
const MAX_TAR_METADATA: u64 = 64 * 1024;
const MAX_TAR_STREAM: u64 = MAX_UNPACKED + 128 * 1024 * 1024;
const MAX_INSTALL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_INSTALL_ENTRIES: usize = 500_000;
const MAX_MANIFEST: u64 = 1024 * 1024;
/// Tarballs requested concurrently; each batch is unpacked before the next is fetched.
const DOWNLOAD_BATCH: usize = 32;
/// Concurrent tarballs use at most 128 MiB of payload buffer per batch.
const PREFETCH_TARBALL_LIMIT: u64 = 128 * 1024 * 1024 / DOWNLOAD_BATCH as u64;
const PROCESS_LOCK: &str = ".rustnpm-install.lock";

/// Checks one unpacked package (its staging directory and lock record) before the tree is
/// activated. An error rejects a required package and skips an optional one.
pub type Verify<'a> = dyn Fn(&Path, &LockedPackage) -> Result<()> + 'a;

/// Checks the effective manifest and surviving lock graph against the whole staged tree.
pub type VerifyTree<'a> = dyn Fn(&Path, &Lockfile) -> Result<()> + 'a;

/// How [`install_locked`] reaches the network and judges packages.
pub struct InstallOptions<'a> {
    pub transport: &'a dyn Transport,
    /// Registry routes and credentials. When set, they must equal the lockfile's routes.
    pub config: Option<&'a RegistryConfig>,
    pub verify: Option<&'a Verify<'a>>,
}

/// Both binary targets and files inside tarballs must stay within their package directory.
/// Relative `./bin/cli.js` paths are valid; `..`, absolute paths, and Windows prefixes are not.
pub fn validate_member_path(path: &str) -> Result<()> {
    let mut useful_components = 0;
    for component in Path::new(path).components() {
        match component {
            Component::Normal(_) => useful_components += 1,
            Component::CurDir => (),
            _ => bail!("unsafe path in package archive or bin: {path}"),
        }
    }
    if useful_components == 0 || path.contains('\\') || path.contains(':') {
        bail!("invalid archive/bin member path: {path}");
    }
    Ok(())
}

struct Downloader<'a> {
    transport: &'a dyn Transport,
    cache: Option<PathBuf>,
    config: Option<&'a RegistryConfig>,
}

enum CachedTarball {
    Missing,
    Ready(Vec<u8>),
    AboveLimit,
}

impl<'a> Downloader<'a> {
    fn new(transport: &'a dyn Transport, config: Option<&'a RegistryConfig>) -> Self {
        let cache = dirs::cache_dir().map(|p| p.join("rustnpm").join("tarballs"));
        Self {
            transport,
            cache,
            config,
        }
    }

    fn cache_path(&self, package: &LockedPackage) -> Option<PathBuf> {
        self.cache.as_ref().map(|dir| {
            let fingerprint =
                Sha256::digest(format!("{}:{}", package.resolved, package.integrity).as_bytes());
            dir.join(hex::encode(fingerprint))
        })
    }

    fn cached(&self, package: &LockedPackage, limit: u64) -> CachedTarball {
        let opened = self.cache_path(package).and_then(|path| {
            let file = File::open(&path).ok()?;
            let metadata = file.metadata().ok()?;
            (metadata.is_file() && metadata.len() <= MAX_COMPRESSED).then_some((
                file,
                path,
                metadata.len(),
            ))
        });
        let Some((file, path, len)) = opened else {
            return CachedTarball::Missing;
        };
        if len > limit {
            return CachedTarball::AboveLimit;
        }
        let mut bytes = Vec::with_capacity(len as usize);
        let read = file.take(limit + 1).read_to_end(&mut bytes);
        if bytes.len() as u64 > limit {
            return CachedTarball::AboveLimit;
        }
        if read.is_ok() && integrity::verify(&bytes, &package.integrity).is_ok() {
            return CachedTarball::Ready(bytes);
        }
        let _ = fs::remove_file(path);
        CachedTarball::Missing
    }

    /// Prefetch a bounded batch. If a response exceeds the prefetch limit, the
    /// iterator fetches it on demand under the ordinary single-tarball limit.
    fn get_all<'b>(
        &'b self,
        packages: &'b [&'b LockedPackage],
    ) -> impl Iterator<Item = Result<Vec<u8>>> + 'b {
        debug_assert!(packages.len() <= DOWNLOAD_BATCH);
        let cached: Vec<_> = packages
            .iter()
            .map(|package| self.cached(package, PREFETCH_TARBALL_LIMIT))
            .collect();
        let fetches: Vec<_> = packages
            .iter()
            .zip(&cached)
            .filter(|(_, cached)| matches!(cached, CachedTarball::Missing))
            .map(|(package, _)| Self::fetch(package, PREFETCH_TARBALL_LIMIT))
            .collect();
        let mut bodies = fetch_all(self.transport, self.config, &fetches).into_iter();
        packages.iter().zip(cached).map(move |(package, cached)| {
            let body = match cached {
                CachedTarball::Ready(bytes) => return Ok(bytes),
                CachedTarball::AboveLimit => {
                    if let CachedTarball::Ready(bytes) = self.cached(package, MAX_COMPRESSED) {
                        return Ok(bytes);
                    }
                    self.fetch_single(package)
                }
                CachedTarball::Missing => {
                    bodies
                        .next()
                        .expect("one result per fetch")
                        .or_else(|error| {
                            if error.is::<BodyTooLarge>() {
                                self.fetch_single(package)
                            } else {
                                Err(error)
                            }
                        })
                }
            };
            let label = format!("{}@{}", package.name, package.version);
            let data = body.with_context(|| format!("failed to download {label}"))?;
            integrity::verify(&data, &package.integrity)
                .with_context(|| format!("tarball verification failed for {label}"))?;
            self.store(package, &data);
            Ok(data)
        })
    }

    fn fetch(package: &LockedPackage, limit: u64) -> Fetch {
        Fetch {
            url: package.resolved.clone(),
            accept: None,
            limit,
        }
    }

    fn fetch_single(&self, package: &LockedPackage) -> Result<Vec<u8>> {
        fetch_all(
            self.transport,
            self.config,
            &[Self::fetch(package, MAX_COMPRESSED)],
        )
        .pop()
        .expect("one result per fetch")
    }

    fn store(&self, package: &LockedPackage, data: &[u8]) {
        let Some(path) = self.cache_path(package) else {
            return;
        };
        let Some(dir) = path.parent() else { return };
        if fs::create_dir_all(dir).is_ok() {
            static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let id = NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let tmp = path.with_extension(format!("tmp.{}.{id}", std::process::id()));
            if let Ok(mut file) = OpenOptions::new().create_new(true).write(true).open(&tmp) {
                let written = file.write_all(data).is_ok();
                drop(file);
                if !written || fs::rename(&tmp, &path).is_err() {
                    let _ = fs::remove_file(&tmp);
                }
            }
        }
    }
}

#[cfg(test)]
mod downloader_tests {
    use super::*;
    use crate::transport::{Request, Response};
    use base64::{engine::general_purpose::STANDARD, Engine};
    use std::sync::Mutex;

    #[test]
    fn retries_large_tarballs_lazily_with_the_single_archive_limit() {
        const ARCHIVE: &[u8] = b"verified fixture";
        #[derive(Default)]
        struct RecordingTransport(Mutex<Vec<Vec<u64>>>);

        impl Transport for RecordingTransport {
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
                        if request.url.ends_with("/large.tgz") && request.limit < MAX_COMPRESSED {
                            return Err(BodyTooLarge {
                                limit: request.limit,
                            }
                            .into());
                        }
                        Ok(Response {
                            status: if request.url.ends_with("/denied.tgz") {
                                401
                            } else {
                                200
                            },
                            location: None,
                            body: ARCHIVE.to_vec(),
                        })
                    })
                    .collect()
            }
        }

        let package = |name: &str| -> LockedPackage {
            serde_json::from_value(serde_json::json!({
                "name": name,
                "version": "1.0.0",
                "resolved": format!("https://registry.test/{name}.tgz"),
                "integrity": format!("sha256-{}", STANDARD.encode(Sha256::digest(ARCHIVE))),
            }))
            .unwrap()
        };
        let transport = RecordingTransport::default();
        let downloader = Downloader {
            transport: &transport,
            cache: None,
            config: None,
        };
        let (small, large) = (package("small"), package("large"));
        let denied = package("denied");
        let packages = [&small, &large, &denied];
        let mut contents = downloader.get_all(&packages);
        assert_eq!(contents.next().unwrap().unwrap(), ARCHIVE);
        assert_eq!(
            *transport.0.lock().unwrap(),
            [vec![PREFETCH_TARBALL_LIMIT; 3]]
        );
        assert_eq!(contents.next().unwrap().unwrap(), ARCHIVE);
        let error = contents.next().unwrap().unwrap_err();
        assert!(format!("{error:#}").contains("HTTP 401"));
        assert_eq!(
            *transport.0.lock().unwrap(),
            [vec![PREFETCH_TARBALL_LIMIT; 3], vec![MAX_COMPRESSED]]
        );
    }
}

/// Decompress only regular files and directories under the tarball's `package/` prefix.
/// Links/devices are rejected to prevent extraction escapes and unexpected filesystem objects.
#[cfg(test)]
fn unpack_tgz(tgz: &[u8], destination: &Path) -> Result<()> {
    let mut budget = ExtractionBudget::default();
    extract_tgz(tgz, destination, &mut budget)
}

#[derive(Default)]
struct ExtractionBudget {
    bytes: u64,
    entries: usize,
}

#[derive(Debug)]
struct InstallBudgetExceeded;

impl std::fmt::Display for InstallBudgetExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("install exceeds the total extraction budget")
    }
}

impl std::error::Error for InstallBudgetExceeded {}

/// Read raw headers before letting tar buffer extension records. A raw walk
/// streams file bodies and checks both the metadata and the decoded stream.
fn inspect_archive(tgz: &[u8], budget: &mut ExtractionBudget) -> Result<()> {
    let mut archive = Archive::new(GzDecoder::new(tgz).take(MAX_TAR_STREAM));
    let mut bytes = 0_u64;
    let mut entries = 0;
    for entry in archive.entries()?.raw(true) {
        let mut entry = entry?;
        entries += 1;
        if entries > MAX_FILES {
            bail!("npm package archive contains too many entries");
        }
        let size = entry.size();
        let kind = entry.header().entry_type();
        if kind.is_file() {
            bytes = bytes.checked_add(size).context("archive size overflow")?;
            if bytes > MAX_UNPACKED {
                bail!("npm package exceeds unpacked size limit");
            }
        } else if kind.is_dir() {
            if size != 0 {
                bail!("npm archive directory has a data payload");
            }
        } else if kind.is_gnu_longname() || kind.is_pax_local_extensions() {
            if size > MAX_TAR_METADATA {
                bail!("npm archive extension exceeds its size limit");
            }
            if let Some(fields) = entry.pax_extensions()? {
                for field in fields {
                    // A PAX size override can make the raw and normal readers
                    // disagree on where the next header starts.
                    if field?.key_bytes() == b"size" {
                        bail!("npm archive cannot override a tar entry size");
                    }
                }
            }
        } else {
            bail!("links and special files are unsupported in npm archives");
        }
    }
    let mut stream = archive.into_inner();
    std::io::copy(&mut stream, &mut std::io::sink())?;
    if stream.limit() == 0 {
        bail!("npm archive exceeds the decoded stream size limit");
    }
    budget.bytes += bytes;
    budget.entries += entries;
    if budget.bytes > MAX_INSTALL_BYTES || budget.entries > MAX_INSTALL_ENTRIES {
        return Err(InstallBudgetExceeded.into());
    }
    Ok(())
}

fn extract_tgz(tgz: &[u8], destination: &Path, budget: &mut ExtractionBudget) -> Result<()> {
    inspect_archive(tgz, budget)?;
    let gzip = GzDecoder::new(tgz).take(MAX_TAR_STREAM);
    let mut archive = Archive::new(gzip);
    let mut total_size = 0_u64;
    let mut entries = 0_usize;
    for file in archive.entries().context("invalid npm tar archive")? {
        let mut entry = file.context("malformed tar entry")?;
        entries += 1;
        if entries > MAX_FILES {
            bail!("npm package archive contains too many files");
        }
        let raw_path = entry.path()?.into_owned();
        let mut components = raw_path.components();
        if components.next() != Some(Component::Normal(std::ffi::OsStr::new("package"))) {
            bail!(
                "npm tarball entries must be inside package/: {}",
                raw_path.display()
            );
        }
        let mut rest = PathBuf::new();
        for component in components {
            match component {
                Component::Normal(value) => {
                    let name = value.to_str().context("non-UTF-8 package archive path")?;
                    if name
                        .trim_end_matches([' ', '.'])
                        .eq_ignore_ascii_case("node_modules")
                    {
                        bail!("bundled node_modules is unsupported in npm archives");
                    }
                    rest.push(value);
                }
                Component::CurDir => (),
                _ => bail!("archive path escapes package/: {}", raw_path.display()),
            }
        }
        if rest.as_os_str().is_empty() {
            continue;
        }
        let relative = rest.to_str().context("non-UTF-8 package archive path")?;
        if relative.len() > 4096 || rest.components().count() > 128 {
            bail!("npm package archive path exceeds its size limit");
        }
        validate_member_path(relative)?;
        let output = destination.join(rest);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            fs::create_dir_all(&output)?;
        } else if kind.is_file() {
            let size = entry.header().size()?;
            total_size = total_size
                .checked_add(size)
                .context("archive size overflow")?;
            if total_size > MAX_UNPACKED {
                bail!("npm package exceeds unpacked size limit");
            }
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut output_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
                .with_context(|| format!("cannot extract {}", output.display()))?;
            let copied = std::io::copy(&mut entry, &mut output_file)?;
            if copied != size {
                bail!("truncated tar entry: {}", raw_path.display());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let executable = entry.header().mode().unwrap_or(0) & 0o111 != 0;
                let permissions =
                    fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 });
                fs::set_permissions(&output, permissions)?;
            }
        } else {
            bail!(
                "links and special files are unsupported in npm archives: {}",
                raw_path.display()
            );
        }
    }
    Ok(())
}

/// Filesystem advisory lock protecting an entire project mutation. Keep this
/// guard alive through installation and all package.json/lockfile writes.
/// A lock file remains on disk; OS locks are released automatically on crash.
pub struct InstallGuard {
    file: File,
    project: PathBuf,
}

impl InstallGuard {
    /// Acquire an exclusive non-blocking project lock. A second jetpack
    /// process receives an error instead of modifying the same project. Complete
    /// any interrupted transaction before the caller reads project metadata.
    pub fn acquire(project: &Path) -> Result<Self> {
        let canonical = project
            .canonicalize()
            .context("cannot access project directory")?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(canonical.join(PROCESS_LOCK))
            .context("cannot open jetpack project lock")?;
        file.try_lock_exclusive()
            .context("another jetpack process is installing in this project")?;
        let guard = Self {
            file,
            project: canonical,
        };
        transaction::recover(&guard.project)?;
        Ok(guard)
    }
}

impl Drop for InstallGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Install packages with an internally acquired lock. Failures before activation
/// leave existing node_modules unchanged; a crash is recovered on the next run.
pub fn install(project: &Path, lock: &Lockfile, transport: &dyn Transport) -> Result<()> {
    let guard = InstallGuard::acquire(project)?;
    install_locked(
        project,
        lock,
        &guard,
        &InstallOptions {
            transport,
            config: None,
            verify: None,
        },
    )
}

/// Install the locked tree under an existing project lock. With a configuration, tokens are
/// sent only to matching registry and tarball URL prefixes, and the lockfile's routing must equal
/// the configuration's, so a graph is never replayed against another registry.
///
/// Optional packages for another OS or CPU are skipped together with their nested packages.
/// A required package for another platform is an error.
pub fn install_locked(
    project: &Path,
    lock: &Lockfile,
    guard: &InstallGuard,
    options: &InstallOptions,
) -> Result<()> {
    install_inner(
        project,
        lock,
        guard,
        options,
        InstallMetadata::Unchanged,
        None,
    )
}

/// Publish the locked tree and lockfile together, with an updated manifest when supplied.
/// The project lock must cover the manifest and lockfile reads used to produce these outputs.
pub fn install_and_save(
    project: &Path,
    lock: &Lockfile,
    guard: &InstallGuard,
    options: &InstallOptions,
    manifest: Option<&Manifest>,
) -> Result<()> {
    install_inner(
        project,
        lock,
        guard,
        options,
        InstallMetadata::Save(manifest),
        None,
    )
}

/// Project metadata to publish with the installed tree.
pub enum InstallMetadata<'a> {
    /// Publish only node_modules.
    Unchanged,
    /// Publish the lockfile and optionally a replacement manifest.
    Save(Option<&'a Manifest>),
}

/// Verify the complete staged project before making any of it active.
pub fn install_verified(
    project: &Path,
    lock: &Lockfile,
    guard: &InstallGuard,
    options: &InstallOptions,
    metadata: InstallMetadata<'_>,
    verify_tree: &VerifyTree<'_>,
) -> Result<()> {
    install_inner(project, lock, guard, options, metadata, Some(verify_tree))
}

fn install_inner(
    project: &Path,
    lock: &Lockfile,
    guard: &InstallGuard,
    options: &InstallOptions,
    metadata: InstallMetadata<'_>,
    verify_tree: Option<&VerifyTree<'_>>,
) -> Result<()> {
    if options
        .config
        .is_some_and(|config| &lock.registry_sources != config.sources())
    {
        bail!("registry configuration differs from lockfile; run jetbunny install to re-resolve");
    }
    if project.canonicalize()? != guard.project {
        bail!("install guard belongs to a different project");
    }
    let project = guard.project.as_path();
    transaction::recover(project)?;
    lock.validate()?;
    let (os, cpu) = current_platform();
    let operation = (|| -> Result<()> {
        let staged = transaction::begin(project)?;
        let staged_modules = staged.join(Output::Modules.filename());
        fs::create_dir(&staged_modules)?;
        let downloader = Downloader::new(options.transport, options.config);
        // Parents before their nested packages, so a skipped parent skips its subtree.
        let mut packages = lock.packages.iter().collect::<Vec<_>>();
        packages.sort_by_key(|(location, _)| location.matches("node_modules/").count());
        let mut skipped = BTreeSet::<String>::new();
        let inside_skipped = |skipped: &BTreeSet<String>, location: &str| {
            skipped
                .iter()
                .any(|s| location.starts_with(&format!("{s}/")))
        };
        let mut wanted = Vec::new();
        for (location, package) in packages {
            if inside_skipped(&skipped, location) {
                continue;
            }
            if !compatible_platform(&package.os, os) || !compatible_platform(&package.cpu, cpu) {
                if !package.optional {
                    bail!(
                        "{}@{} does not support {os}/{cpu}",
                        package.name,
                        package.version
                    );
                }
                skipped.insert(location.clone());
                continue;
            }
            wanted.push((location, package));
        }
        let mut installed = BTreeSet::new();
        let mut failures = Vec::new();
        let mut budget = ExtractionBudget::default();
        for batch in wanted.chunks(DOWNLOAD_BATCH) {
            let records: Vec<&LockedPackage> = batch.iter().map(|(_, package)| *package).collect();
            let contents = downloader.get_all(&records);
            for ((location, package), content) in batch.iter().zip(contents) {
                if inside_skipped(&skipped, location) {
                    continue;
                }
                let label = format!("{}@{}", package.name, package.version);
                let package_dir = staged.join(location);
                let mut created = false;
                let unpacked = content.and_then(|content| {
                    fs::create_dir_all(package_dir.parent().context("missing package scope")?)?;
                    fs::create_dir(&package_dir)?;
                    created = true;
                    extract_tgz(&content, &package_dir, &mut budget)
                        .with_context(|| format!("cannot unpack {label}"))?;
                    let manifest_path = package_dir.join("package.json");
                    let manifest = read_manifest(&manifest_path)
                        .with_context(|| format!("cannot read package.json in {label}"))?;
                    let data: serde_json::Value = serde_json::from_slice(&manifest)?;
                    if data["name"].as_str() != Some(package.name.as_str())
                        || data["version"].as_str() != Some(package.version.as_str())
                    {
                        bail!("tarball package identity mismatch for {label}");
                    }
                    validate_bin_targets(&package_dir, package)?;
                    if let Some(verify) = options.verify {
                        verify(&package_dir, package)
                            .with_context(|| format!("{label} at {location} cannot be used"))?;
                    }
                    Ok(())
                });
                match unpacked {
                    Ok(()) => {
                        installed.insert(location.as_str());
                    }
                    Err(error) if error.is::<InstallBudgetExceeded>() => return Err(error),
                    Err(error) => {
                        if created {
                            fs::remove_dir_all(&package_dir)?;
                        }
                        failures.push((location.as_str(), error.context(label)));
                        skipped.insert((*location).clone());
                    }
                }
            }
        }
        let surviving = graph::surviving(lock, &installed);
        if let Some(location) = surviving.missing_required {
            let problem = lock
                .validate_installed(
                    installed
                        .iter()
                        .map(|path| lock.packages.get_key_value(*path).unwrap().0),
                )
                .err()
                .unwrap_or_else(|| anyhow!("required locked package {location} is missing"));
            if let Some((_, error)) = failures.into_iter().find(|(path, _)| *path == location) {
                return Err(problem.context(format!("{error:#}")));
            }
            return Err(problem);
        }
        for (_, error) in failures {
            eprintln!("warning: skipping optional dependency branch: {error:#}");
        }
        let removed: Vec<_> = installed
            .difference(&surviving.locations)
            .copied()
            .collect();
        for location in removed.into_iter().rev() {
            fs::remove_dir_all(staged.join(location))?;
        }
        let mut installed_lock = lock.clone();
        installed_lock
            .packages
            .retain(|location, _| surviving.locations.contains(location.as_str()));
        lock.validate_installed(installed_lock.packages.keys())?;
        for (location, package) in &installed_lock.packages {
            create_bin_links(&staged, location, package)?;
        }
        let mut outputs = vec![Output::Modules];
        let staged_manifest = staged.join(Output::Manifest.filename());
        if let InstallMetadata::Save(manifest) = metadata {
            if let Some(manifest) = manifest {
                manifest.write(&staged_manifest)?;
                outputs.push(Output::Manifest);
            }
            lock.write(&staged.join(Output::Lockfile.filename()))?;
            outputs.push(Output::Lockfile);
        }
        if let Some(verify) = verify_tree {
            if !outputs.contains(&Output::Manifest) {
                fs::write(
                    &staged_manifest,
                    read_manifest(&project.join("package.json"))?,
                )?;
            }
            verify(&staged, &installed_lock)?;
        }
        transaction::publish(project, &staged, &outputs)
    })();
    let recovered = transaction::recover(project);
    match (operation, recovered) {
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(recovery_error)) => {
            Err(error).context(format!("rollback/recovery also failed: {recovery_error:#}"))
        }
        (Ok(()), Err(error)) => {
            eprintln!("warning: install committed, but transaction cleanup failed: {error:#}");
            Ok(())
        }
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn read_manifest(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_MANIFEST + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MANIFEST {
        bail!("package.json exceeds its size limit");
    }
    Ok(bytes)
}

fn validate_bin_targets(package_dir: &Path, package: &LockedPackage) -> Result<()> {
    for member in package.bin.values() {
        // Both shims must be able to represent the target.
        validate_cmd_argument(member)?;
        if !package_dir.join(member).is_file() {
            bail!("references missing binary {member}");
        }
    }
    Ok(())
}

/// Writes `node_modules/.bin` shims that run the package's bin under jetbunny:
/// `jetbunny run --root <project> --project <package location> --entry <bin path> --debug disabled
/// --status none`. A bin is a tool run, not a debugging session, so it does not ask for the
/// development debugger, and its output is its program's alone, without the status line.
/// Shims rather than symlinks, because a bin's `#!/usr/bin/env node` line names another
/// runtime. The project root is found from the shim's own location at run time.
fn create_bin_links(staged: &Path, location: &str, package: &LockedPackage) -> Result<()> {
    if package.bin.is_empty() {
        return Ok(());
    }
    // The node_modules directory holding this package, relative to the project root.
    let modules = location
        .strip_suffix(&package.name)
        .and_then(|m| m.strip_suffix('/'))
        .ok_or_else(|| anyhow!("cannot locate node_modules directory for {location}"))?;
    let links_dir = staged.join(modules).join(".bin");
    fs::create_dir_all(&links_dir)?;
    // From <modules>/.bin back to the project root.
    let up = vec![".."; modules.split('/').count() + 1].join("/");
    let cmd_up = up.replace('/', "\\");
    let shell_project = quote_shell_argument(location);
    let cmd_project = quote_cmd_argument(location)?;
    for (bin_name, member) in &package.bin {
        let relative_member = member.strip_prefix("./").unwrap_or(member);
        // A relative path prefix keeps names starting with "--" from looking like options.
        let entry = format!("./{relative_member}");
        let shell_entry = quote_shell_argument(&entry);
        let cmd_entry = quote_cmd_argument(&entry)?;
        let shim = links_dir.join(bin_name);
        let cmd = links_dir.join(format!("{bin_name}.cmd"));
        if shim.symlink_metadata().is_ok() || cmd.symlink_metadata().is_ok() {
            bail!("binary name conflict: {} (from {})", bin_name, package.name);
        }
        let mut file = File::create(&shim)?;
        write!(file, "#!/bin/sh\n\
            command -v jetbunny >/dev/null 2>&1 || {{ echo \"jetbunny not found on PATH\" >&2; exit 127; }}\n\
            root=\"$(CDPATH= cd \"$(dirname -- \"$0\")/{up}\" && pwd)\" || exit 1\n\
            exec jetbunny run --root \"$root\" --project {shell_project} --entry {shell_entry} --debug disabled --status none -- \"$@\"\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))?;
        }
        let mut file = File::create(&cmd)?;
        write!(file, "@echo off\r\n\
            setlocal EnableExtensions DisableDelayedExpansion\r\n\
            where jetbunny >nul 2>&1 || (echo jetbunny not found on PATH 1>&2 & exit /b 127)\r\n\
            jetbunny run --root \"%~dp0{cmd_up}\" --project {cmd_project} --entry {cmd_entry} --debug disabled --status none -- %*\r\n")?;
    }
    Ok(())
}

fn quote_shell_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn quote_cmd_argument(value: &str) -> Result<String> {
    validate_cmd_argument(value)?;
    Ok(format!("\"{value}\""))
}

fn validate_cmd_argument(value: &str) -> Result<()> {
    // Double quotes protect whitespace and operators. Percent expansion still
    // happens inside quotes, and control characters can break the command line.
    if value
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '"' | '%'))
    {
        bail!("cannot represent binary shim argument in a .cmd file: {value:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        lockfile::LOCK_NAME,
        transport::{Request, Response},
    };

    struct NoNetwork;

    impl Transport for NoNetwork {
        fn fetch(&self, _: Vec<Request>) -> Vec<Result<Response>> {
            panic!("this install should not fetch packages")
        }
    }

    #[test]
    fn a_missing_bin_target_is_handled_as_a_package_failure() {
        use base64::{engine::general_purpose::STANDARD, Engine};

        struct PackageTarball(Vec<u8>);

        impl Transport for PackageTarball {
            fn fetch(&self, requests: Vec<Request>) -> Vec<Result<Response>> {
                requests
                    .into_iter()
                    .map(|_| {
                        Ok(Response {
                            status: 200,
                            location: None,
                            body: self.0.clone(),
                        })
                    })
                    .collect()
            }
        }

        let transport = PackageTarball(tarball(vec![(
            "package/package.json",
            br#"{"name":"addon","version":"1.0.0","bin":{"addon":"missing.js"}}"#,
        )]));
        let integrity = format!("sha256-{}", STANDARD.encode(Sha256::digest(&transport.0)));
        let mut lock: Lockfile = serde_json::from_value(serde_json::json!({
            "lockfileVersion": crate::lockfile::LOCK_VERSION,
            "dependencies": {},
            "devDependencies": {},
            "optionalDependencies": {"addon": "1.0.0"},
            "packages": {
                "node_modules/addon": {
                    "name": "addon", "version": "1.0.0", "optional": true,
                    "resolved": "https://registry.example/addon.tgz",
                    "integrity": integrity, "bin": {"addon": "missing.js"}
                }
            }
        }))
        .unwrap();
        let project = tempfile::tempdir().unwrap();
        install(project.path(), &lock, &transport).unwrap();
        assert!(!project.path().join("node_modules/addon").exists());
        assert!(!project.path().join("node_modules/.bin").exists());

        lock.dependencies = std::mem::take(&mut lock.optional_dependencies);
        lock.packages
            .get_mut("node_modules/addon")
            .unwrap()
            .optional = false;
        let marker = project.path().join("node_modules/original.txt");
        fs::write(&marker, "original").unwrap();
        let error = install(project.path(), &lock, &transport).unwrap_err();
        assert!(format!("{error:#}").contains("references missing binary missing.js"));
        assert_eq!(fs::read_to_string(marker).unwrap(), "original");
    }

    #[test]
    fn a_skipped_required_peer_preserves_the_existing_tree() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        fs::create_dir(project.join("node_modules")).unwrap();
        fs::write(project.join("node_modules/old.txt"), b"original").unwrap();
        let lock = serde_json::from_value(serde_json::json!({
            "lockfileVersion": crate::lockfile::LOCK_VERSION,
            "dependencies": {},
            "devDependencies": {},
            "optionalDependencies": {"provider": "^1"},
            "peerDependencies": {"provider": "^1"},
            "packages": {
                "node_modules/provider": {
                    "name": "provider", "version": "1.0.0", "optional": true,
                    "os": ["fixture-os"], "resolved": "https://registry.example/provider.tgz",
                    "integrity": "sha1-AAAAAAAAAAAAAAAAAAAAAAAAAAA="
                }
            }
        }))
        .unwrap();
        let error = install(project, &lock, &NoNetwork).unwrap_err();
        assert!(format!("{error:#}").contains("missing required peer provider"));
        assert_eq!(
            fs::read(project.join("node_modules/old.txt")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn an_invalid_metadata_destination_preserves_the_manifest_and_tree() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        fs::create_dir(project.join("node_modules")).unwrap();
        fs::write(project.join("node_modules/old.txt"), b"original").unwrap();
        fs::write(project.join("package.json"), b"{\"name\":\"original\"}").unwrap();
        fs::create_dir(project.join(LOCK_NAME)).unwrap();
        let lock = Lockfile::empty(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
        );
        let guard = InstallGuard::acquire(project).unwrap();
        let options = InstallOptions {
            transport: &NoNetwork,
            config: None,
            verify: None,
        };
        let error = install_and_save(
            project,
            &lock,
            &guard,
            &options,
            Some(&Manifest::started(project)),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("cannot replace directory"));
        assert_eq!(
            fs::read(project.join("node_modules/old.txt")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(project.join("package.json")).unwrap(),
            b"{\"name\":\"original\"}"
        );
        assert!(project.join(LOCK_NAME).is_dir());
    }

    #[test]
    fn exclusive_lock_rejects_concurrent_project_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let first = InstallGuard::acquire(dir.path()).unwrap();
        assert!(InstallGuard::acquire(dir.path()).is_err());
        drop(first);
        InstallGuard::acquire(dir.path()).unwrap();
    }

    #[test]
    fn rejects_unsafe_member_paths() {
        for path in [
            "../outside",
            "/root/.ssh",
            "a/../../b",
            "C:\\outside",
            "file:stream",
            "",
        ] {
            assert!(validate_member_path(path).is_err(), "accepted {path:?}");
        }
        assert!(validate_member_path("./bin/cli.js").is_ok());
    }

    #[test]
    fn refuses_tarball_traversal_and_links() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tarball(vec![("package/ok.txt", b"safe")]);
        unpack_tgz(&data, tmp.path()).unwrap();
        assert_eq!(fs::read(tmp.path().join("ok.txt")).unwrap(), b"safe");
        // A tar archive with an external top-level prefix must be rejected.
        let outside = tarball(vec![("not-package/file", b"bad")]);
        assert!(unpack_tgz(&outside, tmp.path()).is_err());
    }

    fn tarball(files: Vec<(&str, &[u8])>) -> Vec<u8> {
        use flate2::{write::GzEncoder, Compression};
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, name, data).unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn bin_shims_quote_paths_with_spaces_and_apostrophes() {
        let staged = tempfile::tempdir().unwrap();
        let location = "node_modules/@demo/tool";
        let package_dir = staged.path().join(location);
        fs::create_dir_all(package_dir.join("bin")).unwrap();
        fs::write(package_dir.join("bin/tool's entry.js"), b"").unwrap();
        let package = serde_json::from_value(serde_json::json!({
            "name": "@demo/tool",
            "version": "1.0.0",
            "resolved": "https://registry.npmjs.org/tool.tgz",
            "integrity": "",
            "bin": {"tool": "bin/tool's entry.js"}
        }))
        .unwrap();
        create_bin_links(staged.path(), location, &package).unwrap();

        let bin_dir = staged.path().join("node_modules/.bin");
        let shell = fs::read_to_string(bin_dir.join("tool")).unwrap();
        assert!(shell.contains("--project 'node_modules/@demo/tool'"));
        assert!(shell.contains("--entry './bin/tool'\\''s entry.js'"));
        let cmd = fs::read_to_string(bin_dir.join("tool.cmd")).unwrap();
        assert!(cmd.contains("setlocal EnableExtensions DisableDelayedExpansion"));
        assert!(cmd.contains("--project \"node_modules/@demo/tool\""));
        assert!(cmd.contains("--entry \"./bin/tool's entry.js\""));
    }

    #[test]
    fn cmd_arguments_reject_quotes_expansion_and_control_characters() {
        for value in ["one\"name", "one%name", "one\nname"] {
            assert!(quote_cmd_argument(value).is_err());
        }
    }
}
