//! The optional owner-thread socket reactor. Only its doorbell is transferable.
use crate::{
    arena::{Arena, Key},
    transport::{MessageValidator, Wake},
};
use std::{
    cell::RefCell,
    collections::VecDeque,
    future::Future,
    io,
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::{
    runtime::{Builder, EnterGuard, Handle, Runtime},
    sync::Notify,
    task::{AbortHandle, LocalSet},
};

/// How many times [`OwnerIo::run_ready`] ticks the task set in one call.
///
/// Decision: Tokio runs at most 61 local tasks per tick, so four ticks cover 244 ready
/// connections, and a task that wakes itself on every poll cannot hold the event loop.
const READY_ROUNDS: usize = 4;

/// The waker [`OwnerIo::run_ready`] polls the task set with: it records that ready tasks
/// remain instead of unparking the reactor.
struct More(AtomicBool);

impl std::task::Wake for More {
    /// Records that the task set has more ready tasks.
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Records that the task set has more ready tasks.
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.store(true, Ordering::Relaxed);
    }
}

// The reactor stays on the owner. DNS and certificate work can use this small
// blocking pool, rather than acquiring a network scheduler thread per VM.
const BLOCKING_THREADS: usize = 4;

struct Inbox<M> {
    messages: Arena<Option<M>>,
    ready: VecDeque<Key>,
}

// The mailbox's senders are the builtin facades; without them only the owner side is built.
#[cfg_attr(not(feature = "builtin-facades"), allow(dead_code))]
struct Mailbox<M> {
    inbox: RefCell<Option<Inbox<M>>>,
    wake: Arc<Notify>,
    space: Notify,
    validator: Option<MessageValidator<M>>,
}

#[cfg_attr(not(feature = "builtin-facades"), allow(dead_code))]
enum Rejected<M> {
    Full(M),
    Closed(M),
}

/// An owner-local handle to the VM's socket reactor and bounded event inbox.
pub struct OwnerIoHandle<M> {
    tasks: Weak<LocalSet>,
    runtime: Handle,
    mailbox: Rc<Mailbox<M>>,
}

impl<M> Clone for OwnerIoHandle<M> {
    fn clone(&self) -> Self {
        Self {
            tasks: self.tasks.clone(),
            runtime: self.runtime.clone(),
            mailbox: self.mailbox.clone(),
        }
    }
}

#[cfg_attr(not(feature = "builtin-facades"), allow(dead_code))]
impl<M> OwnerIoHandle<M> {
    pub(crate) fn enter(&self) -> EnterGuard<'_> {
        self.runtime.enter()
    }

    pub(crate) fn spawn(
        &self,
        task: impl Future<Output = ()> + 'static,
    ) -> Result<AbortHandle, &'static str> {
        let tasks = self.tasks.upgrade().ok_or("socket reactor is closed")?;
        Ok(tasks.spawn_local(task).abort_handle())
    }

    fn validate(&self, message: &M) -> bool {
        self.mailbox
            .validator
            .as_ref()
            .is_none_or(|f| f(message).is_ok())
    }

    fn push(&self, message: M) -> Result<(), Rejected<M>> {
        let mut state = self.mailbox.inbox.borrow_mut();
        let Some(inbox) = state.as_mut() else {
            return Err(Rejected::Closed(message));
        };
        let key = inbox
            .messages
            .insert(Some(message))
            .map_err(|data| Rejected::Full(data.expect("unadmitted owner I/O message")))?;
        inbox.ready.push_back(key);
        drop(state);
        self.mailbox.wake.notify_one();
        Ok(())
    }

    pub(crate) fn try_send(&self, message: M) -> Result<(), M> {
        if !self.validate(&message) {
            return Err(message);
        }
        self.push(message).map_err(|error| match error {
            Rejected::Full(message) | Rejected::Closed(message) => message,
        })
    }

    pub(crate) async fn send(&self, mut message: M) -> Result<(), M> {
        if !self.validate(&message) {
            return Err(message);
        }
        loop {
            match self.push(message) {
                Ok(()) => return Ok(()),
                Err(Rejected::Closed(message)) => return Err(message),
                Err(Rejected::Full(value)) => message = value,
            }
            // The inbox and all its producers are owner-local. Nothing can free
            // a slot between the Full result and registering this wait.
            self.mailbox.space.notified().await;
        }
    }
}

