//! [`Backend`]: the thread that runs one program instance after another
//! (start, retire, reload), the [`Watchdog`] that interrupts a stuck turn, and
//! [`Settler`], which settles native calls from the shell thread.

use crate::*;

/// The live instance's epoch and the handle that posts to its loop.
pub(crate) type Slot = Arc<Mutex<Option<(u64, Poster<Engine>)>>>;

/// What the control channel carries.
pub(crate) enum Control {
    /// Replace the running backend.
    Load(Program),
    /// End the thread.
    Stop,
}

/// Interrupts a backend whose JS entry outlives the budget.
pub(crate) struct Watchdog {
    /// Tells the thread to end.
    pub(crate) stop: Arc<AtomicBool>,
    /// The thread.
    pub(crate) thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    /// Starts watching a Runtime.
    ///
    /// The thread samples the pulse twice per budget. The same odd value for a
    /// whole budget means one JS entry has been running that long.
    ///
    /// # Arguments
    /// * `pulse` - the Runtime's pulse.
    /// * `interrupt` - the instance's termination handle.
    /// * `budget` - the longest one entry may run.
    ///
    /// # Returns
    /// The watchdog; dropping it ends the thread.
    pub(crate) fn start(pulse: Arc<AtomicU64>, interrupt: Interrupt, budget: Duration) -> Watchdog {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::Builder::new()
            .name("lunarstorm-budget".into())
            .spawn(move || {
                let mut seen = pulse.load(Ordering::Relaxed);
                let mut since = Instant::now();
                while !stopped.load(Ordering::Relaxed) {
                    std::thread::park_timeout(budget / 2);
                    let now = pulse.load(Ordering::Relaxed);
                    if now != seen || now.is_multiple_of(2) {
                        seen = now;
                        since = Instant::now();
                    } else if since.elapsed() >= budget {
                        interrupt.request();
                        since = Instant::now();
                    }
                }
            })
            .expect("spawn the budget watchdog");
        Watchdog { stop, thread: Some(thread) }
    }
}

