//! The JS thread (spec 6): one VM Instance running the backend artifact, its timers, its
//! native calls, and the per-message execution budget.
//!
//! The thread owns exactly one [`Instance`] at a time. A backend reload (6.2) drops the
//! Instance and builds a new one: heap, job queue, timers and pending native calls all go
//! with the old one, and the native router releases every resource tagged with its epoch.
//!
//! Decision: the VM is QuickJS through rquickjs, bound at compile time. The section 5
//! Runtime crate (`runtime/`, hippo) is a binary with no library target, so the loop that
//! section 5 calls `poll` lives here: one blocking receive per step, with the next timer
//! as its timeout.

use crate::envelope::{Envelope, Kind, VERSION};
use crate::manifest::HostConfig;
use crossbeam_channel::{bounded, select, unbounded, Receiver, Sender, TrySendError};
use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::{CaughtError, Context, Ctx, Function, Module, Object, Persistent, Runtime};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// JS glue evaluated before the backend.
const PRELUDE: &str = include_str!("prelude.js");

/// Source of the `@desktop/bridge` built-in module; the browser host serves it too.
pub(crate) const BRIDGE_MODULE: &str = "const b = globalThis.__zebra.bridge;\n\
export const setProperty = b.setProperty;\nexport const serve = b.serve;\nexport const restored = b.restored;\nexport default b;\n";

/// Source of the `@desktop/api` built-in module; the browser host serves it too.
pub(crate) const API_MODULE: &str = "export const native = globalThis.__zebra.native;\nexport default native;\n";

/// Messages the host sends to the JS thread.
#[derive(Debug)]
pub enum ToJs {
    /// Start (or replace) the Instance with a backend artifact.
    Load {
        /// Backend artifact source.
        backend: Arc<str>,
        /// Host configuration (permissions, limits).
        config: Arc<HostConfig>,
        /// Carried-over control values, `{Form: {Control: {prop: value}}}`.
        restore: Value,
        /// Epoch of the new Instance.
        epoch: u64,
    },
    /// An event, request or cancel for the backend.
    Envelope(Envelope),
    /// Shut the thread down.
    Stop,
}

/// Messages the JS thread sends to the host.
#[derive(Debug, Clone, PartialEq)]
pub enum FromJs {
    /// An `update` or `response` from the backend, stamped with the Instance epoch.
    Envelope(Envelope),
    /// The Instance for `epoch` finished loading (or failed to).
    Loaded {
        /// Epoch of the Instance.
        epoch: u64,
        /// The load error, if any.
        error: Option<String>,
    },
    /// An uncaught exception, rejected promise or exceeded budget in the backend.
    Error {
        /// Epoch it happened in.
        epoch: u64,
        /// Description with stack.
        message: String,
    },
    /// A `console` line.
    Log {
        /// `info`, `debug`, `warn` or `error`.
        level: String,
        /// The text.
        message: String,
    },
}

/// Finishes one native call; it can be moved to any thread.
#[derive(Debug, Clone)]
pub struct Completer {
    /// Channel back to the JS thread.
    tx: Sender<Completion>,
    /// Call id.
    id: u64,
    /// Epoch of the caller; a completion for an older epoch is dropped.
    pub epoch: u64,
}

impl Completer {
    /// Delivers the result.
    ///
    /// # Arguments
    /// * `result` - value or error message.
    pub fn complete(self, result: Result<Value, String>) {
        // The JS thread may be gone at shutdown; then there is no one to tell.
        let _ = self.tx.send(Completion { id: self.id, epoch: self.epoch, result });
    }
}

/// One finished native call.
#[derive(Debug)]
struct Completion {
    /// Call id.
    id: u64,
    /// Caller epoch.
    epoch: u64,
    /// Result.
    result: Result<Value, String>,
}

