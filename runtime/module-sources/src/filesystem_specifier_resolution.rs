//! Resolution and loading on the real filesystem: relative paths, `#private`
//! imports and packages in `node_modules`, and each file turned into an ES module.
//!
//! Decisions:
//! - Packages are found as Node finds them: `node_modules/<name>` in the importing
//!   file's directory and each ancestor. This works unchanged with pnpm's symlinked
//!   layout, because every resolved file is canonicalized and a dependency's own
//!   imports resolve from its real path.
//! - Builtins win over packages of the same name, as in Node; the caller supplies
//!   them, so this crate names no builtin.
//! - Relative imports keep the Runtime's probing (`.js`, `.mjs`, `/index.js`) and
//!   add TypeScript: `.ts`, `.mts`, `/index.ts`, and `./x.js` finding `./x.ts`
//!   (TypeScript sources import each other by their emitted names).
//! - A `.js` or `.ts` file whose nearest `package.json` has no `type` (or that has
//!   no `package.json` at all) is an ES module when it uses module syntax, else
//!   CommonJS: Node's syntax detection, which also keeps scripts without a
//!   `package.json` working as they did.

use crate::package_json_exports::{self, Manifest, Type};
use crate::load_time_esm_transforms::{self, FACADE, REQUIRE, Required};
use crate::{Error, Kind};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Suffixes an `import` of a relative path may omit, in order.
const IMPORT_PROBES: &[&str] = &["", ".js", ".mjs", ".ts", ".mts", "/index.js", "/index.ts"];

/// Suffixes a `require()`, or a package without `exports`, may omit, in order.
const REQUIRE_PROBES: &[&str] =
    &["", ".js", ".cjs", ".mjs", ".json", ".ts", ".cts", "/index.js", "/index.cjs", "/index.json", "/index.ts"];

/// Reads a file's bytes under the caller's policy.
pub type Read = Box<dyn Fn(&Path) -> Result<Vec<u8>, String>>;

/// Says whether a specifier names a builtin, and as what module name.
pub type Builtin<'a> = &'a dyn Fn(&str) -> Option<String>;

/// A package directory and its manifest.
struct Package {
    /// The directory holding `package.json`.
    directory: PathBuf,
    /// Its parsed manifest.
    manifest: Manifest,
}

/// Resolves and loads source modules. One per Runtime; it caches manifests and
/// file kinds, so a file that changes on disk keeps the kind it was first given.
pub struct Modules {
    /// Reads module files and `package.json` files.
    read: Read,
    /// The nearest package of each directory visited; `None` above every manifest.
    nearest: RefCell<HashMap<PathBuf, Option<Rc<Package>>>>,
    /// The kind of each file judged so far.
    kinds: RefCell<HashMap<PathBuf, Kind>>,
    /// Source maps of TypeScript modules, until [`Modules::source_map`] takes them.
    maps: RefCell<HashMap<String, String>>,
}

impl Modules {
    /// # Arguments
    /// * `read` - reads a file's bytes, refusing what the host's policy refuses.
    pub fn new(read: Read) -> Modules {
        Modules { read, nearest: RefCell::default(), kinds: RefCell::default(), maps: RefCell::default() }
    }

    /// Takes the source map of a module this loaded, for the debugger.
    ///
    /// # Arguments
    /// * `name` - the module's canonical name.
    ///
    /// # Returns
    /// Source Map v3 JSON from the module's JavaScript to its TypeScript; `None` for
    /// other kinds, and for CommonJS TypeScript, whose output is wrapped again.
    pub fn source_map(&self, name: &str) -> Option<String> {
        self.maps.borrow_mut().remove(name)
    }

    /// Strips types and keeps the map under `name`.
    ///
    /// # Errors
    /// As [`load_time_esm_transforms::strip_types`].
    fn strip_mapped(&self, name: &str, text: &str) -> Result<String, Error> {
        let (code, map) = load_time_esm_transforms::strip_types_mapped(name, text)?;
        if let Some(map) = map {
            self.maps.borrow_mut().insert(name.to_owned(), map);
        }
        Ok(code)
    }

