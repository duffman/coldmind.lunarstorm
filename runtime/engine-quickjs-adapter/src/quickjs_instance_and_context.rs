//! [`QuickInstance`] (a `JSRuntime`: heap, jobs, limits, interrupt, debugger
//! sessions) and [`QuickContext`] (one `JSContext` realm), with the per-instance
//! [`DebugSlot`] of context wrappers.

use crate::*;

/// The bridge wrapper of every context of an instance, keyed by the context's
/// `JSContext` address; shared with the engine hooks (interrupt, rejection tracker)
/// and, through context userdata, with compiles.
///
/// Decision: each context is wrapped when it is created, because the wrapper is
/// what makes jetbunny's Rust compiler the context's compiler
/// ([`Debugger::wrap`]). A debugger session is opened on a wrapper later, by
/// [`Instance::debug`]. The list sits in a `RefCell` so [`QuickInstance`]'s drop can
/// empty it and release the context references before the runtime goes.
#[derive(Clone, Default)]
pub(crate) struct DebugSlot(pub(crate) Rc<RefCell<Vec<(usize, Debugger)>>>);

impl DebugSlot {
    /// Runs `f` on the wrapper of one context.
    ///
    /// # Arguments
    /// * `context` - the `JSContext` address.
    /// * `f` - what to do with the wrapper.
    ///
    /// # Returns
    /// `f`'s result, or `None` for a context this instance did not create.
    pub(crate) fn with<R>(&self, context: usize, f: impl FnOnce(&Debugger) -> R) -> Option<R> {
        self.0.borrow().iter().find(|(key, _)| *key == context).map(|(_, debugger)| f(debugger))
    }

    /// Runs `f` on the wrapper a debugger session is open on.
    ///
    /// # Arguments
    /// * `f` - what to do with the wrapper.
    ///
    /// # Returns
    /// `f`'s result, or `None` while no session is open.
    pub(crate) fn attached<R>(&self, f: impl FnOnce(&Debugger) -> R) -> Option<R> {
        self.0.borrow().iter().find(|(_, debugger)| debugger.compiling()).map(|(_, debugger)| f(debugger))
    }
}

/// Wakes the owner's loop for the debugger; empty until [`Instance::debug`] supplies
/// the owner's waker, because the wrappers exist before any session does.
pub(crate) type Waker = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

// SAFETY: `DebugSlot` holds no JS values and no `'js` lifetime.
unsafe impl<'js> JsLifetime<'js> for DebugSlot {
    type Changed<'to> = DebugSlot;
}

/// Identity of each debugged run, distinct per instance in this process.
pub(crate) static DEBUG_EPOCH: AtomicU64 = AtomicU64::new(1);

/// A QuickJS runtime: heap, GC, job queue, limits and interrupt.
pub struct QuickInstance {
    /// The contexts' bridge wrappers. Emptied first in `drop`.
    pub(crate) debugger: DebugSlot,
    /// The owner's waker, handed to every wrapper; set by [`Instance::debug`].
    pub(crate) waker: Waker,
    /// The engine runtime.
    pub(crate) runtime: rquickjs::Runtime,
    /// Shared with the engine's interrupt hook.
    pub(crate) interrupt: Interrupt,
    /// The memory limit that was set, for [`Instance::heap`].
    pub(crate) memory: Option<usize>,
}

impl Instance<QuickJs> for QuickInstance {
    /// Creates the runtime and installs the owner's callbacks as the engine's module
    /// loader, rejection tracker and interrupt handler.
    ///
    /// # Arguments
    /// * `limits` - memory and stack limits.
    /// * `callbacks` - the owner's callbacks.
    ///
    /// # Returns
    /// The instance.
    ///
    /// # Errors
    /// The engine's message when the runtime cannot be allocated.
    fn new<C: Callbacks<QuickJs>>(limits: &Limits, callbacks: C) -> Result<Self, String> {
        let runtime = rquickjs::Runtime::new().map_err(|error| error.to_string())?;
        if let Some(bytes) = limits.memory {
            runtime.set_memory_limit(bytes);
        }
        if let Some(bytes) = limits.stack {
            runtime.set_max_stack_size(bytes);
        }
        // Decision: applied only when the owner asks; `None` keeps the engine's
        // default (256 KiB, growing 1.5x after each collection), so existing runtimes
        // are unchanged. Adopted from `quickjs_es_runtime`'s builder.
        if let Some(bytes) = limits.gc_threshold {
            runtime.set_gc_threshold(bytes);
        }
        // The three engine hooks share the callbacks; they take `&self`, so a plain
        // `Rc` is enough and a rejection reported during a module load cannot
        // conflict with the load in progress.
        let callbacks = Rc::new(callbacks);
        let debugger = DebugSlot::default();
        runtime.set_loader(Resolve(callbacks.clone()), Loading(callbacks.clone()));
        let tracker = callbacks.clone();
        let slot = debugger.clone();
        let tracker: RejectionTracker = Box::new(move |ctx, promise, reason, handled| {
            let (raw_promise, raw_reason) = (promise.as_raw(), reason.as_raw());
            slot.with(ctx.as_raw().as_ptr() as usize, |debugger| {
                // SAFETY: both values are live for this call of the tracker.
                unsafe {
                    debugger.rejection(
                        (&raw_promise as *const qjs::JSValue).cast(),
                        (&raw_reason as *const qjs::JSValue).cast(),
                        handled,
                    )
                };
            });
            tracker.rejection(&ctx, Scope::identity(&ctx, &promise), reason, handled);
        });
        runtime.set_host_promise_rejection_tracker(Some(tracker));
        let interrupt = Interrupt::default();
        let flag = interrupt.clone();
        let slot = debugger.clone();
        // The debugger's pause requests and evaluation deadlines arrive through the
        // same poll; jetbunny's bridge installed it as its own interrupt handler.
        runtime.set_interrupt_handler(Some(Box::new(move || {
            flag.take() || callbacks.interrupt() || slot.0.borrow().iter().any(|(_, debugger)| debugger.poll())
        })));
        Ok(Self { debugger, waker: Waker::default(), runtime, interrupt, memory: limits.memory })
    }

