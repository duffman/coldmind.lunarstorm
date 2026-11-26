//! Module resolution and loading through rquickjs: the [`Resolve`] and
//! [`Loading`] adapters over the contract's callbacks, native modules
//! ([`NativeDef`], [`NativePending`]) and [`declare`], which compiles source or
//! loads an image.

use crate::*;

/// The engine's resolver hook, forwarding to [`Callbacks::resolve`].
pub(crate) struct Resolve<C>(pub(crate) Rc<C>);

impl<C: Callbacks<QuickJs>> Resolver for Resolve<C> {
    /// # Arguments
    /// * `ctx` - the importing context.
    /// * `base` - the importing module.
    /// * `name` - the specifier.
    /// * `_attributes` - import attributes; not part of the contract.
    ///
    /// # Returns
    /// The canonical module name.
    ///
    /// # Errors
    /// A resolving error carrying the callback's message.
    fn resolve<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        self.0.resolve(ctx, base, name).map_err(|message| Error::new_resolving_message(base, name, message))
    }
}

/// The engine's loader hook, forwarding to [`Callbacks::load`].
pub(crate) struct Loading<C>(pub(crate) Rc<C>);

impl<C: Callbacks<QuickJs>> Loader for Loading<C> {
    /// # Arguments
    /// * `ctx` - the importing context.
    /// * `name` - canonical module name.
    /// * `_attributes` - import attributes; not part of the contract.
    ///
    /// # Returns
    /// The declared module.
    ///
    /// # Errors
    /// A loading error carrying the callback's message, a syntax error, or a
    /// refusal of `Load::Later` (QuickJS needs the module before the hook returns).
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js, Declared>> {
        match self.0.load(ctx, name) {
            Ok(Load::Ready(source)) => declare(ctx, name, source),
            Ok(Load::Later(_)) => Err(Error::new_loading_message(name, "QuickJS loads modules synchronously")),
            Ok(Load::Native) => Module::declare_def::<NativeDef, _>(ctx.clone(), name),
            Err(message) => Err(Error::new_loading_message(name, message)),
        }
    }
}

/// The exports of native modules that were declared by [`Scope::native_module`] and
/// not yet evaluated, by canonical module name.
///
/// Decision: rquickjs's `ModuleDef` is a static trait (no `self`), so the values of
/// one module cannot travel in the definition. They wait here, in the context's
/// userdata, between the callback that built them (`Loader::load`) and the engine's
/// evaluation of the module (`NativeDef::evaluate`), which takes them out. Dropped
/// alternative: building the C module by hand with `JS_NewCModule`, which needs the
/// binding's private `Module::from_ptr` to return the module to the loader.
#[derive(Default)]
pub(crate) struct NativePending(pub(crate) RefCell<HashMap<String, Exports>>);

/// The `(export name, value)` pairs of one native module.
pub(crate) type Exports = Vec<(String, Persistent<Value<'static>>)>;

// SAFETY: `NativePending` holds no scoped handles: persistent handles are `'static`
// and everything else is owned text, so it is the same type under any lifetime.
unsafe impl<'js> JsLifetime<'js> for NativePending {
    type Changed<'to> = NativePending;
}

/// The one native module definition: it declares and fills whatever
/// [`Scope::native_module`] stored under the module's name.
pub(crate) struct NativeDef;

impl ModuleDef for NativeDef {
    /// Declares the export names stored for this module.
    ///
    /// # Arguments
    /// * `decl` - the module being declared.
    ///
    /// # Errors
    /// An error when nothing is stored for the module's name (a `Load::Native` answer
    /// without a prior `native_module` call).
    fn declare<'js>(decl: &Declarations<'js>) -> rquickjs::Result<()> {
        let module = decl.module();
        let name = module.name::<String>()?;
        let ctx = module.ctx();
        let pending = ctx.userdata::<NativePending>().ok_or(Error::Unknown)?;
        let pending = pending.0.borrow();
        let exports = pending.get(&name).ok_or_else(|| Error::new_loading_message(&name, "no native module was declared under this name"))?;
        for (export, _) in exports {
            decl.declare(export.as_str())?;
        }
        Ok(())
    }

    /// Sets the export values; each importer then sees the same values.
    ///
    /// # Arguments
    /// * `ctx` - the context.
    /// * `exports` - the module being evaluated.
    ///
    /// # Errors
    /// An error when the stored values are gone or belong to another instance.
    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &rquickjs::module::Exports<'js>) -> rquickjs::Result<()> {
        let name = exports.module().name::<String>()?;
        let values = {
            let pending = ctx.userdata::<NativePending>().ok_or(Error::Unknown)?;
            let taken = pending.0.borrow_mut().remove(&name);
            taken.ok_or_else(|| Error::new_loading_message(&name, "native module evaluated twice"))?
        };
        for (export, value) in values {
            exports.export(export, value.restore(ctx)?)?;
        }
        Ok(())
    }
}

