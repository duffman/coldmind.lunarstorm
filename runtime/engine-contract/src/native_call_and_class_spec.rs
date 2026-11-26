//! The native side of a call: [`Call`] (receiver, arguments, `new.target`)
//! and [`ClassSpec`], the description of a native class an adapter builds.

use crate::*;

/// One invocation of a native function: the context handle and the arguments.
///
/// Decision: this is a struct rather than two closure parameters because a
/// higher-ranked closure bound must name its lifetime in a parameter type, and
/// associated types alone (`V::Scope<'s>`) do not count. It holds two raw pointers
/// into the adapter's stack frame, so passing it costs nothing; the accessors hand
/// out references that cannot outlive the `Call`.
pub struct Call<'s, V: Vm> {
    /// The context handle of the call.
    scope: *const V::Scope<'s>,
    /// The receiver (`this`).
    this: *const V::Value<'s>,
    /// `new.target`: the constructor `new` was applied to, `undefined` for a plain call.
    new_target: *const V::Value<'s>,
    /// First argument.
    args: *const V::Value<'s>,
    /// Number of arguments.
    len: usize,
}

impl<'s, V: Vm> Call<'s, V> {
    /// Builds the view an adapter passes to a native closure.
    ///
    /// # Arguments
    /// * `scope` - the context handle of the call.
    /// * `this` - the receiver; for a class constructor, the new instance.
    /// * `new_target` - `new.target`; `undefined` when not called with `new`.
    /// * `args` - the arguments, in order.
    ///
    /// # Returns
    /// The call view.
    ///
    /// # Safety
    /// `scope`, `this`, `new_target` and `args` must stay alive and unmoved for as
    /// long as the returned value is used; adapters create it on the stack around
    /// one closure call.
    #[inline]
    pub unsafe fn new(scope: &V::Scope<'s>, this: &V::Value<'s>, new_target: &V::Value<'s>, args: &[V::Value<'s>]) -> Self {
        Self { scope, this, new_target, args: args.as_ptr(), len: args.len() }
    }

    /// # Returns
    /// The receiver: `this` of the call (`undefined` for a plain function call in
    /// strict code); inside a [`Scope::class`] constructor, the new instance.
    #[inline]
    pub fn this(&self) -> &V::Value<'s> {
        // SAFETY: valid for the life of `self` by the contract of `new`.
        unsafe { &*self.this }
    }

    /// # Returns
    /// `new.target`: the constructor `new` was applied to (a JS subclass when the
    /// call comes from its `super()`), or `undefined` for a plain call.
    #[inline]
    pub fn new_target(&self) -> &V::Value<'s> {
        // SAFETY: valid for the life of `self` by the contract of `new`.
        unsafe { &*self.new_target }
    }

    /// # Returns
    /// The context handle.
    #[inline]
    pub fn scope(&self) -> &V::Scope<'s> {
        // SAFETY: valid for the life of `self` by the contract of `new`.
        unsafe { &*self.scope }
    }

    /// # Returns
    /// The arguments the caller passed.
    #[inline]
    pub fn args(&self) -> &[V::Value<'s>] {
        // SAFETY: valid for the life of `self` by the contract of `new`.
        unsafe { std::slice::from_raw_parts(self.args, self.len) }
    }
}

/// One accessor of a [`ClassSpec`]: `(name, getter, setter)`; a missing half is `None`.
pub type Accessor<'s, V> = (String, Option<<V as Vm>::Value<'s>>, Option<<V as Vm>::Value<'s>>);

/// Description of a native class, built from Rust and turned into a JS constructor
/// by [`Scope::class`].
///
/// Decision: members are JS values the caller built (usually with
/// [`Scope::function`], whose closures read the receiver through [`Call::this`]),
/// not closures of their own. Methods of different closure types then fit in one
/// `Vec` without type erasure, so the contract stays free of `dyn`, and the
/// adapter needs no second native-call path. Only the constructor is a closure of
/// its own ([`Scope::class`]'s `constructor`), because it alone runs under `new`.
pub struct ClassSpec<'s, V: Vm> {
    /// The constructor's `name`.
    pub name: String,
    /// The base: a constructor whose `prototype` becomes the parent of this class's
    /// `prototype`, and which builds every instance. A class from [`Scope::class`]
    /// or a builtin such as `Uint8Array`; `None` for a base class.
    pub extends: Option<V::Value<'s>>,
    /// `(name, function)` pairs installed on the prototype: writable, configurable,
    /// not enumerable, like methods of a JS `class`.
    pub methods: Vec<(String, V::Value<'s>)>,
    /// `(name, getter, setter)` triples installed on the prototype as accessors:
    /// configurable, not enumerable. A missing half is `None`.
    pub accessors: Vec<Accessor<'s, V>>,
    /// `(name, value)` pairs installed on the constructor itself: writable,
    /// configurable, not enumerable, like `static` members of a JS `class`.
    pub statics: Vec<(String, V::Value<'s>)>,
}

impl<'s, V: Vm> ClassSpec<'s, V> {
    /// Starts a description with no base and no members.
    ///
    /// # Arguments
    /// * `name` - the class name.
    ///
    /// # Returns
    /// The description.
    pub fn new(name: &str) -> Self {
        Self { name: name.to_owned(), extends: None, methods: Vec::new(), accessors: Vec::new(), statics: Vec::new() }
    }

    /// Sets the base constructor.
    ///
    /// # Arguments
    /// * `base` - a native class or a builtin constructor.
    ///
    /// # Returns
    /// The description.
    pub fn extends(mut self, base: V::Value<'s>) -> Self {
        self.extends = Some(base);
        self
    }

    /// Adds a prototype method.
    ///
    /// # Arguments
    /// * `name` - property name.
    /// * `function` - the method.
    ///
    /// # Returns
    /// The description.
    pub fn method(mut self, name: &str, function: V::Value<'s>) -> Self {
        self.methods.push((name.to_owned(), function));
        self
    }

    /// Adds a prototype accessor.
    ///
    /// # Arguments
    /// * `name` - property name.
    /// * `get` - the getter, if any.
    /// * `set` - the setter, if any.
    ///
    /// # Returns
    /// The description.
    pub fn accessor(mut self, name: &str, get: Option<V::Value<'s>>, set: Option<V::Value<'s>>) -> Self {
        self.accessors.push((name.to_owned(), get, set));
        self
    }

    /// Adds a static member.
    ///
    /// # Arguments
    /// * `name` - property name.
    /// * `value` - the value (a function for a static method).
    ///
    /// # Returns
    /// The description.
    pub fn static_member(mut self, name: &str, value: V::Value<'s>) -> Self {
        self.statics.push((name.to_owned(), value));
        self
    }
}