/// One `native.<ns>.<method>(...)` call from the backend.
#[derive(Debug)]
pub struct NativeCall {
    /// Service namespace (`window`, `dialog`, ...).
    pub ns: String,
    /// Method name.
    pub method: String,
    /// Arguments.
    pub args: Vec<Value>,
    /// Where the result goes.
    pub done: Completer,
}

/// Routes native calls to the thread that may run them (UI thread, worker pool, inline).
pub trait NativeRouter: Send + Sync {
    /// Starts one permitted call.
    ///
    /// # Arguments
    /// * `call` - the call.
    fn call(&self, call: NativeCall);

    /// Releases every native resource the backend of `epoch` created (6.2).
    ///
    /// # Arguments
    /// * `epoch` - the epoch being retired.
    fn release_epoch(&self, epoch: u64);
}

/// Handle to the running JS thread.
pub struct JsThread {
    /// Bounded host -> JS queue.
    tx: Sender<ToJs>,
    /// Join handle.
    handle: Option<JoinHandle<()>>,
}

impl JsThread {
    /// Starts the JS thread. It idles until the first [`ToJs::Load`].
    ///
    /// # Arguments
    /// * `out` - receives every [`FromJs`]; called on the JS thread.
    /// * `natives` - native call router.
    ///
    /// # Returns
    /// The handle.
    pub fn spawn(out: Arc<dyn Fn(FromJs) + Send + Sync>, natives: Arc<dyn NativeRouter>) -> JsThread {
        let (tx, rx) = bounded(crate::envelope::QUEUE_CAPACITY);
        let handle = std::thread::Builder::new()
            .name("zebra-js".into())
            .spawn(move || run(rx, out, natives))
            .expect("spawn the JS thread");
        JsThread { tx, handle: Some(handle) }
    }

    /// Queues a message without blocking.
    ///
    /// # Arguments
    /// * `msg` - the message.
    ///
    /// # Returns
    /// Nothing.
    ///
    /// # Errors
    /// The queue is full (the backend is not keeping up) or the thread is gone.
    pub fn send(&self, msg: ToJs) -> Result<(), String> {
        self.tx.try_send(msg).map_err(|e| match e {
            TrySendError::Full(_) => "backend queue is full".to_owned(),
            TrySendError::Disconnected(_) => "backend thread has stopped".to_owned(),
        })
    }

    /// Queues a message, waiting for room. Used for `Load` and `Stop`, which must not be
    /// lost.
    ///
    /// # Arguments
    /// * `msg` - the message.
    pub fn send_blocking(&self, msg: ToJs) {
        let _ = self.tx.send(msg);
    }
}