/// Compiles a module and fills its `import.meta`.
///
/// # Arguments
/// * `ctx` - the context.
/// * `name` - canonical module name.
/// * `source` - source bytes and metadata.
///
/// # Returns
/// The declared, not yet evaluated module.
///
/// # Errors
/// The engine's syntax error.
pub(crate) fn declare<'js>(ctx: &Ctx<'js>, name: &str, source: ModuleSource) -> rquickjs::Result<Module<'js, Declared>> {
    let module = if let Some(image) = &source.image {
        // SAFETY: an `Image` comes only from a verified package or from
        // `Scope::compile` of this same adapter and engine (same `Descriptor`).
        // Decision: packaged bytecode has no statement markers, so it is not
        // registered with a debugger.
        unsafe { Module::load(ctx.clone(), image.bytes())? }
    } else {
        // While a session is open the source is staged, so jetbunny's compiler
        // callback registers it under its file URI and source map; the callback then
        // compiles it with statement markers and hands the root to the debugger.
        let slot = ctx.userdata::<DebugSlot>().map(|slot| slot.clone());
        let staged = slot.as_ref().and_then(|slot| {
            slot.with(ctx.as_raw().as_ptr() as usize, |debugger| {
                if !debugger.compiling() {
                    return Ok(());
                }
                let text = std::str::from_utf8(&source.source)
                    .map_err(|_| Error::new_loading_message(name, "module source is not UTF-8"))?;
                debugger
                    .stage(text, name, true, source.source_map.clone())
                    .map_err(|error| Error::new_loading_message(name, error.to_string()))
            })
        });
        if let Some(staged) = staged {
            staged?;
        }
        Module::declare(ctx.clone(), name, source.source)?
    };
    if let Some(meta) = source.meta {
        let object = module.meta()?;
        let directory = Path::new(&meta.filename).parent().and_then(Path::to_str).unwrap_or("/");
        object.set("url", format!("file://{}", meta.filename))?;
        object.set("dirname", directory)?;
        object.set("filename", meta.filename)?;
        object.set("main", meta.main)?;
    }
    Ok(module)
}

/// The bytes QuickJS lent for a string, without asserting they are UTF-8.
///
/// Decision: the binding's `CString::as_str` builds a `&str` unchecked, which is
/// undefined behaviour for a string with an unpaired surrogate (QuickJS encodes it
/// as WTF-8); reading the pointer and length avoids ever forming that `&str`.
///
/// # Arguments
/// * `text` - the lent string.
///
/// # Returns
/// Its bytes, without the terminating NUL; valid while `text` lives.
#[inline]
pub(crate) fn cstring_bytes<'a>(text: &'a rquickjs::CString<'_>) -> &'a [u8] {
    // SAFETY: `as_ptr` and `len` describe the buffer QuickJS lent, which stays
    // alive until `text` is dropped.
    unsafe { std::slice::from_raw_parts(text.as_ptr().cast::<u8>(), text.len()) }
}
