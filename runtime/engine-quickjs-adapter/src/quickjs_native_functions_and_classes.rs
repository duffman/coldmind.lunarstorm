//! Natives on QuickJS: functions ([`Native`], arguments in a stack array),
//! class constructors ([`ClassConstructor`]), host data attached to objects
//! ([`HostBox`], [`attach`]) and accessor definition.

use crate::*;

/// Converts a binding error into the contract's marker, leaving an exception
/// pending in the context in every case.
///
/// # Arguments
/// * `ctx` - the context.
/// * `error` - the binding's error.
///
/// # Returns
/// The marker.
#[cold]
pub(crate) fn thrown(ctx: &Ctx<'_>, error: Error) -> Thrown {
    if !error.is_exception() {
        // A Rust-side failure (conversion, allocation): surface it as a JS error so
        // the caller finds something behind `Thrown`.
        let _ = Exception::throw_message(ctx, &error.to_string());
    }
    Thrown
}

/// A native closure, adapted to the binding's function trait.
pub(crate) struct Native<F>(pub(crate) F);

impl<'js, F> IntoJsFunc<'js, ()> for Native<F>
where
    F: for<'a> Fn(&Call<'a, QuickJs>) -> Outcome<Value<'a>> + 'static,
{
    /// # Returns
    /// "Any number of arguments": the closure checks what it needs.
    fn param_requirements() -> ParamRequirement {
        ParamRequirement::any()
    }

    /// Calls the closure with the arguments as a slice.
    ///
    /// # Arguments
    /// * `params` - the engine's view of the call.
    ///
    /// # Returns
    /// The closure's return value.
    ///
    /// # Errors
    /// `Error::Exception` when the closure reports [`Thrown`].
    #[inline]
    fn call<'a>(&self, params: Params<'a, 'js>) -> rquickjs::Result<Value<'js>> {
        let this = params.this();
        // Plain functions are not constructors (no constructor bit), so `new.target`
        // is always undefined here.
        let new_target = Value::new_undefined(params.ctx().clone());
        // SAFETY: `params`, `this`, `new_target` and the lent arguments outlive this
        // call of the closure.
        with_args(&params, |args| (self.0)(&unsafe { Call::new(params.ctx(), &this, &new_target, args) }))
            .map_err(|Thrown| Error::Exception)
    }
}

