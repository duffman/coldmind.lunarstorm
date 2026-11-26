//! What a module is when it reaches the engine: [`ModuleSource`] (source text
//! or a compiled [`Image`]), its [`ModuleMeta`], and the loader's answer
//! [`Load`].

use crate::*;

/// `import.meta` of a file module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleMeta {
    /// Absolute path of the module.
    pub filename: String,
    /// Whether it is the entry module.
    pub main: bool,
}

/// Bytecode an engine may load as a module ([`ModuleSource::image`]).
///
/// Decision (lunarstorm patch, bytecode trust): the engine does not validate
/// bytecode, so the type admits only two trusted origins and its bytes are
/// private: a module of a `.jbn` package, which exists only after
/// `Package::open` checked its signature and every blob digest, and an image
/// this process's engine wrote with [`Image::compile`]. An unverified buffer
/// cannot reach the engine's bytecode loader. Before, `ModuleSource` carried a
/// `bytecode: bool` that any caller could set over any bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image(Vec<u8>);

impl Image {
    /// A module's bytecode from a verified package.
    ///
    /// # Arguments
    /// * `package` - a package `Package::open` verified.
    /// * `module` - the module's name in its manifest.
    ///
    /// # Returns
    /// The image, or `None` when the package has no such module.
    pub fn from_package(package: &lunarstorm_jbn_package_format::Package, module: &str) -> Option<Image> {
        package.bytecode(module).map(|bytes| Image(bytes.to_vec()))
    }

    /// Compiles a module with [`Scope::compile`] and keeps its bytecode as an image.
    ///
    /// # Arguments
    /// * `scope` - the engine that writes the image.
    /// * `name` - the canonical name the module will be loaded under.
    /// * `source` - its source text.
    ///
    /// # Errors
    /// What [`Scope::compile`] reports.
    pub fn compile<'s, V: Vm, S: Scope<'s, V>>(scope: &S, name: &str, source: ModuleSource) -> Outcome<Image> {
        scope.compile(name, source).map(Image)
    }

    /// # Returns
    /// The bytecode.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A module as handed to the engine: source text, or an [`Image`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSource {
    /// The source bytes (UTF-8); ignored when `image` is set.
    pub source: Vec<u8>,
    /// `import.meta` contents; `None` for generated modules.
    pub meta: Option<ModuleMeta>,
    /// Bytecode to load instead of `source`, written by an engine with the same
    /// [`Descriptor`]. Only valid when [`Capabilities::bytecode_cache`] is set.
    pub image: Option<Image>,
    /// Source Map v3 JSON from `source` to the file it was made from (TypeScript),
    /// for an attached debugger; `None` when `source` is the file itself.
    pub source_map: Option<String>,
}

/// Answer of the module load callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Load {
    /// The source is available now.
    Ready(ModuleSource),
    /// The source will be delivered with [`Scope::complete_load`] under this ticket.
    /// Only valid when [`Capabilities::async_module_load`] is set.
    Later(u64),
    /// The module is native: the callback has already declared it with
    /// [`Scope::native_module`] under the name it was asked to load, and the engine
    /// takes it from there. Its exports are values the host built, not source text.
    ///
    /// Decision: the answer carries no payload because the values belong to a scope
    /// (`V::Value<'s>`), which would make [`Load`] generic over the VM and break its
    /// derives. The alternative that was dropped, `Load::Native(Vec<export names>)`
    /// plus a second callback to fill them in, needs a module handle type in the
    /// contract for no gain: `load` already holds the scope.
    Native,
}
