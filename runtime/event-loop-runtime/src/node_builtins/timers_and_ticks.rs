//! `node:timers`, `setImmediate`, `queueMicrotask` and `process.nextTick`.
//!
//! The callbacks wait in Rust ([`Queues`]) as persistent handles; the reactor
//! owns the timers themselves (`Io::timer_start`). Ordering is the glue's,
//! pinned by `node_behaviour_pinned`: nextTick callbacks, then promise jobs, then timers,
//! then immediates; a 0 ms timeout armed with an immediate fires after it.
//!
//! Decision: the scheduling functions are installed without their declaration's
//! argument check. A script sees the glue's behaviour unchanged: the delay is
//! coerced with `Number()` (NaN means 0), extra arguments are passed on, and a
//! callback that is not a function throws the pinned
//! `TypeError: The "callback" argument must be a function`. The declarations
//! carry the types a script should use.

use super::{arg, f, p, Node, Slot};
use crate::host_bindings_registry::declared;
use crate::host_services_trait::Host;
use crate::reactor_io_state::Shared;
use lunarstorm_engine_contract::{Call, ErrorKind, FnDecl, Item, Kind, ModuleDecl, Outcome, Param, Scope, Sig, Ty, Vm};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

/// A timer id is `key * 2^32 + seq`: `key` indexes [`Queues::armed`] and the
/// reactor's slab, `seq` makes a stale id (already fired or cleared) harmless.
const TIMER_SHIFT: f64 = 4_294_967_296.0;

/// What a scheduled callback receives.
const CALLBACK_SIG: Sig = Sig { params: &[], rest: Some(p("args", Ty::Any, "the extra arguments")), returns: Ty::Unknown, about: "A scheduled callback." };
/// The callback parameter.
const CALLBACK: Param = p("callback", Ty::Function(&CALLBACK_SIG), "the function to call");
/// The delay parameter.
const DELAY: Param = p("ms", Ty::Optional(&Ty::Number), "delay in milliseconds; coerced with Number(), NaN means 0");
/// Extra arguments for the callback.
const EXTRA: Param = p("args", Ty::Any, "passed to the callback");
/// A timer id.
const TIMER: Ty = Ty::Number;
/// An immediate's handle.
const IMMEDIATE: Ty = Ty::Named("Immediate");

const SET_TIMEOUT: FnDecl = f("setTimeout", &[CALLBACK, DELAY], Some(EXTRA), TIMER, "Calls a function once after a delay.");
const SET_INTERVAL: FnDecl = f("setInterval", &[CALLBACK, DELAY], Some(EXTRA), TIMER, "Calls a function every delay.");
const SET_IMMEDIATE: FnDecl = f("setImmediate", &[CALLBACK], Some(EXTRA), IMMEDIATE, "Calls a function in the next check phase.");
const CLEAR_TIMEOUT: FnDecl = f("clearTimeout", &[p("id", Ty::Any, "a timer id; anything else is ignored")], None, Ty::Undefined, "Cancels a timer.");
const CLEAR_INTERVAL: FnDecl = f("clearInterval", &[p("id", Ty::Any, "a timer id; anything else is ignored")], None, Ty::Undefined, "Cancels a timer.");
const CLEAR_IMMEDIATE: FnDecl = f("clearImmediate", &[p("handle", Ty::Any, "what setImmediate returned")], None, Ty::Undefined, "Cancels an immediate.");
const QUEUE_MICROTASK: FnDecl = f("queueMicrotask", &[CALLBACK], None, Ty::Undefined, "Queues a promise job.");
/// `process.nextTick`, declared with `process`.
pub(crate) const NEXT_TICK: FnDecl = f("nextTick", &[CALLBACK], Some(EXTRA), Ty::Undefined, "Queues a callback to run before promise jobs.");

/// The module's functions, in its key order.
const MODULE: [FnDecl; 6] = [SET_TIMEOUT, SET_INTERVAL, SET_IMMEDIATE, CLEAR_TIMEOUT, CLEAR_INTERVAL, CLEAR_IMMEDIATE];

/// # Returns
/// The declaration of `node:timers`.
pub(crate) fn declare() -> ModuleDecl {
    ModuleDecl { specifier: "node:timers", items: MODULE.into_iter().map(Item::Function).collect(), about: "Timers and immediates." }
}

/// # Returns
/// The names this module puts on the global object.
pub(crate) fn globals() -> Vec<Item> {
    MODULE.into_iter().chain([QUEUE_MICROTASK]).map(Item::Function).collect()
}