impl Drop for JsThread {
    /// Stops the thread and waits for it.
    fn drop(&mut self) {
        let _ = self.tx.send(ToJs::Stop);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Timers of one Instance.
#[derive(Default)]
struct Timers {
    /// Next id.
    next: u64,
    /// Due times.
    due: BTreeSet<(Instant, u64)>,
    /// id -> (due time, repeat interval).
    live: HashMap<u64, (Instant, Option<Duration>)>,
}

/// Rejections seen by the tracker and not yet handled: (promise identity, description).
type Rejections = Rc<RefCell<Vec<(usize, String)>>>;

/// One VM Instance with the backend loaded.
struct Instance {
    /// The prelude's host object.
    ///
    /// Fix (runtime abort on reload, `JS_FreeRuntime` gc_obj_list assertion): fields drop
    /// in declaration order, and this handle must be released before the context and the
    /// runtime are freed, so it is declared first.
    host: Persistent<Object<'static>>,
    /// Context; declared before the runtime so it drops first.
    ctx: Context,
    /// The VM runtime (heap, GC, jobs, limits, interrupt).
    _rt: Runtime,
    /// Epoch.
    epoch: u64,
    /// Timers.
    timers: Rc<RefCell<Timers>>,
    /// Budget deadline the interrupt handler checks.
    deadline: Rc<Cell<Option<Instant>>>,
    /// Budget per message.
    budget: Duration,
    /// Unhandled rejections.
    rejections: Rejections,
}

/// The thread body: one receive per step, then due timers, then microtasks.
///
/// # Arguments
/// * `rx` - host messages.
/// * `out` - host callback.
/// * `natives` - native router.
fn run(rx: Receiver<ToJs>, out: Arc<dyn Fn(FromJs) + Send + Sync>, natives: Arc<dyn NativeRouter>) {
    let (done_tx, done_rx) = unbounded::<Completion>();
    let mut instance: Option<Instance> = None;
    loop {
        let wait = instance
            .as_ref()
            .and_then(Instance::next_due)
            .map(|due| due.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(3600));
        select! {
            recv(rx) -> msg => match msg {
                Ok(ToJs::Stop) | Err(_) => break,
                Ok(ToJs::Load { backend, config, restore, epoch }) => {
                    if let Some(old) = instance.take() {
                        let old_epoch = old.epoch;
                        drop(old);
                        natives.release_epoch(old_epoch);
                    }
                    match Instance::load(&backend, config, &restore, epoch, &out, &natives, &done_tx) {
                        Ok(new) => {
                            instance = Some(new);
                            out(FromJs::Loaded { epoch, error: None });
                        }
                        Err(error) => out(FromJs::Loaded { epoch, error: Some(error) }),
                    }
                }
                Ok(ToJs::Envelope(envelope)) => {
                    // An envelope for another epoch is late or early: the epoch blocks it (6.2).
                    if let Some(inst) = instance.as_ref().filter(|i| i.epoch == envelope.epoch) {
                        let json = envelope.to_json();
                        inst.call_host(&out, "dispatch", |f| f.call::<_, ()>((json,)));
                    }
                }
            },
            recv(done_rx) -> done => if let (Ok(done), Some(inst)) = (done, instance.as_ref()) {
                if done.epoch == inst.epoch {
                    let (ok, json) = match done.result {
                        Ok(Value::Null) => (true, String::new()),
                        Ok(value) => (true, value.to_string()),
                        Err(message) => (false, Value::String(message).to_string()),
                    };
                    inst.call_host(&out, "settle", |f| f.call::<_, ()>((done.id as f64, ok, json)));
                }
            },
            default(wait) => {}
        }
        if let Some(inst) = instance.as_ref() {
            inst.fire_due(&out);
        }
    }
    if let Some(old) = instance.take() {
        let epoch = old.epoch;
        drop(old);
        natives.release_epoch(epoch);
    }
}

impl Instance {
    /// Creates the VM, installs ops and built-in modules, and evaluates the backend.
    ///
    /// # Arguments
    /// * `backend` - artifact source.
    /// * `config` - host configuration.
    /// * `restore` - carried-over control values.
    /// * `epoch` - epoch of this Instance.
    /// * `out` - host callback.
    /// * `natives` - native router.
    /// * `done_tx` - completion channel for native calls.
    ///
    /// # Returns
    /// The Instance.
    ///
    /// # Errors
    /// The VM could not be created, or the backend threw while loading.
    fn load(
        backend: &str,
        config: Arc<HostConfig>,
        restore: &Value,
        epoch: u64,
        out: &Arc<dyn Fn(FromJs) + Send + Sync>,
        natives: &Arc<dyn NativeRouter>,
        done_tx: &Sender<Completion>,
    ) -> Result<Instance, String> {
        let rt = Runtime::new().map_err(|e| e.to_string())?;
        rt.set_memory_limit((config.limits.heap_mb as usize) << 20);
        let deadline: Rc<Cell<Option<Instant>>> = Rc::default();
        let check = deadline.clone();
        rt.set_interrupt_handler(Some(Box::new(move || check.get().is_some_and(|d| Instant::now() > d))));
        rt.set_loader(
            BuiltinResolver::default().with_module("@desktop/bridge").with_module("@desktop/api"),
            BuiltinLoader::default().with_module("@desktop/bridge", BRIDGE_MODULE).with_module("@desktop/api", API_MODULE),
        );
        let rejections: Rejections = Rc::default();
        rt.set_host_promise_rejection_tracker(Some(tracker(rejections.clone())));
        let ctx = Context::full(&rt).map_err(|e| e.to_string())?;
        let timers: Rc<RefCell<Timers>> = Rc::default();
        let budget = Duration::from_millis(config.limits.budget_ms);

        let host = ctx.with(|ctx| -> Result<Persistent<Object<'static>>, String> {
            let ops = install_ops(&ctx, epoch, out, natives, done_tx, &config, &timers).map_err(|e| describe(&ctx, e))?;
            let prelude: Function = ctx.eval(PRELUDE).map_err(|e| describe(&ctx, e))?;
            let host: Object = prelude.call((ops,)).map_err(|e| describe(&ctx, e))?;
            let set_restore: Function = host.get("setRestore").map_err(|e| describe(&ctx, e))?;
            set_restore.call::<_, ()>((restore.to_string(),)).map_err(|e| describe(&ctx, e))?;
            deadline.set(Some(Instant::now() + budget));
            let evaluated = Module::declare(ctx.clone(), "backend.js", backend).and_then(|m| m.eval());
            let result = match evaluated {
                Ok((_, promise)) => {
                    while ctx.execute_pending_job() {}
                    match promise.result::<rquickjs::Value>() {
                        Some(Err(e)) => Err(describe(&ctx, e)),
                        Some(Ok(_)) => Ok(()),
                        None => Err("backend top-level await did not settle within its budget".into()),
                    }
                }
                Err(e) => Err(describe(&ctx, e)),
            };
            deadline.set(None);
            result?;
            Ok(Persistent::save(&ctx, host))
        })?;
        let instance = Instance { ctx, _rt: rt, host, epoch, timers, deadline, budget, rejections };
        instance.after_turn(out);
        Ok(instance)
    }

    /// Calls one host-object method under the execution budget, then drains microtasks,
    /// flushes property writes and reports unhandled rejections.
    ///
    /// # Arguments
    /// * `out` - host callback.
    /// * `name` - host method.
    /// * `call` - performs the call.
    fn call_host<F>(&self, out: &Arc<dyn Fn(FromJs) + Send + Sync>, name: &str, call: F)
    where
        F: for<'js> FnOnce(Function<'js>) -> rquickjs::Result<()>,
    {
        self.deadline.set(Some(Instant::now() + self.budget));
        self.ctx.with(|ctx| {
            let result = self.host.clone().restore(&ctx).and_then(|host| host.get::<_, Function>(name)).and_then(call);
            if let Err(e) = result {
                out(FromJs::Error { epoch: self.epoch, message: describe(&ctx, e) });
            }
        });
        self.after_turn(out);
    }