    /// Resolves a specifier from an importing module.
    ///
    /// # Arguments
    /// * `base` - canonical name of the importing module: an absolute path, perhaps
    ///   with [`FACADE`].
    /// * `specifier` - as written; a [`REQUIRE`] prefix asks for `require()` rules.
    ///
    /// # Returns
    /// The canonical absolute path, with [`FACADE`] when an `import` reaches a
    /// CommonJS file.
    ///
    /// # Errors
    /// [`Error::NotFound`], or an invalid `package.json` on the way.
    pub fn resolve(&self, base: &str, specifier: &str) -> Result<String, Error> {
        let base = Path::new(base.strip_suffix(FACADE).unwrap_or(base));
        let directory = base.parent().unwrap_or(Path::new("/"));
        let (specifier, require) = match specifier.strip_prefix(REQUIRE) {
            Some(rest) => (rest, true),
            None => (specifier, false),
        };
        let conditions = if require { package_json_exports::REQUIRE } else { package_json_exports::IMPORT };
        let found = if is_path(specifier) {
            self.relative(directory, specifier, require)?
        } else if specifier.starts_with('#') {
            let package = self.nearest(directory)?.ok_or(Error::NotFound)?;
            let target = package.manifest.import_target(specifier, conditions)?;
            if target.starts_with("./") {
                exact(&package.directory.join(&target)).ok_or(Error::NotFound)?
            } else {
                self.package(directory, &target, conditions)?
            }
        } else {
            self.package(directory, specifier, conditions)?
        };
        let canonical = std::fs::canonicalize(&found).map_err(|_| Error::NotFound)?;
        let name = canonical.to_string_lossy().into_owned();
        Ok(if !require && self.kind(&canonical, None)?.commonjs() { name + FACADE } else { name })
    }

    /// Loads one module by its canonical name and turns it into an ES module.
    ///
    /// # Arguments
    /// * `name` - a name [`Modules::resolve`] produced.
    /// * `builtin` - maps a `require()` specifier to a builtin module name.
    ///
    /// # Errors
    /// The file cannot be read, or does not parse.
    pub fn load(&self, name: &str, builtin: Builtin<'_>) -> Result<Vec<u8>, Error> {
        let (path, facade) = match name.strip_suffix(FACADE) {
            Some(path) => (path, true),
            None => (name, false),
        };
        let bytes = (self.read)(Path::new(path)).map_err(Error::Invalid)?;
        self.source(Path::new(path), bytes, facade, builtin)
    }

    /// The source of an entry module the host handed over.
    ///
    /// # Arguments
    /// * `name` - its canonical name.
    /// * `code` - its bytes.
    /// * `file` - read from a file, so judged like any module (a CommonJS entry runs
    ///   its body at once); otherwise text, which is an ES module (TypeScript when its
    ///   name says so).
    /// * `builtin` - as for [`Modules::load`].
    ///
    /// # Errors
    /// The code does not parse.
    pub fn entry(&self, name: &str, code: Vec<u8>, file: bool, builtin: Builtin<'_>) -> Result<Vec<u8>, Error> {
        if file {
            let commonjs = self.kind(Path::new(name), Some(&code))?.commonjs();
            let mut source = self.source(Path::new(name), code, false, builtin)?;
            if commonjs {
                source.extend_from_slice(b"__lunarstorm_cjs();\n");
            }
            return Ok(source);
        }
        if [".ts", ".mts", ".cts"].iter().any(|ext| name.ends_with(ext)) {
            let text = utf8(name, code)?;
            return Ok(self.strip_mapped(name, &text)?.into_bytes());
        }
        Ok(code)
    }

    /// Turns one file's bytes into an ES module.
    ///
    /// # Arguments
    /// * `path` - the file.
    /// * `bytes` - its contents.
    /// * `facade` - the CommonJS facade is asked for, not the file's body.
    /// * `builtin` - as for [`Modules::load`].
    fn source(&self, path: &Path, bytes: Vec<u8>, facade: bool, builtin: Builtin<'_>) -> Result<Vec<u8>, Error> {
        let kind = self.kind(path, Some(&bytes))?;
        if facade && !kind.commonjs() {
            return Err(Error::NotFound);
        }
        if kind == Kind::Esm {
            return Ok(bytes);
        }
        let name = path.to_string_lossy();
        let text = utf8(&name, bytes)?;
        let code = match kind {
            Kind::TypeScript { commonjs: false } => self.strip_mapped(&name, &text)?,
            Kind::TypeScript { .. } => load_time_esm_transforms::strip_types(&name, &text)?,
            _ => text,
        };
        let module = match kind {
            Kind::Json => load_time_esm_transforms::json_module(&name, &code)?,
            Kind::TypeScript { commonjs: false } => code,
            _ if !facade => self.commonjs_body(&name, &code, builtin)?,
            _ => {
                // The facade: an `import` of a CommonJS file runs its body and re-exports.
                let file = name.rsplit_once('/').map_or(&*name, |(_, file)| file);
                let exports = load_time_esm_transforms::scan_commonjs(&name, &code)?.exports;
                load_time_esm_transforms::commonjs_facade(&format!("{REQUIRE}./{file}"), &exports)
            }
        };
        Ok(module.into_bytes())
    }