impl Wake for Notify {
    fn wake(&self) -> io::Result<()> {
        self.notify_one();
        Ok(())
    }
}

pub(crate) struct OwnerIo<M> {
    // Drop tasks (and their sockets) before dropping the runtime that polled them.
    tasks: Rc<LocalSet>,
    runtime: Runtime,
    mailbox: Rc<Mailbox<M>>,
    /// Set by `ticker` when a tick of [`Self::run_ready`] left ready tasks behind.
    more: Arc<More>,
    /// The waker of `more`, built once so a tick allocates nothing.
    ticker: Waker,
}

impl<M> OwnerIo<M> {
    pub(crate) fn new(capacity: usize, validator: Option<MessageValidator<M>>) -> io::Result<Self> {
        if Handle::try_current().is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the socket reactor requires a VM owner thread outside Tokio",
            ));
        }
        let capacity_error = |_| io::Error::other("owner I/O message capacity");
        let messages = Arena::new(capacity).map_err(capacity_error)?;
        let mut ready = VecDeque::new();
        ready
            .try_reserve_exact(capacity)
            .map_err(|_| capacity_error(()))?;
        let more = Arc::new(More(AtomicBool::new(false)));
        Ok(Self {
            ticker: Waker::from(more.clone()),
            more,
            tasks: Rc::new(LocalSet::new()),
            runtime: Builder::new_current_thread()
                .enable_all()
                .max_blocking_threads(BLOCKING_THREADS)
                .build()?,
            mailbox: Rc::new(Mailbox {
                inbox: RefCell::new(Some(Inbox { messages, ready })),
                wake: Arc::new(Notify::new()),
                space: Notify::new(),
                validator,
            }),
        })
    }

    pub(crate) fn handle(&self) -> OwnerIoHandle<M> {
        OwnerIoHandle {
            tasks: Rc::downgrade(&self.tasks),
            runtime: self.runtime.handle().clone(),
            mailbox: self.mailbox.clone(),
        }
    }

    pub(crate) fn waker(&self) -> Arc<dyn Wake> {
        self.mailbox.wake.clone()
    }

    /// Turns the reactor once (one kqueue/epoll call), then runs the socket tasks that turn
    /// made ready.
    ///
    /// # Arguments
    /// * `timeout` - `Some(ZERO)` asks the operating system for readiness without waiting,
    ///   `Some(t)` waits at most `t` for a published event, `None` waits until one is
    ///   published.
    pub(crate) fn poll(&self, timeout: Option<Duration>) {
        if timeout == Some(Duration::ZERO) {
            self.tasks.block_on(&self.runtime, tokio::task::yield_now());
        } else {
            // Decision: the wait ticks the task set itself and then checks the doorbell,
            // instead of `LocalSet::block_on` over `notified()`. That one polls the doorbell
            // first and the tasks second, so the task that reads a request publishes it
            // after the check; Tokio then makes a nonblocking reactor turn (12 microseconds
            // on macOS) before it polls the doorbell again. Ticking first ends the wait in
            // the poll that published the event.
            self.runtime.block_on(async {
                let doorbell = self.mailbox.wake.notified();
                let deadline = timeout.map(tokio::time::sleep);
                tokio::pin!(doorbell, deadline);
                std::future::poll_fn(|context| {
                    let tick = std::pin::pin!(self.tasks.run_until(std::future::pending::<()>()));
                    let _ = tick.poll(context);
                    if doorbell.as_mut().poll(context).is_ready() {
                        return Poll::Ready(());
                    }
                    match deadline.as_mut().as_pin_mut() {
                        Some(deadline) => deadline.poll(context),
                        None => Poll::Pending,
                    }
                })
                .await;
            });
        }
        // `block_on` returns as soon as its own future is ready, which leaves the tasks the
        // last reactor turn woke unpolled; they run here instead of one slice later.
        self.run_ready();
    }

    /// Runs the socket tasks that are ready now, without a system call for new readiness.
    ///
    /// Decision: the task set is polled directly instead of through `block_on`. Every
    /// `block_on` that does not finish at once turns the reactor, and a nonblocking
    /// kqueue call measured 12 microseconds on macOS, which was most of a request's time at
    /// one connection. A response a JavaScript turn just queued needs only its task polled.
    ///
    /// Polling also leaves `ticker` registered as the task set's waker. A wake from a
    /// JavaScript turn then sets a flag instead of unparking the reactor (a system call,
    /// and a spurious return of the next wait). No wake is lost: `block_on` registers its
    /// own waker and ticks the task set before it parks.
    pub(crate) fn run_ready(&self) {
        let _runtime = self.runtime.enter();
        let mut context = Context::from_waker(&self.ticker);
        for _ in 0..READY_ROUNDS {
            self.more.0.store(false, Ordering::Relaxed);
            let tick = std::pin::pin!(self.tasks.run_until(std::future::pending::<()>()));
            let _ = tick.poll(&mut context);
            if !self.more.0.load(Ordering::Relaxed) {
                break;
            }
        }
    }

    pub(crate) fn next(&self) -> Option<Key> {
        self.mailbox.inbox.borrow_mut().as_mut()?.ready.pop_front()
    }

    pub(crate) fn take(&self, key: Key) -> Option<M> {
        self.mailbox
            .inbox
            .borrow_mut()
            .as_mut()?
            .messages
            .get_mut(key)?
            .take()
    }

    pub(crate) fn finish(&self, key: Key) {
        let removed = self
            .mailbox
            .inbox
            .borrow_mut()
            .as_mut()
            .and_then(|inbox| inbox.messages.remove(key));
        if removed.is_some() {
            self.mailbox.space.notify_one();
        }
    }

    pub(crate) fn has_ready(&self) -> bool {
        self.mailbox
            .inbox
            .borrow()
            .as_ref()
            .is_some_and(|inbox| !inbox.ready.is_empty())
    }

    pub(crate) fn len(&self) -> usize {
        self.mailbox
            .inbox
            .borrow()
            .as_ref()
            .map_or(0, |inbox| inbox.messages.len())
    }

    pub(crate) fn close(self) {
        let inbox = self.mailbox.inbox.borrow_mut().take();
        self.mailbox.space.notify_waiters();
        drop(self.tasks);
        self.runtime.shutdown_background();
        drop(inbox);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_sender_waits_until_dispatch_finishes() {
        let io = OwnerIo::new(1, None).unwrap();
        let sender = io.handle();
        sender.try_send(1).unwrap();
        let first = io.next().unwrap();
        assert_eq!(io.take(first), Some(1));
        let task = sender
            .clone()
            .spawn(async move { sender.send(2).await.unwrap() })
            .unwrap();
        io.poll(Some(Duration::ZERO));
        assert!(!task.is_finished());
        assert!(!io.has_ready());
        assert_eq!(io.len(), 1);

        io.finish(first);
        io.poll(Some(Duration::ZERO));
        let second = io.next().unwrap();
        assert_eq!(io.take(second), Some(2));
        io.finish(second);
        assert_eq!(io.len(), 0);
        io.close();
    }

    #[test]
    fn nonblocking_slices_drive_real_socket_readiness() {
        use std::io::Write as _;

        let io = OwnerIo::new(1, None).unwrap();
        let sender = io.handle();
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let listener = {
            let _guard = sender.enter();
            tokio::net::TcpListener::from_std(listener).unwrap()
        };
        sender
            .clone()
            .spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                // `readable` + `try_read` instead of `AsyncReadExt::read_u8`: that trait needs
                // Tokio's `io-util`, which only the optional builtins enable, so this test
                // did not compile with `--features quickjs-ng` alone.
                let mut byte = [0u8; 1];
                loop {
                    stream.readable().await.unwrap();
                    match stream.try_read(&mut byte) {
                        Ok(1) => break,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                        other => panic!("read: {other:?}"),
                    }
                }
                sender.send(byte[0]).await.unwrap();
            })
            .unwrap();
        let mut peer = std::net::TcpStream::connect(addr).unwrap();
        peer.write_all(&[42]).unwrap();

        for _ in 0..64 {
            io.poll(Some(Duration::ZERO));
            if let Some(key) = io.next() {
                assert_eq!(io.take(key), Some(42));
                io.finish(key);
                io.close();
                return;
            }
        }
        panic!("an owner that never went idle did not service the socket")
    }
}