    /// Drains jobs under the budget, flushes the outbox and reports rejections.
    ///
    /// # Arguments
    /// * `out` - host callback.
    fn after_turn(&self, out: &Arc<dyn Fn(FromJs) + Send + Sync>) {
        self.deadline.set(Some(Instant::now() + self.budget));
        self.ctx.with(|ctx| {
            while ctx.execute_pending_job() {}
            let flushed = self.host.clone().restore(&ctx).and_then(|h| h.get::<_, Function>("flush")).and_then(|f| f.call::<_, ()>(()));
            if let Err(e) = flushed {
                out(FromJs::Error { epoch: self.epoch, message: describe(&ctx, e) });
            }
            while ctx.execute_pending_job() {}
        });
        self.deadline.set(None);
        for (_, message) in self.rejections.borrow_mut().drain(..) {
            out(FromJs::Error { epoch: self.epoch, message: format!("unhandled rejection: {message}") });
        }
    }

    /// Earliest timer due time.
    ///
    /// # Returns
    /// The time, if any timer is live.
    fn next_due(&self) -> Option<Instant> {
        self.timers.borrow().due.first().map(|(t, _)| *t)
    }

    /// Runs every timer that is due.
    ///
    /// # Arguments
    /// * `out` - host callback.
    fn fire_due(&self, out: &Arc<dyn Fn(FromJs) + Send + Sync>) {
        let now = Instant::now();
        let due: Vec<u64> = {
            let mut timers = self.timers.borrow_mut();
            let ready: Vec<(Instant, u64)> = timers.due.iter().take_while(|(t, _)| *t <= now).copied().collect();
            for key in &ready {
                timers.due.remove(key);
                let id = key.1;
                match timers.live.get(&id).and_then(|(_, repeat)| *repeat) {
                    // Rescheduled before the callback runs, so clearInterval inside it wins.
                    Some(every) => {
                        timers.due.insert((now + every, id));
                        timers.live.insert(id, (now + every, Some(every)));
                    }
                    None => {
                        timers.live.remove(&id);
                    }
                }
            }
            ready.into_iter().map(|(_, id)| id).collect()
        };
        for id in due {
            self.call_host(out, "fire", |f| f.call::<_, ()>((id as f64,)));
        }
    }
}