    /// # Returns
    /// A context with the full standard library, compiling with jetbunny's Rust
    /// compiler.
    ///
    /// # Errors
    /// The engine's message when allocation fails, or the bridge's when the context
    /// cannot be wrapped.
    fn context(&self) -> Result<QuickContext, String> {
        let context = rquickjs::Context::full(&self.runtime).map_err(|error| error.to_string())?;
        let epoch = NonZeroU64::new(DEBUG_EPOCH.fetch_add(1, Ordering::Relaxed)).expect("the counter starts at 1");
        let pointer: NonNull<c_void> = context.as_raw().cast();
        let waker = self.waker.clone();
        // SAFETY: the context is live on this thread; the wrapper holds a reference to
        // it and `QuickInstance::drop` releases every wrapper before the runtime.
        let debugger = unsafe {
            Debugger::wrap(
                pointer,
                DebugLimits::default(),
                Arc::new(move || {
                    if let Some(wake) = &*waker.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) {
                        wake();
                    }
                    Ok(())
                }),
                epoch,
            )
        }
        .map_err(|error| error.to_string())?;
        self.debugger.0.borrow_mut().push((pointer.as_ptr() as usize, debugger));
        // Compiles reach the debugger through the context: `declare` has only a `Ctx`.
        context.with(|ctx| ctx.store_userdata(self.debugger.clone()).map(|_| ()).map_err(|error| error.to_string()))?;
        Ok(QuickContext(context))
    }

    /// Runs a full collection (cycle removal; reference counts free the rest eagerly).
    fn collect_garbage(&self) {
        self.runtime.run_gc();
    }

    /// # Returns
    /// Bytes in use by the engine's allocator and the configured limit.
    fn heap(&self) -> Heap {
        Heap { used: self.runtime.memory_usage().memory_used_size.max(0) as usize, limit: self.memory }
    }

    /// # Returns
    /// The handle the engine's interrupt hook polls.
    fn interrupt(&self) -> Interrupt {
        self.interrupt.clone()
    }

    /// Opens a debugger session on the context's bridge wrapper
    /// (`lunarstorm_debugger::engines::quickjs_ng::Debugger`).
    fn debug(
        &self,
        context: &QuickContext,
        wake: Arc<dyn Fn() + Send + Sync>,
        options: DebugSessionOptions,
    ) -> Result<VmDebugHandle, String> {
        if self.debugger.attached(|_| ()).is_some() {
            return Err(String::from("a debugger is already attached"));
        }
        *self.waker.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(wake);
        self.debugger
            .with(context.0.as_raw().as_ptr() as usize, |debugger| debugger.attach(options).map_err(|error| error.to_string()))
            .unwrap_or_else(|| Err(String::from("the context does not belong to this instance")))
    }

    fn debug_entry(&self) -> Result<(), String> {
        self.debugger.attached(|debugger| debugger.before_entry().map_err(|error| error.to_string())).unwrap_or(Ok(()))
    }

    fn debug_service(&self) -> Result<bool, String> {
        self.debugger
            .attached(|debugger| {
                debugger.service().map_err(|error| error.to_string())?;
                Ok(debugger.is_stopping())
            })
            .unwrap_or(Ok(false))
    }

    fn debug_checkpoint(&self) -> Result<(), String> {
        self.debugger.attached(|debugger| debugger.checkpoint(true).map_err(|error| error.to_string())).unwrap_or(Ok(()))
    }
}

impl Drop for QuickInstance {
    /// Releases the bridge wrappers, and with them their context references, hooks
    /// and compilers, while the runtime is still alive.
    fn drop(&mut self) {
        let wrappers = std::mem::take(&mut *self.debugger.0.borrow_mut());
        drop(wrappers);
    }
}

/// A QuickJS context: one realm.
pub struct QuickContext(pub(crate) rquickjs::Context);

impl Context<QuickJs> for QuickContext {
    /// Enters the realm.
    ///
    /// # Arguments
    /// * `f` - the code to run with the context handle.
    ///
    /// # Returns
    /// The closure's result.
    #[inline]
    fn enter<R>(&self, f: impl for<'s> FnOnce(Ctx<'s>) -> R) -> R {
        self.0.with(f)
    }
}