impl Drop for Watchdog {
    /// Ends the thread and waits for it.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

/// One running backend.
pub(crate) struct Instance<S: Shell> {
    /// Its Runtime.
    pub(crate) runtime: Runtime<Engine, DesktopHost<S>>,
    /// Its epoch.
    pub(crate) epoch: u64,
    /// Its budget watchdog; dropped (and joined) with the instance.
    pub(crate) _watchdog: Watchdog,
}

/// Starts a backend: a new Runtime, its bindings and the entry module.
///
/// # Arguments
/// * `program` - what to run.
/// * `shell` - the desktop process.
/// * `slot` - where the new instance's poster is published.
///
/// # Returns
/// The instance.
///
/// # Errors
/// Why the backend did not load. Nothing of it is left running.
pub(crate) fn start<S: Shell>(program: Program, shell: &Arc<S>, slot: &Slot) -> Result<Instance<S>, String> {
    let Program { epoch, code, restore, memory, budget, roots, environment, name } = program;
    let host = DesktopHost {
        shell: shell.clone(),
        epoch,
        name,
        roots,
        environment,
        loading: Cell::new(true),
        failure: RefCell::new(None),
    };
    let mut runtime = Runtime::<Engine, _>::new(host).map_err(|error| format!("cannot create the event loop: {error}"))?;
    runtime.limits(Limits { memory, ..Limits::default() });
    runtime.register(bindings(shell, epoch, restore));
    runtime.start()?;
    // Events arrive as posted tasks, so the loop sleeps instead of going idle.
    runtime.hold(true);
    let interrupt = runtime.interrupt().ok_or("the runtime has no instance")?;
    let watchdog = Watchdog::start(runtime.pulse(), interrupt, budget);

    let loaded = runtime.load(Source::Text { name: String::from("backend.js"), code: code.to_string() });
    runtime.host().loading.set(false);
    let failure = runtime.host().failure.borrow_mut().take();
    if let Some(error) = failure.or(loaded.err()) {
        clear();
        runtime.stop();
        return Err(error);
    }
    *slot.lock().expect("slot lock") = Some((epoch, runtime.poster()));
    Ok(Instance { runtime, epoch, _watchdog: watchdog })
}

/// Ends the running backend, if any, and tells the shell its epoch is retired.
///
/// # Arguments
/// * `live` - the running backend.
/// * `shell` - the desktop process.
/// * `slot` - cleared first, so nothing more is posted to the dying loop.
pub(crate) fn retire<S: Shell>(live: &mut Option<Instance<S>>, shell: &Arc<S>, slot: &Slot) {
    slot.lock().expect("slot lock").take();
    if let Some(mut instance) = live.take() {
        // The desktop modules' state holds engine handles and must go before the instance does.
        clear();
        instance.runtime.stop();
        let epoch = instance.epoch;
        drop(instance);
        shell.retired(epoch);
    }
}

/// The backend thread: runs the live Runtime's loop and acts on control messages.
///
/// # Arguments
/// * `control` - reloads and the stop request.
/// * `shell` - the desktop process.
/// * `slot` - where the live instance's poster is published.
pub(crate) fn run<S: Shell>(control: Receiver<Control>, shell: Arc<S>, slot: Slot) {
    let mut live: Option<Instance<S>> = None;
    loop {
        let message = match live.as_mut() {
            // Nothing runs: only a control message can change that.
            None => match control.recv() {
                Ok(message) => message,
                Err(_) => break,
            },
            Some(instance) => match control.try_recv() {
                Ok(message) => message,
                Err(TryRecvError::Empty) => {
                    // Sleeps in the kernel until an event is posted, a timer is
                    // due or I/O is ready; a control message wakes it too.
                    instance.runtime.poll(Wait::Block);
                    continue;
                }
                Err(TryRecvError::Disconnected) => break,
            },
        };
        match message {
            Control::Stop => break,
            Control::Load(program) => {
                retire(&mut live, &shell, &slot);
                let epoch = program.epoch;
                match start(program, &shell, &slot) {
                    Ok(instance) => {
                        live = Some(instance);
                        shell.loaded(epoch, None);
                    }
                    Err(error) => shell.loaded(epoch, Some(&error)),
                }
            }
        }
    }
    retire(&mut live, &shell, &slot);
}

/// Delivers native-call results to the backend from any thread.
pub struct Settler<S> {
    /// The desktop process, for errors.
    pub(crate) shell: Arc<S>,
    /// The live instance's poster.
    pub(crate) slot: Slot,
}

impl<S> Clone for Settler<S> {
    /// # Returns
    /// Another handle to the same backend thread.
    fn clone(&self) -> Self {
        Settler { shell: self.shell.clone(), slot: self.slot.clone() }
    }
}

impl<S: Shell> Settler<S> {
    /// Finishes a native call. A result for a retired epoch is dropped.
    ///
    /// # Arguments
    /// * `epoch` - the epoch [`Shell::native`] was called with.
    /// * `call` - the call id it was called with.
    /// * `result` - the value as JSON (empty for `undefined`), or the error
    ///   message.
    pub fn settle(&self, epoch: u64, call: u64, result: Result<String, String>) {
        let guard = self.slot.lock().expect("slot lock");
        let Some((live, poster)) = guard.as_ref() else { return };
        if *live != epoch {
            return;
        }
        let shell = self.shell.clone();
        poster.post(move |scope| {
            hook(scope, &*shell, epoch, || settle(scope, call, result));
        });
    }
}

/// The backend of a desktop application: a thread that runs one Runtime per
/// epoch. Dropping it stops the backend.
pub struct Backend<S: Shell> {
    /// Reloads and the stop request.
    pub(crate) control: Sender<Control>,
    /// The live instance's epoch and poster.
    pub(crate) slot: Slot,
    /// The desktop process.
    pub(crate) shell: Arc<S>,
    /// The backend thread.
    pub(crate) thread: Option<JoinHandle<()>>,
}

impl<S: Shell> Backend<S> {
    /// Starts the backend thread. It idles until the first [`Backend::load`].
    ///
    /// # Arguments
    /// * `shell` - the desktop process.
    ///
    /// # Returns
    /// The handle.
    pub fn spawn(shell: Arc<S>) -> Backend<S> {
        let (control, receiver) = channel();
        let slot: Slot = Arc::default();
        let (thread_shell, thread_slot) = (shell.clone(), slot.clone());
        let thread = std::thread::Builder::new()
            .name("lunarstorm-backend".into())
            .spawn(move || run(receiver, thread_shell, thread_slot))
            .expect("spawn the backend thread");
        Backend { control, slot, shell, thread: Some(thread) }
    }

    /// Sends a control message and wakes the loop so it is seen.
    ///
    /// # Arguments
    /// * `message` - the message.
    pub(crate) fn control(&self, message: Control) {
        if self.control.send(message).is_ok() {
            if let Some((_, poster)) = self.slot.lock().expect("slot lock").as_ref() {
                poster.post(|_| {});
            }
        }
    }

    /// Starts a backend, replacing the running one. [`Shell::loaded`] reports the
    /// outcome; the previous epoch is retired first.
    ///
    /// # Arguments
    /// * `program` - what to run.
    pub fn load(&self, program: Program) {
        self.control(Control::Load(program));
    }

    /// Delivers an `event`, `request` or `cancel` envelope to the backend. An
    /// envelope for any epoch but the live one is late or early and is dropped
    /// (spec 6.2).
    ///
    /// # Arguments
    /// * `epoch` - the epoch the envelope is for.
    /// * `envelope` - the envelope as JSON.
    pub fn dispatch(&self, epoch: u64, envelope: String) {
        let guard = self.slot.lock().expect("slot lock");
        let Some((live, poster)) = guard.as_ref() else { return };
        if *live != epoch {
            return;
        }
        let shell = self.shell.clone();
        poster.post(move |scope| {
            hook(scope, &*shell, epoch, || dispatch(scope, &envelope));
        });
    }

    /// # Returns
    /// A handle that settles native calls from any thread.
    pub fn settler(&self) -> Settler<S> {
        Settler { shell: self.shell.clone(), slot: self.slot.clone() }
    }
}

impl<S: Shell> Drop for Backend<S> {
    /// Stops the backend and waits for its thread.
    fn drop(&mut self) {
        self.control(Control::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