/// Lends a call's arguments to a closure as a slice, from a stack array of
/// [`INLINE_ARGS`] slots; only longer calls allocate.
///
/// # Arguments
/// * `params` - the engine's view of the call.
/// * `f` - receives the arguments.
///
/// # Returns
/// The closure's result.
#[inline]
pub(crate) fn with_args<'js, R>(params: &Params<'_, 'js>, f: impl FnOnce(&[Value<'js>]) -> R) -> R {
    let count = params.len();
    if count <= INLINE_ARGS {
        let mut slots = [const { MaybeUninit::<Value<'js>>::uninit() }; INLINE_ARGS];
        for (index, slot) in slots.iter_mut().enumerate().take(count) {
            slot.write(params.arg(index).unwrap_or_else(|| Value::new_undefined(params.ctx().clone())));
        }
        // SAFETY: the first `count` slots were just initialized, and
        // `MaybeUninit<T>` has the layout of `T`.
        let args = unsafe { std::slice::from_raw_parts(slots.as_ptr().cast::<Value<'js>>(), count) };
        let result = f(args);
        for slot in &mut slots[..count] {
            // SAFETY: initialized above and not used again.
            unsafe { slot.assume_init_drop() };
        }
        result
    } else {
        let args: Vec<Value<'js>> = (0..count).filter_map(|index| params.arg(index)).collect();
        f(&args)
    }
}

/// The constructor of a native class ([`Scope::class`]).
pub(crate) struct ClassConstructor<T, F> {
    /// The user's constructor: returns the instance data.
    pub(crate) f: F,
    /// The class name, for the "without new" error.
    pub(crate) name: String,
    /// Whether the class has a base that builds its instances.
    pub(crate) derived: bool,
    /// The data type the constructor returns.
    pub(crate) data: std::marker::PhantomData<fn() -> T>,
}

impl<'js, T, F> IntoJsFunc<'js, ()> for ClassConstructor<T, F>
where
    T: 'static,
    F: for<'a> Fn(&Call<'a, QuickJs>) -> Outcome<T> + 'static,
{
    /// # Returns
    /// "Any number of arguments": the constructor checks what it needs.
    fn param_requirements() -> ParamRequirement {
        ParamRequirement::any()
    }

    /// Builds an instance: refuses a call without `new`, makes the object with the
    /// prototype of `new.target` (or lets the base make it), runs the user's
    /// constructor and attaches its data.
    ///
    /// Decision: a derived class reads its base from its own `[[Prototype]]` at
    /// construction time, which is where JS's `super()` looks too, instead of
    /// keeping a persistent handle to the base in the closure; a persistent here
    /// would be an untraced root held by a GC-managed function.
    ///
    /// # Arguments
    /// * `params` - the engine's view of the call; `this` is `new.target`.
    ///
    /// # Returns
    /// The instance.
    ///
    /// # Errors
    /// A `TypeError` without `new` or with a base that is not a constructor; any
    /// exception from the base or the user's constructor.
    fn call<'a>(&self, params: Params<'a, 'js>) -> rquickjs::Result<Value<'js>> {
        let ctx = params.ctx().clone();
        if !params.is_constructor() {
            return Err(Exception::throw_type(&ctx, &format!("Class constructor {} cannot be invoked without 'new'", self.name)));
        }
        let new_target = params.this();
        let class = params.function().into_object().ok_or(Error::Unknown)?;
        let this = with_args(&params, |args| -> rquickjs::Result<Value<'js>> {
            if self.derived {
                let base = class.get_prototype().and_then(|base| base.into_value().into_function()).filter(Function::is_constructor);
                let Some(base) = base else {
                    return Err(Exception::throw_type(&ctx, "Super constructor is not a constructor"));
                };
                let mut raw: Vec<qjs::JSValue> = args.iter().map(Value::as_raw).collect();
                // SAFETY: the context, base, new.target and arguments are live for the
                // whole call; the result is owned by us unless it is the sentinel.
                unsafe {
                    let made = qjs::JS_CallConstructor2(ctx.as_raw().as_ptr(), base.as_raw(), new_target.as_raw(), raw.len() as _, raw.as_mut_ptr());
                    if qjs::JS_IsException(made) {
                        return Err(Error::Exception);
                    }
                    Ok(Value::from_raw(ctx.clone(), made))
                }
            } else {
                // `new.target.prototype`, falling back to the class's own when it is
                // not an object (GetPrototypeFromConstructor).
                let proto = match new_target.as_object().map(|target| target.get::<_, Value<'js>>("prototype")).transpose()?.and_then(Value::into_object) {
                    Some(proto) => proto,
                    None => class.get::<_, Object<'js>>("prototype")?,
                };
                Ok(Class::instance_proto(HostBox::default(), proto)?.into_value())
            }
        })?;
        let data = with_args(&params, |args| (self.f)(&unsafe { Call::new(&ctx, &this, &new_target, args) }))
            .map_err(|Thrown| Error::Exception)?;
        attach(&ctx, &this, Box::new(data))?;
        Ok(this)
    }
}

/// Attaches one native class's data to an instance: into the instance itself when
/// it is a host object, else into a host object hung off the instance under the
/// private [`HostKey`].
///
/// # Arguments
/// * `ctx` - the context.
/// * `this` - the instance.
/// * `data` - the data.
///
/// # Errors
/// A `TypeError` when the base returned a non-object or a proxy, or when the
/// instance's data is being read at this moment.
pub(crate) fn attach<'js>(ctx: &Ctx<'js>, this: &Value<'js>, data: Box<dyn Any>) -> rquickjs::Result<()> {
    let holder = match Class::<HostBox>::from_value(this) {
        Ok(holder) => holder,
        Err(_) => {
            // Decision: a proxy is refused, because reading the key through it would
            // run its traps, and `host_data` must not run JS.
            let object = this.as_object().filter(|_| !this.is_proxy());
            let Some(object) = object else {
                return Err(Exception::throw_type(ctx, "a native class instance must be a plain object, not a proxy"));
            };
            let key = host_key(ctx)?;
            match Class::<HostBox>::from_value(&object.get::<_, Value<'js>>(key.clone())?) {
                Ok(holder) => holder,
                Err(_) => {
                    let holder = Class::instance(ctx.clone(), HostBox::default())?;
                    // Not writable, enumerable or configurable: scripts cannot see the key
                    // (it is private) nor replace the holder.
                    object.prop(key, Property::from(holder.clone().into_value()))?;
                    holder
                }
            }
        }
    };
    let cell = holder.borrow();
    let Ok(mut slots) = cell.0.try_borrow_mut() else {
        return Err(Exception::throw_type(ctx, "instance data is in use"));
    };
    slots.push(data);
    Ok(())
}

/// The private symbol under which an instance that is not itself a host object
/// (one built by a base such as `Uint8Array`) carries its [`HostBox`]; one per
/// runtime, kept in its userdata.
///
/// Decision: a private symbol (`JS_NewPrivateSymbol`) rather than a JS symbol or
/// a side table: scripts cannot reach or forge it, the engine collects the box
/// with its instance, and lookup is one property read. Dropped alternative: an
/// ordinary `Symbol`, which `Object.getOwnPropertySymbols` exposes to scripts.
pub(crate) struct HostKey(pub(crate) Persistent<Value<'static>>);

// SAFETY: a persistent handle is `'static`, so `HostKey` is the same type under any
// lifetime.
unsafe impl<'js> JsLifetime<'js> for HostKey {
    type Changed<'to> = HostKey;
}

/// The runtime's [`HostKey`], created on first use.
///
/// # Arguments
/// * `ctx` - the context.
///
/// # Returns
/// The key as an atom.
///
/// # Errors
/// An allocation failure.
pub(crate) fn host_key<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<Atom<'js>> {
    if let Some(key) = existing_host_key(ctx) {
        return key;
    }
    // SAFETY: the context is live; the description is NUL-terminated; the result is
    // owned by us unless it is the sentinel.
    let symbol = unsafe {
        let raw = JS_NewPrivateSymbol(ctx.as_raw().as_ptr(), c"lunarstorm.host".as_ptr());
        if qjs::JS_IsException(raw) {
            return Err(Error::Exception);
        }
        Value::from_raw(ctx.clone(), raw)
    };
    ctx.store_userdata(HostKey(Persistent::save(ctx, symbol.clone()))).map_err(|_| Error::Unknown)?;
    Atom::from_value(ctx.clone(), &symbol)
}

/// The runtime's [`HostKey`] if one was made; reading never creates it.
///
/// # Arguments
/// * `ctx` - the context.
///
/// # Returns
/// The key as an atom, or `None` when no native instance was ever built.
pub(crate) fn existing_host_key<'js>(ctx: &Ctx<'js>) -> Option<rquickjs::Result<Atom<'js>>> {
    let symbol = ctx.userdata::<HostKey>()?.0.clone();
    Some(symbol.restore(ctx).and_then(|symbol| Atom::from_value(ctx.clone(), &symbol)))
}