    /// The body module of a CommonJS file, with its literal requires bound.
    fn commonjs_body(&self, name: &str, code: &str, builtin: Builtin<'_>) -> Result<String, Error> {
        let scan = load_time_esm_transforms::scan_commonjs(name, code)?;
        let mut requires = Vec::new();
        for specifier in scan.requires {
            if !is_path(&specifier)
                && !specifier.starts_with('#')
                && let Some(module) = builtin(&specifier)
            {
                requires.push((specifier, module, Required::Namespace));
                continue;
            }
            // Anything that does not resolve throws MODULE_NOT_FOUND only if required.
            let Ok(target) = self.resolve(name, &format!("{REQUIRE}{specifier}")) else { continue };
            let required = if self.kind(Path::new(&target), None)?.commonjs() {
                Required::CommonJs
            } else if self.kind(Path::new(&target), None)? == Kind::Json {
                Required::Json
            } else {
                Required::Namespace
            };
            requires.push((specifier.clone(), format!("{REQUIRE}{specifier}"), required));
        }
        Ok(load_time_esm_transforms::commonjs_body(code, name, &requires))
    }

    /// How a file becomes a module.
    ///
    /// # Arguments
    /// * `path` - the file.
    /// * `bytes` - its contents when already read; read here when syntax decides.
    fn kind(&self, path: &Path, bytes: Option<&[u8]>) -> Result<Kind, Error> {
        if let Some(kind) = self.kinds.borrow().get(path) {
            return Ok(*kind);
        }
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let kind = match extension {
            "mjs" => Kind::Esm,
            "cjs" => Kind::CommonJs,
            "json" => Kind::Json,
            "mts" => Kind::TypeScript { commonjs: false },
            "cts" => Kind::TypeScript { commonjs: true },
            _ => {
                let declared = match self.nearest(path.parent().unwrap_or(Path::new("/")))? {
                    Some(package) => package.manifest.kind,
                    None => Type::Unset,
                };
                let module = match declared {
                    Type::Module => true,
                    Type::CommonJs => false,
                    Type::Unset => {
                        let read;
                        let bytes = match bytes {
                            Some(bytes) => bytes,
                            None => {
                                read = (self.read)(path).map_err(Error::Invalid)?;
                                &read
                            }
                        };
                        let text = String::from_utf8_lossy(bytes);
                        load_time_esm_transforms::has_module_syntax(&path.to_string_lossy(), &text)
                    }
                };
                match (extension, module) {
                    ("ts", module) => Kind::TypeScript { commonjs: !module },
                    (_, true) => Kind::Esm,
                    (_, false) => Kind::CommonJs,
                }
            }
        };
        self.kinds.borrow_mut().insert(path.to_owned(), kind);
        Ok(kind)
    }

    /// The nearest package at or above `directory`.
    ///
    /// # Errors
    /// A `package.json` on the way that cannot be read or parsed.
    fn nearest(&self, directory: &Path) -> Result<Option<Rc<Package>>, Error> {
        let mut visited = Vec::new();
        let mut found = None;
        for dir in directory.ancestors() {
            if let Some(cached) = self.nearest.borrow().get(dir) {
                found = cached.clone();
                break;
            }
            visited.push(dir.to_owned());
            let file = dir.join("package.json");
            if file.is_file() {
                let bytes = (self.read)(&file).map_err(Error::Invalid)?;
                let manifest = Manifest::parse(&bytes)
                    .map_err(|e| Error::Invalid(format!("{}: {e}", file.display())))?;
                found = Some(Rc::new(Package { directory: dir.to_owned(), manifest }));
                break;
            }
        }
        let mut cache = self.nearest.borrow_mut();
        for dir in visited {
            cache.insert(dir, found.clone());
        }
        Ok(found)
    }