/// Builds the engine's rejection tracker. A rejection that gets a handler later in the
/// same turn is removed again, so only genuinely unhandled ones are reported.
///
/// # Arguments
/// * `list` - pending rejections.
///
/// # Returns
/// The tracker.
fn tracker(list: Rejections) -> rquickjs::runtime::RejectionTracker {
    /// Pins the closure to the signature the engine expects.
    ///
    /// # Arguments
    /// * `f` - the closure.
    ///
    /// # Returns
    /// The boxed tracker.
    fn typed<F>(f: F) -> rquickjs::runtime::RejectionTracker
    where
        F: for<'a> Fn(Ctx<'a>, rquickjs::Value<'a>, rquickjs::Value<'a>, bool) + 'static,
    {
        Box::new(f)
    }
    typed(move |ctx, promise, reason, handled| {
        // SAFETY: the promise pointer is used only as an identity key, never dereferenced.
        let id = unsafe { rquickjs::qjs::JS_VALUE_GET_PTR(promise.as_raw()) } as usize;
        let mut list = list.borrow_mut();
        if handled {
            list.retain(|(p, _)| *p != id);
        } else {
            let text = match reason.as_exception() {
                Some(e) => format!("{}\n{}", e.message().unwrap_or_default(), e.stack().unwrap_or_default()),
                None => ctx.json_stringify(reason).ok().flatten().and_then(|s| s.to_string().ok()).unwrap_or_default(),
            };
            list.push((id, text));
        }
    })
}

/// Turns an engine error into text, taking the pending exception.
///
/// # Arguments
/// * `ctx` - context.
/// * `error` - the error.
///
/// # Returns
/// The description; an interrupt reads as an exceeded budget.
fn describe(ctx: &Ctx<'_>, error: rquickjs::Error) -> String {
    let text = CaughtError::from_error(ctx, error).to_string();
    if text.contains("interrupted") {
        format!("execution budget exceeded ({text})")
    } else {
        text
    }
}

