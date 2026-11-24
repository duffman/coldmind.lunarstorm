//! Owned, restartable recovery for project files published by an install.

use crate::lockfile::{atomic_json_write, LOCK_NAME};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::ErrorKind,
    path::{Path, PathBuf},
};

const STAGING: &str = ".rustnpm-staging";
const BACKUP: &str = ".rustnpm-backup";
const JOURNAL: &str = ".rustnpm-transaction.json";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Output {
    Modules,
    Manifest,
    Lockfile,
}

impl Output {
    pub(super) fn filename(self) -> &'static str {
        match self {
            Self::Modules => "node_modules",
            Self::Manifest => "package.json",
            Self::Lockfile => LOCK_NAME,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Identity {
    device: u64,
    file: u64,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    output: Output,
    original: Option<Identity>,
    replacement: Identity,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum Phase {
    Preparing,
    Publishing { entries: Vec<Entry> },
    Committed { entries: Vec<Entry> },
}

/// Only this per-user record authorizes recovery. The checkout journal is a locator.
#[derive(Serialize, Deserialize)]
struct Transaction {
    version: u32,
    id: String,
    project: PathBuf,
    project_identity: Identity,
    staged: Option<Identity>,
    backup: Option<Identity>,
    phase: Phase,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    id: String,
}

fn state_path(project: &Path) -> Result<PathBuf> {
    #[cfg(test)]
    let base = std::env::temp_dir().join("jetpack-transaction-tests");
    #[cfg(not(test))]
    let base = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .context("cannot locate per-user transaction state")?
        .join("jetpack")
        .join("transactions");
    let key = Sha256::digest(serde_json::to_vec(&(project, identity(project)?))?);
    Ok(base.join(format!("{}.json", hex::encode(key))))
}

fn record(project: &Path, transaction: &Transaction) -> Result<()> {
    let path = state_path(project)?;
    let parent = path.parent().context("transaction state has no parent")?;
    fs::create_dir_all(parent)?;
    ensure_directory(parent)?;
    if parent.canonicalize()?.starts_with(project.canonicalize()?) {
        bail!("per-user jetpack transaction state must be outside the project");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    atomic_json_write(&path, transaction)
}

fn load(project: &Path) -> Result<Transaction> {
    let path = state_path(project)?;
    ensure_regular(&path)?;
    let transaction: Transaction = serde_json::from_slice(&fs::read(&path)?)
        .context("damaged per-user jetpack transaction state")?;
    if transaction.version != 3
        || transaction.project != project.canonicalize()?
        || transaction.project_identity != identity(project)?
    {
        bail!("jetpack transaction belongs to another project; preserve files for inspection");
    }
    Ok(transaction)
}

pub(super) fn begin(project: &Path) -> Result<PathBuf> {
    recover(project)?;
    let mut transaction = Transaction {
        version: 3,
        id: format!(
            "{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ),
        project: project.canonicalize()?,
        project_identity: identity(project)?,
        staged: None,
        backup: None,
        phase: Phase::Preparing,
    };
    record(project, &transaction)?;
    atomic_json_write(
        &project.join(JOURNAL),
        &Journal {
            version: 3,
            id: transaction.id.clone(),
        },
    )?;
    let staged = project.join(STAGING);
    fs::create_dir(&staged)?;
    transaction.staged = Some(identity(&staged)?);
    record(project, &transaction)?;
    sync_directory(project)?;
    Ok(staged)
}

pub(super) fn publish(project: &Path, staged: &Path, outputs: &[Output]) -> Result<()> {
    let mut transaction = load(project)?;
    validate_roots(project, &transaction)?;
    if staged != project.join(STAGING) || !matches!(transaction.phase, Phase::Preparing) {
        bail!("invalid jetpack publication state");
    }
    let mut entries = Vec::new();
    for &output in outputs {
        if entries.iter().any(|entry: &Entry| entry.output == output) {
            bail!("duplicate jetpack publication output");
        }
        let original = optional_identity(&project.join(output.filename()))?;
        if output != Output::Modules {
            if original.is_some() {
                let target = project.join(output.filename());
                if fs::symlink_metadata(&target)?.is_dir() {
                    bail!("cannot replace directory {} with a file", target.display());
                }
                ensure_regular(&target)?;
            }
            ensure_regular(&staged.join(output.filename()))?;
        }
        entries.push(Entry {
            output,
            original,
            replacement: identity(&staged.join(output.filename()))?,
        });
    }
    sync_tree(staged)?;
    let backup = project.join(BACKUP);
    fs::create_dir(&backup)?;
    transaction.backup = Some(identity(&backup)?);
    transaction.phase = Phase::Publishing { entries };
    record(project, &transaction)?;
    sync_directory(project)?;
    for &output in outputs {
        let filename = output.filename();
        let target = project.join(filename);
        if optional_identity(&target)?.is_some() {
            rename(&target, &backup.join(filename))?;
            sync_directory(&backup)?;
            sync_directory(project)?;
        }
        rename(&staged.join(filename), &target)?;
        sync_directory(project)?;
        sync_directory(staged)?;
    }
    let Phase::Publishing { entries } = transaction.phase else {
        unreachable!("publication retains its output entries");
    };
    transaction.phase = Phase::Committed { entries };
    record(project, &transaction)
}

fn rename(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to)
        .with_context(|| format!("cannot move {} to {}", from.display(), to.display()))
}

fn optional_identity(path: &Path) -> Result<Option<Identity>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_link(&metadata) => {
            bail!("jetpack recovery refuses symbolic link {}", path.display())
        }
        Ok(_) => identity(path).map(Some),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot inspect {}", path.display())),
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

fn ensure_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || is_link(&metadata) {
        bail!("expected an ordinary directory at {}", path.display());
    }
    Ok(())
}

fn ensure_regular(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || is_link(&metadata) {
        bail!("expected an ordinary file at {}", path.display());
    }
    Ok(())
}

fn remove(path: &Path) -> Result<()> {
    if optional_identity(path)?.is_none() {
        return Ok(());
    }
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn validate_roots(project: &Path, transaction: &Transaction) -> Result<()> {
    for (name, expected) in [
        (STAGING, &transaction.staged),
        (BACKUP, &transaction.backup),
    ] {
        let root = project.join(name);
        let actual = optional_identity(&root)?;
        if actual.is_some() && actual != *expected {
            bail!(
                "unowned jetpack transaction directory {}; preserve files for inspection",
                root.display()
            );
        }
        if actual.is_some() {
            ensure_directory(&root)?;
        }
    }
    Ok(())
}

/// Validate the whole rollback before touching any output. Identities allow a restart
/// after restoring an original and before recording that restoration.
fn validate_entries(project: &Path, entries: &[Entry], rollback: bool) -> Result<()> {
    let mut seen = Vec::new();
    for entry in entries {
        if seen.contains(&entry.output) {
            bail!("duplicate output in jetpack recovery state");
        }
        seen.push(entry.output);
        let filename = entry.output.filename();
        let target = optional_identity(&project.join(filename))?;
        let backup = optional_identity(&project.join(BACKUP).join(filename))?;
        let staged = optional_identity(&project.join(STAGING).join(filename))?;
        if backup.is_some() && backup != entry.original
            || staged.is_some() && staged.as_ref() != Some(&entry.replacement)
            || target.is_some()
                && target != entry.original
                && target.as_ref() != Some(&entry.replacement)
            || rollback
                && entry.original.is_some()
                && target != entry.original
                && backup != entry.original
        {
            bail!("jetpack output {filename} changed outside its transaction; preserve files for inspection");
        }
    }
    Ok(())
}

/// The caller holds the project lock before examining or modifying transaction state.
pub(super) fn recover(project: &Path) -> Result<()> {
    let journal = project.join(JOURNAL);
    let state = state_path(project)?;
    let local = optional_identity(&journal)?;
    let external = optional_identity(&state)?;
    if local.is_none() && external.is_none() {
        if optional_identity(&project.join(STAGING))?.is_some()
            || optional_identity(&project.join(BACKUP))?.is_some()
        {
            bail!("unowned jetpack transaction paths; preserve files for inspection");
        }
        return Ok(());
    }
    if external.is_none() {
        bail!("jetpack journal has no per-user ownership record; preserve files for inspection");
    }
    let mut transaction = load(project)?;
    if local.is_some() {
        ensure_regular(&journal)?;
        let locator: Journal = serde_json::from_slice(&fs::read(&journal)?)
            .context("unowned or legacy jetpack journal; preserve files for inspection")?;
        if locator.version != 3 || locator.id != transaction.id {
            bail!("jetpack journal ownership mismatch; preserve files for inspection");
        }
    } else if !matches!(
        transaction.phase,
        Phase::Preparing | Phase::Committed { .. }
    ) {
        bail!("missing jetpack journal during publication; preserve files for inspection");
    }
    validate_roots(project, &transaction)?;
    if let Phase::Publishing { entries } | Phase::Committed { entries } = &transaction.phase {
        validate_entries(
            project,
            entries,
            matches!(transaction.phase, Phase::Publishing { .. }),
        )?;
        if project.join(BACKUP).exists() {
            for child in fs::read_dir(project.join(BACKUP))? {
                let child = child?;
                if !entries
                    .iter()
                    .any(|entry| child.file_name() == entry.output.filename())
                {
                    bail!("unexpected file in jetpack backup; preserve files for inspection");
                }
            }
        }
    }
    if let Phase::Committed { entries } = &transaction.phase {
        for entry in entries {
            if optional_identity(&project.join(entry.output.filename()))?.as_ref()
                != Some(&entry.replacement)
            {
                bail!("committed jetpack output changed; preserve files for inspection");
            }
        }
    }
    if let Phase::Publishing { entries } = &transaction.phase {
        for entry in entries.iter().rev() {
            let filename = entry.output.filename();
            let target = project.join(filename);
            let original = project.join(BACKUP).join(filename);
            if optional_identity(&original)?.is_some() {
                remove(&target)?;
                rename(&original, &target)?;
                sync_directory(&project.join(BACKUP))?;
            } else if entry.original.is_none() {
                remove(&target)?;
            }
            sync_directory(project)?;
        }
        transaction.phase = Phase::Preparing;
        record(project, &transaction)?;
    }
    remove(&project.join(STAGING))?;
    remove(&project.join(BACKUP))?;
    sync_directory(project)?;
    remove(&journal)?;
    sync_directory(project)?;
    remove(&state)?;
    sync_directory(state.parent().context("missing transaction state parent")?)
}

/// Flush file contents before the commit record can outlive staged payloads.
fn sync_tree(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if is_link(&metadata) {
        bail!("symbolic link in staged installation: {}", path.display());
    }
    if metadata.is_file() {
        return fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?
            .sync_all()
            .map_err(Into::into);
    }
    ensure_directory(path)?;
    for entry in fs::read_dir(path)? {
        sync_tree(&entry?.path())?;
    }
    sync_directory(path)
}

#[cfg(unix)]
fn identity(path: &Path) -> Result<Identity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path)?;
    Ok(Identity {
        device: metadata.dev(),
        file: metadata.ino(),
    })
}

#[cfg(windows)]
fn identity(path: &Path) -> Result<Identity> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    #[repr(C)]
    struct FileInformation {
        attributes: u32,
        creation: [u32; 2],
        access: [u32; 2],
        write: [u32; 2],
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(
            handle: *mut std::ffi::c_void,
            info: *mut FileInformation,
        ) -> i32;
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(0x02000000 | 0x00200000)
        .open(path)?;
    let mut information = std::mem::MaybeUninit::<FileInformation>::uninit();
    // SAFETY: the live file owns the handle and the output points to a correctly sized structure.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: a successful call initialized the complete output structure.
    let information = unsafe { information.assume_init() };
    Ok(Identity {
        device: information.volume.into(),
        file: (u64::from(information.index_high) << 32) | u64::from(information.index_low),
    })
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<()> {
    File::open(directory)?.sync_all().map_err(Into::into)
}

// Windows flushes payload files, but directory rename durability is filesystem-dependent.
// Recovery remains guarded by stable volume/file IDs; power-loss durability is Unix-only.
#[cfg(not(unix))]
fn sync_directory(_: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_journal_preserves_existing_outputs() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), b"original").unwrap();
        fs::write(dir.path().join(JOURNAL), br#"{"version":1}"#).unwrap();
        assert!(recover(dir.path()).is_err());
        assert_eq!(
            fs::read(dir.path().join("package.json")).unwrap(),
            b"original"
        );
        assert!(dir.path().join(JOURNAL).exists());
    }

    #[test]
    fn refuses_unowned_backup_without_a_journal() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(BACKUP)).unwrap();
        assert!(recover(dir.path()).is_err());
        assert!(dir.path().join(BACKUP).exists());
    }

    #[test]
    fn copied_locator_does_not_authorize_recovery() {
        let original = tempfile::tempdir().unwrap();
        let copy = tempfile::tempdir().unwrap();
        begin(original.path()).unwrap();
        fs::copy(original.path().join(JOURNAL), copy.path().join(JOURNAL)).unwrap();
        fs::write(copy.path().join("package.json"), b"keep").unwrap();
        assert!(recover(copy.path()).is_err());
        assert_eq!(fs::read(copy.path().join("package.json")).unwrap(), b"keep");
        recover(original.path()).unwrap();
    }

    fn prepare_outputs(project: &Path) -> (PathBuf, Vec<Entry>) {
        let staged = begin(project).unwrap();
        fs::create_dir(staged.join("node_modules")).unwrap();
        fs::write(staged.join("node_modules/new.txt"), b"new").unwrap();
        fs::write(staged.join("package.json"), b"new manifest").unwrap();
        fs::write(staged.join(LOCK_NAME), b"new lockfile").unwrap();
        fs::create_dir(project.join("node_modules")).unwrap();
        fs::write(project.join("node_modules/old.txt"), b"original").unwrap();
        fs::write(project.join("package.json"), b"original manifest").unwrap();
        let entries = [Output::Modules, Output::Manifest, Output::Lockfile]
            .into_iter()
            .map(|output| Entry {
                output,
                original: optional_identity(&project.join(output.filename())).unwrap(),
                replacement: identity(&staged.join(output.filename())).unwrap(),
            })
            .collect();
        (staged, entries)
    }

    fn start_publishing(project: &Path, entries: Vec<Entry>) {
        let mut transaction = load(project).unwrap();
        fs::create_dir(project.join(BACKUP)).unwrap();
        transaction.backup = Some(identity(&project.join(BACKUP)).unwrap());
        transaction.phase = Phase::Publishing { entries };
        record(project, &transaction).unwrap();
    }

    #[test]
    fn rollback_restores_originals_after_each_publication_move() {
        let outputs = [Output::Modules, Output::Manifest, Output::Lockfile];
        for moves in 0..=5 {
            let dir = tempfile::tempdir().unwrap();
            let project = dir.path();
            let (staged, entries) = prepare_outputs(project);
            start_publishing(project, entries);
            let mut completed = 0;
            for output in outputs {
                let filename = output.filename();
                if completed == moves {
                    break;
                }
                if project.join(filename).exists() {
                    rename(
                        &project.join(filename),
                        &project.join(BACKUP).join(filename),
                    )
                    .unwrap();
                    completed += 1;
                }
                if completed == moves {
                    break;
                }
                rename(&staged.join(filename), &project.join(filename)).unwrap();
                completed += 1;
            }
            recover(project).unwrap();
            recover(project).unwrap();
            assert_eq!(
                fs::read(project.join("node_modules/old.txt")).unwrap(),
                b"original"
            );
            assert_eq!(
                fs::read(project.join("package.json")).unwrap(),
                b"original manifest"
            );
            assert!(!project.join(LOCK_NAME).exists());
            assert!(!staged.exists());
        }
    }

    #[test]
    fn rollback_can_resume_after_an_original_was_restored() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let (staged, entries) = prepare_outputs(project);
        start_publishing(project, entries);
        for output in [Output::Modules, Output::Manifest, Output::Lockfile] {
            let filename = output.filename();
            if project.join(filename).exists() {
                rename(
                    &project.join(filename),
                    &project.join(BACKUP).join(filename),
                )
                .unwrap();
            }
            rename(&staged.join(filename), &project.join(filename)).unwrap();
        }
        remove(&project.join("package.json")).unwrap();
        rename(
            &project.join(BACKUP).join("package.json"),
            &project.join("package.json"),
        )
        .unwrap();
        recover(project).unwrap();
        assert_eq!(
            fs::read(project.join("package.json")).unwrap(),
            b"original manifest"
        );
        assert_eq!(
            fs::read(project.join("node_modules/old.txt")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn changed_output_prevents_all_rollback_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let (staged, entries) = prepare_outputs(project);
        start_publishing(project, entries);
        rename(
            &project.join("package.json"),
            &project.join(BACKUP).join("package.json"),
        )
        .unwrap();
        // A distinct file identity represents a caller's replacement after interruption.
        fs::write(project.join("package.json"), b"caller data").unwrap();
        assert!(recover(project).is_err());
        assert_eq!(
            fs::read(project.join("package.json")).unwrap(),
            b"caller data"
        );
        assert!(project.join("node_modules/old.txt").is_file());
        remove(&project.join("package.json")).unwrap();
        rename(&staged.join("package.json"), &project.join("package.json")).unwrap();
        recover(project).unwrap();
    }

    #[test]
    fn keeps_outputs_after_commit_and_cleans_external_state() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let (staged, _) = prepare_outputs(project);
        publish(
            project,
            &staged,
            &[Output::Modules, Output::Manifest, Output::Lockfile],
        )
        .unwrap();
        recover(project).unwrap();
        recover(project).unwrap();
        assert_eq!(
            fs::read(project.join("node_modules/new.txt")).unwrap(),
            b"new"
        );
        assert_eq!(
            fs::read(project.join("package.json")).unwrap(),
            b"new manifest"
        );
        assert!(!state_path(project).unwrap().exists());
    }

    #[test]
    fn committed_cleanup_can_resume_after_backup_removal() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let (staged, _) = prepare_outputs(project);
        publish(
            project,
            &staged,
            &[Output::Modules, Output::Manifest, Output::Lockfile],
        )
        .unwrap();
        remove(&staged).unwrap();
        remove(&project.join(BACKUP)).unwrap();
        remove(&project.join(JOURNAL)).unwrap();
        recover(project).unwrap();
        assert_eq!(
            fs::read(project.join("package.json")).unwrap(),
            b"new manifest"
        );
        assert!(!state_path(project).unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn substituted_recovery_root_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let project = dir.path();
        let staged = begin(project).unwrap();
        fs::remove_dir(&staged).unwrap();
        std::os::unix::fs::symlink(outside.path(), &staged).unwrap();
        fs::write(outside.path().join("keep.txt"), b"keep").unwrap();
        assert!(recover(project).is_err());
        assert!(outside.path().join("keep.txt").is_file());
        fs::remove_file(&staged).unwrap();
        recover(project).unwrap();
    }
}