/// A callback and its extra arguments, kept across loop turns.
struct Callback<V: Vm> {
    /// The function.
    function: V::Persistent,
    /// Its extra arguments.
    args: Vec<V::Persistent>,
}

impl<V: Vm> Callback<V> {
    /// Keeps a callback.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `function` - the function.
    /// * `args` - its extra arguments.
    ///
    /// # Returns
    /// The kept callback.
    fn keep<'s>(scope: &V::Scope<'s>, function: &V::Value<'s>, args: &[V::Value<'s>]) -> Self {
        Self { function: scope.persist(function), args: args.iter().map(|value| scope.persist(value)).collect() }
    }

    /// # Arguments
    /// * `scope` - the context handle.
    ///
    /// # Returns
    /// The function and its arguments as scoped values, so the call runs with no
    /// borrow of the queues held.
    fn local<'s>(&self, scope: &V::Scope<'s>) -> (V::Value<'s>, Vec<V::Value<'s>>) {
        (scope.local(&self.function), self.args.iter().map(|value| scope.local(value)).collect())
    }
}

/// One `setImmediate` callback.
struct Immediate<V: Vm> {
    /// What to call.
    callback: Callback<V>,
    /// Cleared by `clearImmediate`; shared with the handle the script holds.
    live: Rc<Cell<bool>>,
}

/// The Rust data of an immediate's handle (a host object).
struct Handle(Rc<Cell<bool>>);

/// The callbacks waiting for the loop.
pub(crate) struct Queues<V: Vm> {
    /// Callbacks of armed timers, by key.
    armed: RefCell<Vec<Option<Callback<V>>>>,
    /// Immediates queued for the next check phase.
    queue: RefCell<Vec<Immediate<V>>>,
    /// The batch the reactor is running, one per `runImmediate`.
    batch: RefCell<VecDeque<Immediate<V>>>,
    /// `process.nextTick` callbacks.
    ticks: RefCell<VecDeque<Callback<V>>>,
    /// Whether `ticks` is draining. A callback queued during the drain is picked up
    /// by the drain itself, so the loop's wake-up flag is not set for it. A
    /// callback that throws leaves this set, as in the glue: that is an uncaught
    /// error, after which the loop runs no more JS.
    ticking: Cell<bool>,
}

impl<V: Vm> Default for Queues<V> {
    /// # Returns
    /// Empty queues.
    fn default() -> Self {
        Self {
            armed: RefCell::default(),
            queue: RefCell::default(),
            batch: RefCell::default(),
            ticks: RefCell::default(),
            ticking: Cell::new(false),
        }
    }
}

impl<V: Vm> Queues<V> {
    /// Queues a `process.nextTick` callback from Rust.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `tick` - the loop's wake-up flag (`Shared::tick`).
    /// * `function` - the callback.
    /// * `args` - its arguments.
    pub(crate) fn next_tick<'s>(&self, scope: &V::Scope<'s>, tick: &Cell<bool>, function: &V::Value<'s>, args: &[V::Value<'s>]) {
        let mut ticks = self.ticks.borrow_mut();
        if !self.ticking.get() && ticks.is_empty() {
            tick.set(true);
        }
        ticks.push_back(Callback::keep(scope, function, args));
    }

    /// Runs the tick queue until it is empty. Called by the loop after every JS
    /// entry.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    ///
    /// # Errors
    /// What a callback throws; the rest stay queued.
    pub(crate) fn run_ticks<'s>(&self, scope: &V::Scope<'s>) -> Outcome<()> {
        self.ticking.set(true);
        loop {
            let next = self.ticks.borrow_mut().pop_front();
            let Some(next) = next else { break };
            let (function, args) = next.local(scope);
            drop(next);
            scope.call(&function, &args)?;
        }
        self.ticking.set(false);
        Ok(())
    }

    /// Runs one immediate of the current batch; the loop calls this once per
    /// callback so promise jobs run between immediates, as in Node.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    ///
    /// # Returns
    /// Whether the batch has more.
    ///
    /// # Errors
    /// What the callback throws; it is already off the queue.
    pub(crate) fn run_immediate<'s>(&self, scope: &V::Scope<'s>) -> Outcome<bool> {
        let (next, more) = {
            let mut batch = self.batch.borrow_mut();
            if batch.is_empty() {
                batch.extend(self.queue.borrow_mut().drain(..));
            }
            let next = batch.pop_front();
            (next, !batch.is_empty())
        };
        if let Some(next) = next.filter(|next| next.live.get()) {
            let (function, args) = next.callback.local(scope);
            drop(next);
            scope.call(&function, &args)?;
        }
        Ok(more)
    }

    /// Calls a fired timer's callback; a one-shot timer's callback is dropped first.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `key` - the timer's slab key.
    /// * `one_shot` - whether the timer is done.
    ///
    /// # Errors
    /// What the callback throws.
    pub(crate) fn fire_timer<'s>(&self, scope: &V::Scope<'s>, key: usize, one_shot: bool) -> Outcome<()> {
        let local = {
            let mut armed = self.armed.borrow_mut();
            match armed.get_mut(key) {
                Some(slot) if one_shot => slot.take().map(|kept| kept.local(scope)),
                Some(slot) => slot.as_ref().map(|kept| kept.local(scope)),
                None => None,
            }
        };
        if let Some((function, args)) = local {
            scope.call(&function, &args)?;
        }
        Ok(())
    }

    /// Drops every kept callback. Runs before the VM instance is destroyed.
    pub(crate) fn clear(&self) {
        let armed = std::mem::take(&mut *self.armed.borrow_mut());
        let queue = std::mem::take(&mut *self.queue.borrow_mut());
        let batch = std::mem::take(&mut *self.batch.borrow_mut());
        let ticks = std::mem::take(&mut *self.ticks.borrow_mut());
        drop((armed, queue, batch, ticks));
    }
}