/// Installs the native ops the prelude receives.
///
/// | op                         | does                                              |
/// |----------------------------|---------------------------------------------------|
/// | `send(json)`               | backend envelope (update, response) to the host   |
/// | `native(ns, method, args)` | starts a permitted native call, returns its id    |
/// | `timer(ms, repeat)`        | schedules a timer, returns its id                 |
/// | `clearTimer(id)`           | cancels a timer                                   |
/// | `log(level, text)`         | console line                                      |
/// | `error(text)`              | reports a handler error                           |
///
/// # Arguments
/// * `ctx` - context.
/// * `epoch` - Instance epoch, stamped on everything sent.
/// * `out` - host callback.
/// * `natives` - native router.
/// * `done_tx` - completion channel.
/// * `config` - host configuration (permissions).
/// * `timers` - timer table.
///
/// # Returns
/// The ops object.
///
/// # Errors
/// An engine error while creating functions.
fn install_ops<'js>(
    ctx: &Ctx<'js>,
    epoch: u64,
    out: &Arc<dyn Fn(FromJs) + Send + Sync>,
    natives: &Arc<dyn NativeRouter>,
    done_tx: &Sender<Completion>,
    config: &Arc<HostConfig>,
    timers: &Rc<RefCell<Timers>>,
) -> rquickjs::Result<Object<'js>> {
    let ops = Object::new(ctx.clone())?;

    let o = out.clone();
    ops.set(
        "send",
        Function::new(ctx.clone(), move |json: String| match serde_json::from_str::<Value>(&json) {
            Ok(mut v) => {
                v["version"] = VERSION.into();
                v["epoch"] = epoch.into();
                match serde_json::from_value::<Envelope>(v) {
                    Ok(env) if matches!(env.kind, Kind::Update | Kind::Response) => o(FromJs::Envelope(env)),
                    Ok(env) => o(FromJs::Error { epoch, message: format!("backend may not send {:?}", env.kind) }),
                    Err(e) => o(FromJs::Error { epoch, message: format!("bad backend envelope: {e}") }),
                }
            }
            Err(e) => o(FromJs::Error { epoch, message: format!("bad backend envelope: {e}") }),
        })?,
    )?;

    let (n, tx, cfg) = (natives.clone(), done_tx.clone(), config.clone());
    let next_call = Cell::new(0u64);
    ops.set(
        "native",
        Function::new(ctx.clone(), move |ns: String, method: String, args: String| -> f64 {
            let id = next_call.get() + 1;
            next_call.set(id);
            let done = Completer { tx: tx.clone(), id, epoch };
            let args = match serde_json::from_str::<Vec<Value>>(&args) {
                Ok(args) => args,
                Err(e) => {
                    done.complete(Err(format!("bad arguments: {e}")));
                    return id as f64;
                }
            };
            // Decision: permissions are checked here, on the JS thread, before any
            // service sees the call; services never trust the backend.
            if cfg.allows_native(&ns, &method) {
                n.call(NativeCall { ns, method, args, done });
            } else {
                done.complete(Err(format!("native method {ns}.{method} is not permitted by the host configuration")));
            }
            id as f64
        })?,
    )?;

    let t = timers.clone();
    ops.set(
        "timer",
        Function::new(ctx.clone(), move |ms: f64, repeat: bool| -> f64 {
            let mut timers = t.borrow_mut();
            timers.next += 1;
            let id = timers.next;
            // A repeating timer of 0 ms would spin the thread; it is clamped to 1 ms.
            let every = Duration::from_micros((ms.max(if repeat { 1.0 } else { 0.0 }) * 1000.0) as u64);
            let due = Instant::now() + every;
            timers.due.insert((due, id));
            timers.live.insert(id, (due, repeat.then_some(every)));
            id as f64
        })?,
    )?;

    let t = timers.clone();
    ops.set(
        "clearTimer",
        Function::new(ctx.clone(), move |id: f64| {
            let mut timers = t.borrow_mut();
            if let Some((due, _)) = timers.live.remove(&(id as u64)) {
                timers.due.remove(&(due, id as u64));
            }
        })?,
    )?;

    let o = out.clone();
    ops.set(
        "log",
        Function::new(ctx.clone(), move |level: String, message: String| o(FromJs::Log { level, message }))?,
    )?;

    let o = out.clone();
    ops.set("error", Function::new(ctx.clone(), move |message: String| o(FromJs::Error { epoch, message }))?)?;
    Ok(ops)
}

#[cfg(test)]
pub(crate) mod tests;
