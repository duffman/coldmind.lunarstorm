//! `lunarstorm:ipc`: the byte channel between a program and the application that
//! embeds it, driven the way an embedding host drives it (an `Inbox` from any
//! thread, `Host::ipc_send` for what the program sends). Ported from jetbunny's
//! `jetbunny:ipc` tests, with the same bounds.

use lunarstorm_event_loop_runtime::{Access, Bindings, Host, Log, Runtime, Source, Stream, MAX_MESSAGES, MAX_MESSAGE_BYTES};
use lunarstorm_engine_quickjs_adapter::QuickJs;
use std::cell::RefCell;
use std::thread;
use std::time::Duration;

/// A host with a channel when `IPC` is true: output and sent messages are captured.
#[derive(Default)]
struct TestHost<const IPC: bool> {
    /// Everything written to stdout.
    out: RefCell<Vec<u8>>,
    /// Every message the program sent, in order.
    sent: RefCell<Vec<Vec<u8>>>,
    /// Uncaught-error records; the host survives them.
    errors: RefCell<Vec<String>>,
}

impl<const IPC: bool> Host for TestHost<IPC> {
    /// No extensions.
    type Extensions<V: lunarstorm_event_loop_runtime::vm::Vm> = ();

    /// # Returns
    /// The empty set.
    fn extensions<V: lunarstorm_event_loop_runtime::vm::Vm>(&self) -> Self::Extensions<V> {}

    const IPC: bool = IPC;

    /// # Returns
    /// A fixed `process.argv`.
    fn args(&self) -> Vec<String> {
        vec![String::from("ipc"), String::from("main")]
    }

    /// # Returns
    /// No variables.
    fn environment(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// # Returns
    /// A directory that exists everywhere.
    fn working_directory(&self) -> String {
        String::from("/")
    }

    /// Captures stdout; stderr is dropped.
    ///
    /// # Arguments
    /// * `stream` - which stream.
    /// * `bytes` - what was written.
    fn write(&self, stream: Stream, bytes: &[u8]) {
        if stream == Stream::Out {
            self.out.borrow_mut().extend_from_slice(bytes);
        }
    }

    /// Fails the test: nothing here is expected to exit.
    ///
    /// # Arguments
    /// * `code` - the exit status the Runtime asked for.
    fn exit(&self, code: i32) -> ! {
        panic!("the runtime asked the host to exit with {code}; errors: {:?}", self.errors.borrow());
    }

    /// # Returns
    /// `true`: these programs touch nothing.
    fn allow(&self, _access: Access<'_>) -> bool {
        true
    }

    /// Fills the buffer with a known byte.
    ///
    /// # Arguments
    /// * `into` - the buffer to fill.
    fn entropy(&self, into: &mut [u8]) {
        into.fill(7);
    }

    /// Ignores the record.
    ///
    /// # Arguments
    /// * `_record` - the record.
    fn log(&self, _record: Log<'_>) {}

    /// Records the error and survives it.
    ///
    /// # Arguments
    /// * `_in_promise` - whether it is a rejected promise.
    /// * `description` - the error as `console` would show it.
    fn uncaught(&self, _in_promise: bool, description: &str) {
        self.errors.borrow_mut().push(description.to_owned());
    }

    /// Records what the program sent.
    ///
    /// # Arguments
    /// * `bytes` - the message.
    fn ipc_send(&self, bytes: &[u8]) {
        self.sent.borrow_mut().push(bytes.to_vec());
    }
}

/// Starts a Runtime and loads `code` as its entry module.
///
/// # Arguments
/// * `code` - the entry module's source.
///
/// # Returns
/// The Runtime, or the load error.
fn boot<const IPC: bool>(code: &str) -> Result<Runtime<QuickJs, TestHost<IPC>>, String> {
    let mut runtime = Runtime::<QuickJs, TestHost<IPC>>::new(TestHost::default()).expect("event loop");
    runtime.register(Bindings::new());
    runtime.start().expect("start");
    runtime.load(Source::Text { name: String::from("main.js"), code: code.to_owned() })?;
    Ok(runtime)
}

/// # Arguments
/// * `runtime` - a Runtime.
///
/// # Returns
/// What its program wrote to stdout so far.
fn stdout<const IPC: bool>(runtime: &Runtime<QuickJs, TestHost<IPC>>) -> String {
    String::from_utf8(runtime.host().out.borrow().clone()).expect("stdout is UTF-8")
}

const ECHO: &str = "import { send, onMessage } from 'lunarstorm:ipc'; onMessage((bytes) => send(bytes));";

#[test]
fn the_host_and_a_program_talk_over_ipc_until_the_channel_closes() {
    let mut runtime = boot::<true>(ECHO).expect("load");
    let inbox = runtime.inbox();
    assert!(inbox.post(b"hello".to_vec()));
    assert!(inbox.post(b"world".to_vec()));
    inbox.close();
    assert_eq!(runtime.run(), 0, "the loop ends once the channel is closed and drained");
    assert_eq!(*runtime.host().sent.borrow(), vec![b"hello".to_vec(), b"world".to_vec()]);
    assert!(runtime.host().errors.borrow().is_empty(), "{:?}", runtime.host().errors.borrow());
}

#[test]
fn a_message_from_another_thread_wakes_a_sleeping_loop() {
    let mut runtime = boot::<true>(ECHO).expect("load");
    let inbox = runtime.inbox();
    let poster = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        assert!(inbox.post(b"late".to_vec()));
        thread::sleep(Duration::from_millis(20));
        inbox.close();
    });
    assert_eq!(runtime.run(), 0);
    poster.join().expect("poster thread");
    assert_eq!(*runtime.host().sent.borrow(), vec![b"late".to_vec()]);
}