    /// A relative or absolute path specifier, with probing.
    fn relative(&self, directory: &Path, specifier: &str, require: bool) -> Result<PathBuf, Error> {
        let joined = directory.join(specifier);
        let probes = if require { REQUIRE_PROBES } else { IMPORT_PROBES };
        if let Some(found) = probe(&joined, probes) {
            return Ok(found);
        }
        // TypeScript sources import each other by their emitted `.js` names.
        [(".js", ".ts"), (".mjs", ".mts"), (".cjs", ".cts")]
            .iter()
            .find_map(|(js, ts)| specifier.strip_suffix(js).map(|stem| directory.join(format!("{stem}{ts}"))))
            .and_then(|swapped| exact(&swapped))
            .ok_or(Error::NotFound)
    }

    /// A bare specifier: the package's own name (self-reference) or a package in
    /// `node_modules` at or above `directory`, then its `exports` or `main`.
    fn package(&self, directory: &Path, specifier: &str, conditions: &[&str]) -> Result<PathBuf, Error> {
        if specifier.contains(['\\', '\0'])
            || specifier.split('/').any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(Error::NotFound);
        }
        let (name, subpath) = if specifier.starts_with('@') {
            let slash = specifier.find('/').ok_or(Error::NotFound)?;
            match specifier[slash + 1..].find('/') {
                Some(end) => (&specifier[..slash + 1 + end], &specifier[slash + 2 + end..]),
                None => (specifier, ""),
            }
        } else {
            specifier.split_once('/').unwrap_or((specifier, ""))
        };
        let subpath = if subpath.is_empty() { String::from(".") } else { format!("./{subpath}") };
        let own = self
            .nearest(directory)?
            .filter(|package| package.manifest.name.as_deref() == Some(name) && package.manifest.exports.is_some());
        let package = match own {
            Some(package) => package,
            None => {
                let root = directory
                    .ancestors()
                    .filter(|dir| dir.file_name().is_none_or(|n| n != "node_modules"))
                    .map(|dir| dir.join("node_modules").join(name))
                    .find(|candidate| candidate.is_dir())
                    .ok_or(Error::NotFound)?;
                let file = root.join("package.json");
                let manifest = if file.is_file() {
                    let bytes = (self.read)(&file).map_err(Error::Invalid)?;
                    Manifest::parse(&bytes).map_err(|e| Error::Invalid(format!("{}: {e}", file.display())))?
                } else {
                    Manifest { name: None, kind: Type::Unset, main: None, exports: None, imports: None }
                };
                Rc::new(Package { directory: root, manifest })
            }
        };
        let target = package.manifest.export_target(&subpath, conditions)?;
        if target.split('/').any(|part| part == "..") {
            return Err(Error::Invalid(format!("{name}: package target {target} leaves the package")));
        }
        let path = package.directory.join(target.strip_prefix("./").unwrap_or(&target));
        // Exported targets are exact; a legacy `main` or subpath may omit its extension.
        if package.manifest.exports.is_some() {
            exact(&path).ok_or(Error::NotFound)
        } else {
            probe(&path, REQUIRE_PROBES).ok_or(Error::NotFound)
        }
    }
}

/// Whether a specifier is a path: `./x`, `../x`, `/x`, `.` or `..`.
fn is_path(specifier: &str) -> bool {
    specifier.starts_with("./")
        || specifier.starts_with("../")
        || specifier.starts_with('/')
        || specifier == "."
        || specifier == ".."
}

/// # Returns
/// `path` when it is a file.
fn exact(path: &Path) -> Option<PathBuf> {
    path.is_file().then(|| path.to_owned())
}

/// # Returns
/// The first of `path` plus each suffix that is a file.
fn probe(path: &Path, suffixes: &[&str]) -> Option<PathBuf> {
    let text = path.to_string_lossy();
    let text = text.strip_suffix('/').unwrap_or(&text);
    suffixes.iter().map(|suffix| PathBuf::from(format!("{text}{suffix}"))).find(|candidate| candidate.is_file())
}

/// # Errors
/// The bytes are not UTF-8.
fn utf8(name: &str, bytes: Vec<u8>) -> Result<String, Error> {
    String::from_utf8(bytes).map_err(|_| Error::Invalid(format!("{name}: source is not UTF-8")))
}