/// Throws the glue's error for a callback that is not a function.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `callback` - the argument.
///
/// # Errors
/// `TypeError: The "callback" argument must be a function`.
fn callback<'s, V: Vm>(scope: &V::Scope<'s>, callback: &V::Value<'s>) -> Outcome<()> {
    if scope.kind(callback) == Kind::Function {
        Ok(())
    } else {
        Err(scope.throw_error(ErrorKind::Type, "The \"callback\" argument must be a function"))
    }
}

/// Arms a timer.
///
/// # Arguments
/// * `call` - `(callback, ms, ...args)`.
/// * `shared` - loop state: the reactor's timers.
/// * `node` - the kept builtins and queues.
/// * `repeat` - whether the timer repeats.
///
/// # Returns
/// The timer id.
///
/// # Errors
/// A `TypeError` for a callback that is not a function, a `RangeError` when too
/// many timers are active, and what `Number()` throws for the delay.
fn arm<'s, V: Vm, H: Host>(call: &Call<'s, V>, shared: &Shared<H>, node: &Node<V>, repeat: bool) -> Outcome<V::Value<'s>> {
    let (scope, args) = (call.scope(), call.args());
    let function = arg::<V>(scope, args, 0);
    callback::<V>(scope, &function)?;
    let delay = arg::<V>(scope, args, 1);
    let ms = match scope.kind(&delay) {
        Kind::Number => scope.as_number(&delay).unwrap_or(0.0),
        Kind::Undefined => 0.0,
        _ => scope.as_number(&node.call(scope, Slot::Number, &[delay])?).unwrap_or(0.0),
    };
    // `+ms || 0`: NaN becomes 0.
    let ms = if ms.is_nan() { 0.0 } else { ms };
    let Some(id) = shared.io.borrow_mut().timer_start(ms, repeat, shared.host.monotonic()) else {
        return Err(scope.throw_error(ErrorKind::Range, "too many active timers"));
    };
    let key = (id / TIMER_SHIFT).floor() as usize;
    let kept = Callback::keep(scope, &function, args.get(2..).unwrap_or(&[]));
    let mut armed = node.timers.armed.borrow_mut();
    if armed.len() <= key {
        armed.resize_with(key + 1, || None);
    }
    armed[key] = Some(kept);
    Ok(scope.number(id))
}

