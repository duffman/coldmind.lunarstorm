//! [`Scope`] for `rquickjs::Ctx`: every value operation of the contract on
//! QuickJS.

use crate::*;

impl<'s> Scope<'s, QuickJs> for Ctx<'s> {
    #[inline]
    fn undefined(&self) -> Value<'s> {
        Value::new_undefined(self.clone())
    }

    #[inline]
    fn null(&self) -> Value<'s> {
        Value::new_null(self.clone())
    }

    #[inline]
    fn boolean(&self, value: bool) -> Value<'s> {
        Value::new_bool(self.clone(), value)
    }

    #[inline]
    fn int(&self, value: i32) -> Value<'s> {
        Value::new_int(self.clone(), value)
    }

    /// Boxes a number with the engine's own rule (`JS_NewNumber`): integral values
    /// become engine integers, which the interpreter handles faster than doubles,
    /// and everything else stays a double.
    ///
    /// Defect fixed: the binding's `Value::new_number` normalized with
    /// `value as i32` and `value == int as f64`, which stored `-0.0` as integer `+0`,
    /// so `1 / -0` came out as `+Infinity` in JS. Decision: call the engine's
    /// normalization instead of keeping a copy of it here, so both sides apply one
    /// rule. Dropped alternative: patching the binding's copy, which other callers
    /// of `rquickjs` would then depend on.
    ///
    /// # Arguments
    /// * `value` - the number.
    ///
    /// # Returns
    /// The boxed value, owned by this scope.
    #[inline]
    fn number(&self, value: f64) -> Value<'s> {
        // SAFETY: the context is live for the scope's lifetime; `JS_NewNumber` returns
        // an owned reference, which `Value` takes over without a second increment.
        unsafe { Value::from_raw(self.clone(), qjs::JS_NewNumber(self.as_raw().as_ptr(), value)) }
    }

    #[inline]
    fn string(&self, text: &str) -> Outcome<Value<'s>> {
        JsString::from_str(self.clone(), text).map(JsString::into_value).map_err(|error| thrown(self, error))
    }

    #[inline]
    fn bytes(&self, bytes: Vec<u8>) -> Outcome<Value<'s>> {
        TypedArray::<u8>::new(self.clone(), bytes).map(TypedArray::into_value).map_err(|error| thrown(self, error))
    }

    #[inline]
    fn bytes_copy(&self, bytes: &[u8]) -> Outcome<Value<'s>> {
        TypedArray::<u8>::new_copy(self.clone(), bytes).map(TypedArray::into_value).map_err(|error| thrown(self, error))
    }

    fn array(&self, items: &[Value<'s>]) -> Outcome<Value<'s>> {
        let build = || -> rquickjs::Result<Value<'s>> {
            let array = Array::new(self.clone())?;
            for (index, item) in items.iter().enumerate() {
                array.set(index, item.clone())?;
            }
            Ok(array.into_value())
        };
        build().map_err(|error| thrown(self, error))
    }

    #[inline]
    fn object(&self) -> Outcome<Value<'s>> {
        Object::new(self.clone()).map(Object::into_value).map_err(|error| thrown(self, error))
    }

    /// The binding only constructs typed errors by throwing them, so the error is
    /// thrown and immediately taken back; nothing is left pending.
    fn error(&self, kind: ErrorKind, message: &str) -> Outcome<Value<'s>> {
        let _ = match kind {
            ErrorKind::Error => Exception::throw_message(self, message),
            ErrorKind::Type => Exception::throw_type(self, message),
            ErrorKind::Range => Exception::throw_range(self, message),
            ErrorKind::Syntax => Exception::throw_syntax(self, message),
            ErrorKind::Reference => Exception::throw_reference(self, message),
        };
        Ok(self.catch())
    }

    fn function<F>(&self, name: &str, f: F) -> Outcome<Value<'s>>
    where
        F: for<'a> Fn(&Call<'a, QuickJs>) -> Outcome<Value<'a>> + 'static,
    {
        Function::new(self.clone(), Native(f))
            .and_then(|function| function.with_name(name))
            .map(Function::into_value)
            .map_err(|error| thrown(self, error))
    }

    fn host_object<T: 'static>(&self, data: T) -> Outcome<Value<'s>> {
        let data: Box<dyn Any> = Box::new(data);
        Class::instance(self.clone(), HostBox(RefCell::new(vec![data]))).map(Class::into_value).map_err(|error| thrown(self, error))
    }

    /// Looks in the value itself when it is a host object, else in the holder under
    /// the private [`HostKey`] (a native instance built by a non-native base).
    fn host_data<T: 'static, R>(&self, value: &Value<'s>, f: impl FnOnce(&T) -> R) -> Option<R> {
        let holder = match Class::<HostBox>::from_value(value) {
            Ok(holder) => holder,
            Err(_) => {
                let object = value.as_object().filter(|_| !value.is_proxy())?;
                let key = existing_host_key(self)?.ok()?;
                // A private key has no getter and no proxy is involved, so this read
                // runs no JS.
                Class::<HostBox>::from_value(&object.get::<_, Value<'s>>(key).ok()?).ok()?
            }
        };
        let cell = holder.borrow();
        let slots = cell.0.borrow();
        slots.iter().find_map(|data| data.downcast_ref::<T>()).map(f)
    }

    /// A promise capability; the resolver is the pair `[resolve, reject]` kept as
    /// one persistent array.
    fn promise(&self) -> Outcome<(Value<'s>, Persistent<Value<'static>>)> {
        let make = || -> rquickjs::Result<(Value<'s>, Persistent<Value<'static>>)> {
            let (promise, resolve, reject) = rquickjs::Promise::new(self)?;
            let pair = Array::new(self.clone())?;
            pair.set(0, resolve)?;
            pair.set(1, reject)?;
            Ok((promise.into_value(), Persistent::save(self, pair.into_value())))
        };
        make().map_err(|error| thrown(self, error))
    }

    /// Calls `resolve` or `reject` of the pair; the engine's resolving functions
    /// ignore every call after the first.
    fn settle(&self, resolver: &Persistent<Value<'static>>, result: Result<Value<'s>, Value<'s>>) -> Outcome<()> {
        let pair = self.local(resolver);
        let (index, value) = match result {
            Ok(value) => (0, value),
            Err(reason) => (1, reason),
        };
        let function = pair.as_array().and_then(|pair| pair.get::<Value<'s>>(index).ok()).filter(Value::is_function);
        let Some(function) = function else {
            return Err(self.throw_error(ErrorKind::Type, "not a promise resolver"));
        };
        self.call(&function, &[value]).map(|_| ())
    }

    fn class<T: 'static, F>(&self, spec: ClassSpec<'s, QuickJs>, constructor: F) -> Outcome<Value<'s>>
    where
        F: for<'a> Fn(&Call<'a, QuickJs>) -> Outcome<T> + 'static,
    {
        let build = || -> rquickjs::Result<Value<'s>> {
            let base = match &spec.extends {
                Some(base) => match base.as_function().filter(|base| base.is_constructor()) {
                    Some(base) => Some(base.clone()),
                    None => return Err(Exception::throw_type(self, "Class extends value is not a constructor")),
                },
                None => None,
            };
            let derived = base.is_some();
            let shell = ClassConstructor { f: constructor, name: spec.name.clone(), derived, data: std::marker::PhantomData };
            let class = Function::new(self.clone(), shell)?.with_name(&spec.name)?.with_constructor(true);
            let proto = Object::new(self.clone())?;
            if let Some(base) = &base {
                // As `class C extends B`: C.prototype inherits B.prototype, and C itself
                // inherits B (statics, and where the constructor finds its base).
                let parent = base.get::<_, Value<'s>>("prototype")?;
                match parent.as_object() {
                    Some(parent) => proto.set_prototype(Some(parent))?,
                    None if parent.is_null() => proto.set_prototype(None)?,
                    None => return Err(Exception::throw_type(self, "Class extends value does not have a valid prototype")),
                }
                class.set_prototype(Some(base))?;
            }
            // SAFETY: both are live objects of this context; the call defines
            // `class.prototype` and `proto.constructor` and takes no ownership.
            unsafe { qjs::JS_SetConstructor(self.as_raw().as_ptr(), class.as_raw(), proto.as_raw()) };
            for (name, method) in &spec.methods {
                proto.prop(name.as_str(), Property::from(method.clone()).writable().configurable())?;
            }
            for (name, get, set) in &spec.accessors {
                define_accessor(self, &proto, name, get.as_ref(), set.as_ref())?;
            }
            for (name, value) in &spec.statics {
                class.prop(name.as_str(), Property::from(value.clone()).writable().configurable())?;
            }
            Ok(class.into_value())
        };
        build().map_err(|error| thrown(self, error))
    }

    fn kind(&self, value: &Value<'s>) -> Kind {
        match value.type_of() {
            Type::Undefined | Type::Uninitialized => Kind::Undefined,
            Type::Null => Kind::Null,
            Type::Bool => Kind::Bool,
            Type::Int | Type::Float => Kind::Number,
            Type::String => Kind::String,
            Type::Function | Type::Constructor => Kind::Function,
            Type::Object | Type::Array | Type::Exception | Type::Promise | Type::Proxy => {
                match value.as_object().and_then(|object| object.as_typed_array::<u8>()) {
                    Some(_) => Kind::Bytes,
                    None => Kind::Object,
                }
            }
            _ => Kind::Other,
        }
    }

    #[inline]
    fn as_bool(&self, value: &Value<'s>) -> Option<bool> {
        value.as_bool()
    }

    #[inline]
    fn as_number(&self, value: &Value<'s>) -> Option<f64> {
        value.as_number()
    }

    /// Defect fixed: the binding's `CString::as_str` assumes QuickJS's encoding is
    /// UTF-8, but QuickJS keeps an unpaired surrogate as its 3-byte WTF-8 form
    /// ("Keep unmatched surrogate code points", quickjs.c `JS_ToCStringLen2`), so
    /// `'a\uD800b'` reached natives as an invalid `&str`. Decision: validate the
    /// lent bytes and throw a `TypeError` for an unpaired surrogate. Dropped
    /// alternative: replacing it with U+FFFD, which needs a copy and silently
    /// changes the text a native sees (a path, a key).
    #[inline]
    fn with_str<R>(&self, value: &Value<'s>, f: impl FnOnce(&str) -> R) -> Outcome<R> {
        match value.as_string() {
            Some(string) => match string.clone().to_cstring() {
                Ok(text) => match std::str::from_utf8(cstring_bytes(&text)) {
                    Ok(text) => Ok(f(text)),
                    Err(_) => Err(thrown(self, Exception::throw_type(self, "string contains an unpaired surrogate"))),
                },
                Err(error) => Err(thrown(self, error)),
            },
            None => Err(thrown(self, Exception::throw_type(self, "expected a string"))),
        }
    }

    #[inline]
    fn with_bytes<R>(&self, value: &Value<'s>, f: impl FnOnce(&[u8]) -> R) -> Outcome<R> {
        if let Some(string) = value.as_string() {
            return match string.clone().to_cstring() {
                // Raw bytes, not `as_str()`: see `cstring_bytes`.
                Ok(text) => Ok(f(cstring_bytes(&text))),
                Err(error) => Err(thrown(self, error)),
            };
        }
        if let Some(array) = value.as_object().and_then(|object| object.as_typed_array::<u8>()) {
            // SAFETY: the slice aliases the engine's heap and is valid until JS
            // runs again. The contract forbids `f` from calling into the engine,
            // so nothing can move or detach the buffer while it is lent.
            return Ok(f(unsafe { array.as_bytes() }.unwrap_or(&[])));
        }
        if value.is_undefined() || value.is_null() {
            return Ok(f(&[]));
        }
        Err(thrown(self, Exception::throw_type(self, "data must be a string or Uint8Array")))
    }

    #[inline]
    fn identity(&self, value: &Value<'s>) -> usize {
        // SAFETY: the pointer is only used as a number; it is never dereferenced.
        unsafe { qjs::JS_VALUE_GET_PTR(value.as_raw()) as usize }
    }

    #[inline]
    fn global(&self) -> Value<'s> {
        self.globals().into_value()
    }

    #[inline]
    fn get(&self, object: &Value<'s>, key: &str) -> Outcome<Value<'s>> {
        match object.as_object() {
            Some(object) => object.get::<_, Value<'s>>(key).map_err(|error| thrown(self, error)),
            None => Err(thrown(self, Exception::throw_type(self, "cannot read a property of a non-object"))),
        }
    }

    #[inline]
    fn set(&self, object: &Value<'s>, key: &str, value: Value<'s>) -> Outcome<()> {
        match object.as_object() {
            Some(object) => object.set(key, value).map_err(|error| thrown(self, error)),
            None => Err(thrown(self, Exception::throw_type(self, "cannot set a property of a non-object"))),
        }
    }

    /// [`Scope::call_with`] with `this` undefined.
    #[inline]
    fn call(&self, function: &Value<'s>, args: &[Value<'s>]) -> Outcome<Value<'s>> {
        self.call_with(function, &Value::new_undefined(self.clone()), args)
    }

    /// Calls straight into `JS_Call`. The receiver and arguments are borrowed: the
    /// engine does not consume them, so their reference counts are untouched.
    #[inline]
    fn call_with(&self, function: &Value<'s>, this: &Value<'s>, args: &[Value<'s>]) -> Outcome<Value<'s>> {
        let mut inline = [qjs::JS_UNDEFINED; INLINE_ARGS];
        let mut spilled;
        let raw: &mut [qjs::JSValue] = if args.len() <= INLINE_ARGS {
            &mut inline[..args.len()]
        } else {
            spilled = vec![qjs::JS_UNDEFINED; args.len()];
            &mut spilled
        };
        for (slot, arg) in raw.iter_mut().zip(args) {
            *slot = arg.as_raw();
        }
        // SAFETY: the context, callee and arguments are live for the whole call,
        // `raw` holds exactly `args.len()` values, and the returned value is owned
        // by us: it is either wrapped or, for the exception sentinel, not a
        // reference at all.
        unsafe {
            let result = qjs::JS_Call(self.as_raw().as_ptr(), function.as_raw(), this.as_raw(), raw.len() as _, raw.as_mut_ptr());
            if qjs::JS_IsException(result) {
                Err(Thrown)
            } else {
                Ok(Value::from_raw(self.clone(), result))
            }
        }
    }

    fn eval(&self, source: &str) -> Outcome<Value<'s>> {
        Ctx::eval::<Value<'s>, _>(self, source).map_err(|error| thrown(self, error))
    }

    fn module(&self, name: &str, source: ModuleSource) -> Outcome<(Value<'s>, Value<'s>)> {
        let run = || -> rquickjs::Result<(Value<'s>, Value<'s>)> {
            let (module, promise) = declare(self, name, source)?.eval()?;
            Ok((module.namespace()?.into_value(), promise.into_value()))
        };
        run().map_err(|error| thrown(self, error))
    }

    /// Stores the exports under the module's name; the loader then declares the C
    /// module from them (`Load::Native`).
    fn native_module(&self, name: &str, exports: &[(&str, Value<'s>)]) -> Outcome<()> {
        let stored = exports.iter().map(|(export, value)| ((*export).to_owned(), Persistent::save(self, value.clone()))).collect();
        let store = || -> Result<(), Error> {
            if self.userdata::<NativePending>().is_none() {
                self.store_userdata(NativePending::default()).map_err(|_| Error::Unknown)?;
            }
            let pending = self.userdata::<NativePending>().ok_or(Error::Unknown)?;
            pending.0.borrow_mut().insert(name.to_owned(), stored);
            Ok(())
        };
        store().map_err(|error| thrown(self, error))
    }

    /// Compiles without linking: QuickJS resolves a module's imports only when it
    /// is evaluated, so nothing is loaded here.
    ///
    /// Decision: source text is stripped and debug information (file name, line and
    /// column tables) is kept, so stack traces of loaded bytecode keep positions.
    fn compile(&self, name: &str, source: ModuleSource) -> Outcome<Vec<u8>> {
        let run = || -> rquickjs::Result<Vec<u8>> {
            Module::declare(self.clone(), name, source.source)?
                .write(WriteOptions { strip_source: true, ..WriteOptions::default() })
        };
        run().map_err(|error| thrown(self, error))
    }

    /// QuickJS never hands out a ticket, so there is nothing to complete.
    fn complete_load(&self, _ticket: u64, _result: Result<ModuleSource, String>) -> Outcome<()> {
        Err(self.throw_error(ErrorKind::Error, "QuickJS loads modules synchronously"))
    }

    #[inline]
    fn run_job(&self) -> bool {
        self.execute_pending_job()
    }

    /// Reads the engine's sticky out-of-memory flag (`jetbunny-sticky-oom-v1` in
    /// the vendored sources), which `JS_ThrowOutOfMemory` sets.
    #[inline]
    fn take_out_of_memory(&self) -> bool {
        // SAFETY: the context is entered on its owner thread, which is the only
        // thread the patch allows to take the flag; the call neither allocates nor
        // runs JS.
        unsafe { JS_JetbunnyTakeOutOfMemory(qjs::JS_GetRuntime(self.as_raw().as_ptr())) }
    }

    #[inline]
    fn throw(&self, value: Value<'s>) -> Thrown {
        let _ = Ctx::throw(self, value);
        Thrown
    }

    /// The engine reports "nothing pending" with its uninitialized sentinel, which
    /// the contract presents as `undefined`.
    #[inline]
    fn exception(&self) -> Value<'s> {
        let value = self.catch();
        if value.type_of() == Type::Uninitialized {
            Value::new_undefined(self.clone())
        } else {
            value
        }
    }

    #[inline]
    fn persist(&self, value: &Value<'s>) -> Persistent<Value<'static>> {
        Persistent::save(self, value.clone())
    }

    /// # Panics
    /// When the handle was created in a different instance; that is a bug in the
    /// caller, not a runtime condition.
    #[inline]
    fn local(&self, handle: &Persistent<Value<'static>>) -> Value<'s> {
        handle.clone().restore(self).expect("persistent handle used in the instance that created it")
    }
}
