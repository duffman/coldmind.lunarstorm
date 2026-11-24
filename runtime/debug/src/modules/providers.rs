use super::vfs::VirtualDir;
use super::*;
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

/// Immutable explicit source registry with exact aliases; no implicit file access.
#[derive(Default)]
pub struct MemoryModuleProvider {
    sources: HashMap<ModuleKey, ScriptSource>,
    aliases: HashMap<(ModuleKey, JsString), ModuleKey>,
}
impl MemoryModuleProvider {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }
    /// Adds one immutable canonical source. Duplicate insertion preserves the old source.
    pub fn insert(&mut self, key: ModuleKey, source: ScriptSource) -> Result<(), ModuleError> {
        if self.sources.contains_key(&key) {
            return Err(ModuleError::InvalidIdentity);
        }
        self.sources.insert(key, source);
        Ok(())
    }
    /// Registers an exact referrer/specifier alias without changing existing bindings.
    pub fn alias(
        &mut self,
        referrer: ModuleKey,
        specifier: JsString,
        target: ModuleKey,
    ) -> Result<(), ModuleError> {
        let k = (referrer, specifier);
        if self.aliases.contains_key(&k) {
            return Err(ModuleError::InvalidIdentity);
        }
        self.aliases.insert(k, target);
        Ok(())
    }
}
impl ModuleProvider for MemoryModuleProvider {
    fn resolve(
        &self,
        referrer: &ModuleKey,
        specifier: &JsString,
    ) -> Result<ModuleKey, ModuleError> {
        if let Some(k) = self.aliases.get(&(referrer.clone(), specifier.clone())) {
            return Ok(k.clone());
        }
        let key = ModuleKey::new(specifier.clone())?;
        if self.sources.contains_key(&key) {
            Ok(key)
        } else {
            Err(ModuleError::NotFound)
        }
    }
    fn load(&self, request: &ModuleLoadRequest) -> Result<ModuleInput, ModuleError> {
        self.sources
            .get(&request.key)
            .cloned()
            .map(ModuleInput::Source)
            .ok_or(ModuleError::NotFound)
    }
}
/// File provider rooted in an already-open directory capability.
/// Symlinks and canonical aliases are resolved by capability-confined operations over one
/// cached snapshot of the tree: each directory is listed once, and a name missing from a
/// listing re-reads that directory once.
pub struct FileModuleProvider {
    files: Arc<VirtualDir>,
    aliases: HashMap<String, String>,
    max_source_bytes: usize,
}
impl FileModuleProvider {
    /// Takes an explicit directory authority and positive source-read bound.
    pub fn new(root: cap_std::fs::Dir, max_source_bytes: usize) -> Result<Self, ModuleError> {
        Self::with_virtual_dir(Arc::new(VirtualDir::new(root)), max_source_bytes)
    }
    /// Shares one snapshot with the platform package resolver.
    pub(crate) fn with_virtual_dir(
        files: Arc<VirtualDir>,
        max_source_bytes: usize,
    ) -> Result<Self, ModuleError> {
        if max_source_bytes == 0 {
            return Err(ModuleError::Limit);
        }
        Ok(Self {
            files,
            aliases: HashMap::new(),
            max_source_bytes,
        })
    }
    /// Adds an explicit bare-specifier alias to a confined relative file.
    pub fn alias(&mut self, name: String, path: String) -> Result<(), ModuleError> {
        checked_path(&path)?;
        if name.is_empty() || name.contains(['/', '\\', '\0']) || self.aliases.contains_key(&name) {
            return Err(ModuleError::InvalidIdentity);
        }
        self.aliases.insert(name, path);
        Ok(())
    }
    /// Resolves a root-relative entry without ambient filesystem operations.
    pub fn entry(&self, path: &str) -> Result<ModuleKey, ModuleError> {
        self.canonical(path)
    }
    fn canonical(&self, path: &str) -> Result<ModuleKey, ModuleError> {
        checked_path(path)?;
        let located = self.files.locate(path)?.ok_or(ModuleError::NotFound)?;
        if located.path.is_empty() {
            return Err(ModuleError::NotFound);
        }
        ModuleKey::from_utf8(&format!("file:///{}", located.path))
    }
}
fn checked_path(path: &str) -> Result<PathBuf, ModuleError> {
    if path.is_empty() || path.contains(['\\', '\0', ':', '%']) {
        return Err(ModuleError::InvalidIdentity);
    }
    let p = Path::new(path);
    if p.components().any(|c| {
        !matches!(
            c,
            Component::Normal(_) | Component::CurDir | Component::ParentDir
        )
    }) {
        return Err(ModuleError::InvalidIdentity);
    }
    Ok(p.to_owned())
}
impl ModuleProvider for FileModuleProvider {
    fn resolve(
        &self,
        referrer: &ModuleKey,
        specifier: &JsString,
    ) -> Result<ModuleKey, ModuleError> {
        let spec = specifier
            .to_utf8()
            .map_err(|_| ModuleError::InvalidIdentity)?;
        if let Some(path) = self.aliases.get(&spec) {
            return self.canonical(path);
        }
        if !spec.starts_with("./") && !spec.starts_with("../") {
            return Err(ModuleError::NotFound);
        }
        let base = referrer
            .text()
            .to_utf8()
            .map_err(|_| ModuleError::InvalidIdentity)?;
        let base = base
            .strip_prefix("file:///")
            .ok_or(ModuleError::InvalidIdentity)?;
        checked_path(&spec)?;
        // Module identities use '/' on every host. Host Path::join would introduce
        // Windows backslashes which are forbidden in external module specifiers.
        let parent = base.rsplit_once('/').map_or("", |(parent, _)| parent);
        let path = if parent.is_empty() {
            spec
        } else {
            format!("{parent}/{spec}")
        };
        // Relative specifiers are URL references: `..` removes the previous written segment,
        // not the physical parent of a symlink target.
        let mut segments = Vec::new();
        for segment in path.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    segments.pop().ok_or(ModuleError::InvalidIdentity)?;
                }
                s => segments.push(s),
            }
        }
        self.canonical(&segments.join("/"))
    }
    fn load(&self, request: &ModuleLoadRequest) -> Result<ModuleInput, ModuleError> {
        let key = request
            .key
            .text()
            .to_utf8()
            .map_err(|_| ModuleError::InvalidIdentity)?;
        let path = key.strip_prefix("file:///").ok_or(ModuleError::NotFound)?;
        checked_path(path)?;
        let bytes = self.files.read(path, self.max_source_bytes)?;
        if bytes.len() > self.max_source_bytes {
            return Err(ModuleError::Limit);
        }
        let source = String::from_utf8(bytes).map_err(|_| ModuleError::InvalidIdentity)?;
        Ok(ModuleInput::Source(ScriptSource::new(
            JsString::from_utf8(&source),
            key,
        )))
    }
}