/// Builds the timer functions and puts them on the global object.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `shared` - loop state: the reactor's timers and the wake-up flags.
/// * `node` - the kept builtins and queues.
/// * `native` - the object the glue receives; `nextTick`, `ticks`,
///   `runImmediate` and `fireTimer` are set on it.
///
/// # Returns
/// The `node:timers` module object; its functions are the globals.
///
/// # Errors
/// [`lunarstorm_engine_contract::Thrown`] when a value cannot be created.
pub(crate) fn install<'s, V: Vm, H: Host>(scope: &V::Scope<'s>, shared: &Rc<Shared<H>>, node: &Rc<Node<V>>, native: &V::Value<'s>) -> Outcome<V::Value<'s>> {
    let global = scope.global();
    let module = scope.object()?;

    let (s, n) = (shared.clone(), node.clone());
    let set_timeout = scope.function(SET_TIMEOUT.name, move |call: &Call<'_, V>| arm(call, &s, &n, false))?;
    let (s, n) = (shared.clone(), node.clone());
    let set_interval = scope.function(SET_INTERVAL.name, move |call: &Call<'_, V>| arm(call, &s, &n, true))?;

    let (s, n) = (shared.clone(), node.clone());
    let set_immediate = scope.function(SET_IMMEDIATE.name, move |call: &Call<'_, V>| {
        let (scope, args) = (call.scope(), call.args());
        let function = arg::<V>(scope, args, 0);
        callback::<V>(scope, &function)?;
        let live = Rc::new(Cell::new(true));
        let handle = scope.host_object(Handle(live.clone()))?;
        let mut queue = n.timers.queue.borrow_mut();
        if queue.is_empty() {
            s.immediate.set(true);
        }
        queue.push(Immediate { callback: Callback::keep(scope, &function, args.get(1..).unwrap_or(&[])), live });
        Ok(handle)
    })?;

    // Unknown and stale ids are ignored; one function serves both names, as in
    // the glue (`clearTimeout === clearInterval`).
    let (s, n) = (shared.clone(), node.clone());
    let clear_timer = scope.function(CLEAR_TIMEOUT.name, move |call: &Call<'_, V>| {
        let scope = call.scope();
        let id = arg::<V>(scope, call.args(), 0);
        if let Some(id) = scope.as_number(&id) {
            if s.io.borrow_mut().timer_cancel(id) {
                let key = (id / TIMER_SHIFT).floor() as usize;
                let dropped = n.timers.armed.borrow_mut().get_mut(key).and_then(Option::take);
                drop(dropped);
            }
        }
        Ok(scope.undefined())
    })?;

    let clear_immediate = scope.function(CLEAR_IMMEDIATE.name, |call: &Call<'_, V>| {
        let scope = call.scope();
        let handle = arg::<V>(scope, call.args(), 0);
        scope.host_data::<Handle, _>(&handle, |Handle(live)| live.set(false));
        Ok(scope.undefined())
    })?;

    for (name, value) in [
        (SET_TIMEOUT.name, &set_timeout),
        (SET_INTERVAL.name, &set_interval),
        (SET_IMMEDIATE.name, &set_immediate),
        (CLEAR_TIMEOUT.name, &clear_timer),
        (CLEAR_INTERVAL.name, &clear_timer),
        (CLEAR_IMMEDIATE.name, &clear_immediate),
    ] {
        scope.set(&module, name, value.clone())?;
        scope.set(&global, name, value.clone())?;
    }

    // The engine's own `queueMicrotask` wins; this one is the fallback the glue had.
    let existing = scope.get(&global, QUEUE_MICROTASK.name)?;
    if scope.kind(&existing) != Kind::Function {
        let queue_microtask = declared(scope, QUEUE_MICROTASK.name, QUEUE_MICROTASK.sig, |call: &Call<'_, V>| {
            let scope = call.scope();
            let (promise, resolver) = scope.promise()?;
            scope.settle(&resolver, Ok(scope.undefined()))?;
            scope.call_with(&scope.get(&promise, "then")?, &promise, &call.args()[..1])?;
            Ok(scope.undefined())
        })?;
        scope.set(&global, QUEUE_MICROTASK.name, queue_microtask)?;
    }

    let (s, n) = (shared.clone(), node.clone());
    let next_tick = declared(scope, NEXT_TICK.name, NEXT_TICK.sig, move |call: &Call<'_, V>| {
        let (scope, args) = (call.scope(), call.args());
        n.timers.next_tick(scope, &s.tick, &args[0], &args[1..]);
        Ok(scope.undefined())
    })?;
    scope.set(native, "nextTick", next_tick)?;

    let n = node.clone();
    let ticks = scope.function("ticks", move |call: &Call<'_, V>| {
        n.timers.run_ticks(call.scope())?;
        Ok(call.scope().undefined())
    })?;
    scope.set(native, "ticks", ticks)?;
    let n = node.clone();
    let run_immediate = scope.function("runImmediate", move |call: &Call<'_, V>| Ok(call.scope().boolean(n.timers.run_immediate(call.scope())?)))?;
    scope.set(native, "runImmediate", run_immediate)?;
    let n = node.clone();
    let fire_timer = scope.function("fireTimer", move |call: &Call<'_, V>| {
        let (scope, args) = (call.scope(), call.args());
        let key = scope.as_number(&arg::<V>(scope, args, 0)).unwrap_or(-1.0);
        let one_shot = scope.as_bool(&arg::<V>(scope, args, 1)).unwrap_or(true);
        if key >= 0.0 {
            n.timers.fire_timer(scope, key as usize, one_shot)?;
        }
        Ok(scope.undefined())
    })?;
    scope.set(native, "fireTimer", fire_timer)?;
    Ok(module)
}
