//! Path resolution over cached listings (port of Bun's `dir_info_cached` / `load_as_file`).
use super::entries::{DirEntries, Kind};
use crate::modules::ModuleError;
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::Mutex;

/// Bound on symlink traversals while resolving one path, as `ELOOP` would impose.
const MAX_SYMLINKS: usize = 40;
/// First read size; most sources fit, so no size query is needed (Bun reads 16 KiB first).
const INITIAL_READ: usize = 16 * 1024;

#[derive(Default)]
struct State {
    /// Root-relative real directory path (`""` is the root) → its listing.
    dirs: HashMap<String, DirEntries>,
    /// Root-relative symlink path → root-relative real target path.
    links: HashMap<String, String>,
    /// Directory scans performed, for regression tests of the one-listing-per-directory rule.
    #[cfg(test)]
    scans: usize,
}

/// One snapshot of a capability-confined tree. Every open stays beneath the root authority.
pub(crate) struct VirtualDir {
    root: cap_std::fs::Dir,
    state: Mutex<State>,
}

/// A resolved path: real, exactly spelled, root-relative (`""` is the root).
pub(crate) struct Located {
    pub(crate) path: String,
    pub(crate) kind: Kind,
}

impl VirtualDir {
    pub(crate) fn new(root: cap_std::fs::Dir) -> Self {
        Self {
            root,
            state: Mutex::default(),
        }
    }

    /// Resolves `path` to its real, exactly spelled location, following symlinks.
    ///
    /// `..` is physical: it leaves the directory a symlink resolved to, as `canonicalize`
    /// does. Escaping the root fails. A name missing from a cached listing re-reads that
    /// directory once before the path is reported absent.
    pub(crate) fn locate(&self, path: &str) -> Result<Option<Located>, ModuleError> {
        self.walk(path, true)
    }

    /// Kind of the real target of `path` from the snapshot, without re-reading listings.
    /// For probing optional locations such as `node_modules/<name>` in ancestors.
    pub(crate) fn probe(&self, path: &str) -> Result<Option<Kind>, ModuleError> {
        Ok(self.walk(path, false)?.map(|l| l.kind))
    }

    /// Reads at most `limit + 1` bytes of `path`; callers compare against `limit`.
    pub(crate) fn read(&self, path: &str, limit: usize) -> Result<Vec<u8>, ModuleError> {
        let file = self.root.open(path).map_err(io_error)?;
        let mut bytes = Vec::with_capacity(INITIAL_READ.min(limit.saturating_add(1)));
        file.take(limit.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        Ok(bytes)
    }

    /// Directory scans so far.
    #[cfg(test)]
    pub(crate) fn scans(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).scans
    }

    fn walk(&self, path: &str, refresh: bool) -> Result<Option<Located>, ModuleError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut pending: VecDeque<String> = components(path).collect();
        let mut stack: Vec<String> = Vec::new();
        let mut kind = Kind::Dir;
        let mut symlinks = 0;
        while let Some(name) = pending.pop_front() {
            // Only directories have children, including `..` (ENOTDIR otherwise).
            if kind != Kind::Dir {
                return Ok(None);
            }
            if name == ".." {
                if stack.pop().is_none() {
                    return Err(ModuleError::Io(
                        "a path led outside of the filesystem".into(),
                    ));
                }
                continue;
            }
            let dir = stack.join("/");
            let Some(found) = self.lookup(&mut state, &dir, &name, refresh)? else {
                return Ok(None);
            };
            if found == Kind::Symlink {
                symlinks += 1;
                if symlinks > MAX_SYMLINKS {
                    return Err(ModuleError::Io("too many levels of symbolic links".into()));
                }
                let target = self.link_target(&mut state, &join(&dir, &name))?;
                // The target is a root-relative real path: restart from the root with it,
                // then continue with what followed the link.
                for part in components(&target).collect::<Vec<_>>().into_iter().rev() {
                    pending.push_front(part);
                }
                stack.clear();
                kind = Kind::Dir;
                continue;
            }
            stack.push(name);
            kind = found;
        }
        Ok(Some(Located {
            path: stack.join("/"),
            kind,
        }))
    }

    /// Kind of `name` in real directory `dir`: listing first, lazy stat if unknown.
    fn lookup(
        &self,
        state: &mut State,
        dir: &str,
        name: &str,
        refresh: bool,
    ) -> Result<Option<Kind>, ModuleError> {
        if !state.dirs.contains_key(dir) {
            let entries = self.list(state, dir)?;
            state.dirs.insert(dir.to_owned(), entries);
        }
        let mut found = state.dirs[dir].get(name);
        if found.is_none() && refresh {
            let entries = self.list(state, dir)?;
            state.dirs.insert(dir.to_owned(), entries);
            found = state.dirs[dir].get(name);
        }
        match found {
            None => Ok(None),
            Some(Some(kind)) => Ok(Some(kind)),
            Some(None) => {
                let entries = state.dirs.get_mut(dir).expect("listed above");
                match self.root.symlink_metadata(join(dir, name)) {
                    Ok(m) => {
                        let kind = if m.is_symlink() {
                            Kind::Symlink
                        } else if m.is_dir() {
                            Kind::Dir
                        } else if m.is_file() {
                            Kind::File
                        } else {
                            entries.remove(name);
                            return Ok(None);
                        };
                        entries.set_kind(name, kind);
                        Ok(Some(kind))
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        entries.remove(name);
                        Ok(None)
                    }
                    Err(e) => Err(io_error(e)),
                }
            }
        }
    }

    fn list(&self, state: &mut State, dir: &str) -> Result<DirEntries, ModuleError> {
        #[cfg(test)]
        {
            state.scans += 1;
        }
        #[cfg(not(test))]
        let _ = state;
        DirEntries::read(&self.root, dir).map_err(|e| match e.kind() {
            std::io::ErrorKind::PermissionDenied => ModuleError::Io(format!(
                "directory not listable for exact-spelling check: {e}"
            )),
            _ => io_error(e),
        })
    }

    /// Real target of one symlink, confined to the root; resolved once per link.
    fn link_target(&self, state: &mut State, link: &str) -> Result<String, ModuleError> {
        if let Some(target) = state.links.get(link) {
            return Ok(target.clone());
        }
        let real = self.root.canonicalize(link).map_err(io_error)?;
        let real = real
            .to_str()
            .ok_or(ModuleError::InvalidIdentity)?
            .replace(std::path::MAIN_SEPARATOR, "/");
        let real = if real == "." { String::new() } else { real };
        state.links.insert(link.to_owned(), real.clone());
        Ok(real)
    }
}

fn components(path: &str) -> impl Iterator<Item = String> + '_ {
    path.split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .map(str::to_owned)
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

pub(crate) fn io_error(e: std::io::Error) -> ModuleError {
    if e.kind() == std::io::ErrorKind::NotFound {
        ModuleError::NotFound
    } else {
        ModuleError::Io(e.to_string())
    }
}
