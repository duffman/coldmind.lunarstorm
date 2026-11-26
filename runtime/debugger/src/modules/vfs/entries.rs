//! One cached directory listing (port of Bun's `DirEntry` / `Entry`).
use std::collections::HashMap;
use std::io;

/// Entry kind. Symlinks are kept as symlinks; [`super::VirtualDir`] resolves them on use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Dir,
    Symlink,
}

/// Stored names of one directory and their kinds.
///
/// Keys are the names exactly as stored on disk, so a query that differs in case or Unicode
/// normalization misses even on insensitive volumes: one file has one module identity.
/// Names that cannot be module identities (non-UTF-8) and special files (devices, pipes,
/// sockets) are not recorded, as Bun's `add_entry` skips them.
pub(crate) struct DirEntries {
    names: HashMap<String, Option<Kind>>,
}

impl DirEntries {
    /// Reads `dir` (root-relative, `""` for the root) with one directory scan.
    ///
    /// Kinds come from the directory entry type where the platform reports it; an entry
    /// whose type is unavailable is recorded as unknown and stat'd lazily by its first user.
    pub(crate) fn read(root: &cap_std::fs::Dir, dir: &str) -> io::Result<Self> {
        let listing = if dir.is_empty() {
            root.entries()?
        } else {
            root.read_dir(dir)?
        };
        let mut names = HashMap::new();
        for entry in listing {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let kind = match entry.file_type() {
                Ok(t) if t.is_symlink() => Some(Kind::Symlink),
                Ok(t) if t.is_dir() => Some(Kind::Dir),
                Ok(t) if t.is_file() => Some(Kind::File),
                Ok(_) => continue,
                Err(_) => None,
            };
            names.insert(name, kind);
        }
        Ok(Self { names })
    }

    /// `None`: no entry with this exact stored name. `Some(None)`: present, kind unknown.
    pub(crate) fn get(&self, name: &str) -> Option<Option<Kind>> {
        self.names.get(name).copied()
    }

    /// Records the kind found by a lazy stat.
    pub(crate) fn set_kind(&mut self, name: &str, kind: Kind) {
        if let Some(slot) = self.names.get_mut(name) {
            *slot = Some(kind);
        }
    }

    /// Forgets an entry whose lazy stat found it gone or special.
    pub(crate) fn remove(&mut self, name: &str) {
        self.names.remove(name);
    }
}