#[test]
fn messages_posted_before_the_receiver_wait_and_arrive_in_order_in_a_later_turn() {
    let mut runtime = boot::<true>(
        "import { onMessage } from 'lunarstorm:ipc';
         console.log('before');
         onMessage((bytes) => console.log('got ' + bytes[0]));
         console.log('after');",
    )
    .expect("load");
    let inbox = runtime.inbox();
    for n in 1..=3u8 {
        assert!(inbox.post(vec![n]));
    }
    inbox.close();
    assert_eq!(runtime.run(), 0);
    assert_eq!(stdout(&runtime), "before\nafter\ngot 1\ngot 2\ngot 3\n");
}

#[test]
fn a_message_posted_before_the_program_ran_at_all_is_not_lost() {
    // The Runtime is built and the message posted before the program loads, as a host
    // does when it hands over a first message at start-up.
    let mut runtime = Runtime::<QuickJs, TestHost<true>>::new(TestHost::default()).expect("event loop");
    runtime.start().expect("start");
    let inbox = runtime.inbox();
    assert!(inbox.post(b"first".to_vec()));
    runtime.load(Source::Text { name: String::from("main.js"), code: ECHO.to_owned() }).expect("load");
    inbox.close();
    assert_eq!(runtime.run(), 0);
    assert_eq!(*runtime.host().sent.borrow(), vec![b"first".to_vec()]);
}

#[test]
fn post_is_refused_past_the_bounds_and_nothing_is_queued_for_it() {
    let mut runtime = boot::<true>(
        "import { onMessage } from 'lunarstorm:ipc';
         let count = 0;
         onMessage((bytes) => { count += 1; if (bytes.length === 0) console.log('count ' + count); });",
    )
    .expect("load");
    let inbox = runtime.inbox();
    for n in 0..MAX_MESSAGES {
        assert!(inbox.post(vec![1]), "message {n} is within the bound");
    }
    assert!(!inbox.post(vec![1]), "message {} passes the 4096 bound", MAX_MESSAGES + 1);
    assert!(!inbox.post(vec![0; MAX_MESSAGE_BYTES + 1]), "one message over 8 MiB is refused");
    inbox.close();
    assert!(!inbox.post(vec![1]), "a closed channel refuses");
    assert_eq!(runtime.run(), 0);
    assert!(runtime.host().errors.borrow().is_empty(), "{:?}", runtime.host().errors.borrow());
}

#[test]
fn on_message_null_releases_the_channel_and_the_program_ends() {
    let mut runtime = boot::<true>(
        "import { onMessage } from 'lunarstorm:ipc';
         onMessage((bytes) => { console.log('got ' + bytes[0]); onMessage(null); });",
    )
    .expect("load");
    let inbox = runtime.inbox();
    assert!(inbox.post(vec![9]));
    assert_eq!(runtime.run(), 0, "no close from the host: the program released the channel itself");
    assert_eq!(stdout(&runtime), "got 9\n");
}

#[test]
fn a_program_with_a_receiver_stays_alive_until_the_host_closes_the_channel() {
    let mut runtime = boot::<true>(ECHO).expect("load");
    let inbox = runtime.inbox();
    let closer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(80));
        inbox.close();
    });
    let started = std::time::Instant::now();
    assert_eq!(runtime.run(), 0);
    closer.join().expect("closer thread");
    assert!(started.elapsed() >= Duration::from_millis(60), "the loop must not end before the host closes: {:?}", started.elapsed());
}

#[test]
fn the_module_does_not_resolve_under_a_host_without_a_channel() {
    let failure = boot::<false>("import { send } from 'lunarstorm:ipc'; send(new Uint8Array(1));");
    assert!(failure.is_err(), "a host with IPC = false must not serve lunarstorm:ipc");
}

#[test]
fn send_and_on_message_check_their_arguments() {
    let mut runtime = boot::<true>(
        "import { send, onMessage } from 'lunarstorm:ipc';
         for (const bad of ['text', 5, null, {}, undefined]) {
           try { send(bad); console.log('no throw'); } catch (e) { console.log(e.name); }
         }
         try { onMessage(5); console.log('no throw'); } catch (e) { console.log(e.name); }
         try { onMessage('x'); console.log('no throw'); } catch (e) { console.log(e.name); }
         onMessage(null);",
    )
    .expect("load");
    assert_eq!(runtime.run(), 0);
    assert_eq!(stdout(&runtime), "TypeError\nTypeError\nTypeError\nTypeError\nTypeError\nTypeError\nTypeError\n");
    assert!(runtime.host().sent.borrow().is_empty());
}

#[test]
fn a_receiver_that_throws_is_reported_and_later_messages_still_arrive() {
    let mut runtime = boot::<true>(
        "import { onMessage } from 'lunarstorm:ipc';
         onMessage((bytes) => { if (bytes[0] === 1) throw new Error('boom'); console.log('ok ' + bytes[0]); });",
    )
    .expect("load");
    let inbox = runtime.inbox();
    assert!(inbox.post(vec![1]));
    assert!(inbox.post(vec![2]));
    inbox.close();
    assert_eq!(runtime.run(), 0);
    assert_eq!(stdout(&runtime), "ok 2\n");
    assert_eq!(runtime.host().errors.borrow().len(), 1);
    assert!(runtime.host().errors.borrow()[0].contains("boom"));
}

#[test]
fn post_after_the_runtime_is_gone_returns_false() {
    let runtime = boot::<true>(ECHO).expect("load");
    let inbox = runtime.inbox();
    assert!(inbox.post(vec![1]), "accepted while the Runtime exists");
    drop(runtime);
    assert!(!inbox.post(vec![2]), "nothing will ever receive this message");
}