/// Defines an accessor property: configurable, not enumerable, like a getter or
/// setter of a JS `class`.
///
/// # Arguments
/// * `ctx` - the context.
/// * `object` - where to define it.
/// * `name` - the property name.
/// * `get` - the getter, if any.
/// * `set` - the setter, if any.
///
/// # Errors
/// The engine's exception when the definition fails.
pub(crate) fn define_accessor<'js>(ctx: &Ctx<'js>, object: &Object<'js>, name: &str, get: Option<&Value<'js>>, set: Option<&Value<'js>>) -> rquickjs::Result<()> {
    let flags = qjs::JS_PROP_HAS_GET | qjs::JS_PROP_HAS_SET | qjs::JS_PROP_HAS_CONFIGURABLE | qjs::JS_PROP_CONFIGURABLE | qjs::JS_PROP_HAS_ENUMERABLE | qjs::JS_PROP_THROW;
    let raw = |half: Option<&Value<'js>>| half.map_or(qjs::JS_UNDEFINED, Value::as_raw);
    // SAFETY: the context and values are live; `JS_DefineProperty` borrows the getter
    // and setter, and the atom made here is freed after the call.
    unsafe {
        let ctx_ptr = ctx.as_raw().as_ptr();
        let atom = qjs::JS_NewAtomLen(ctx_ptr, name.as_ptr().cast(), name.len() as _);
        if atom == qjs::JS_ATOM_NULL {
            return Err(Error::Exception);
        }
        let result = qjs::JS_DefineProperty(ctx_ptr, object.as_raw(), atom, qjs::JS_UNDEFINED, raw(get), raw(set), flags as _);
        qjs::JS_FreeAtom(ctx_ptr, atom);
        if result < 0 {
            return Err(Error::Exception);
        }
    }
    Ok(())
}

/// The one class behind every host object and native class instance: owned,
/// type-erased Rust values. Dropping it is the finalizer.
///
/// Decision: a list, not one value, because each native class in an instance's
/// chain attaches its own data (`class B extends A`, both native), told apart by
/// type when read. The `RefCell` is borrowed mutably only while one native
/// constructor attaches its data, when no JS runs.
#[derive(Default)]
pub(crate) struct HostBox(pub(crate) RefCell<Vec<Box<dyn Any>>>);

// SAFETY: `HostBox` holds no JS values and no `'js` lifetime, so it is the same
// type under any lifetime.
unsafe impl<'js> JsLifetime<'js> for HostBox {
    type Changed<'to> = HostBox;
}

impl<'js> Trace<'js> for HostBox {
    /// Nothing to trace: host data never holds scoped handles (they cannot be
    /// `'static`), and persistent handles are roots of their own.
    ///
    /// # Arguments
    /// * `_tracer` - unused.
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

impl<'js> JsClass<'js> for HostBox {
    const NAME: &'static str = "HostObject";
    type Mutable = Readable;

    /// # Arguments
    /// * `_ctx` - unused.
    ///
    /// # Returns
    /// `None`: host objects are created from Rust only.
    ///
    /// # Errors
    /// Never fails.
    fn constructor(_ctx: &Ctx<'js>) -> rquickjs::Result<Option<Constructor<'js>>> {
        Ok(None)
    }
}
