//! `lunarstorm:ipc`: the program's byte channel to the application that embeds it.
//!
//! ```js
//! import { send, onMessage } from 'lunarstorm:ipc';
//! onMessage((bytes) => send(bytes)); // bytes: Uint8Array
//! onMessage(null);                   // releases the channel
//! ```
//!
//! - `send(bytes)` hands the bytes to [`Host::ipc_send`] on the loop thread, before
//!   it returns.
//! - `onMessage(fn | null)` sets the receiver of what the host posts through an
//!   [`Inbox`], one `Uint8Array` per message, in order. While a receiver is set it
//!   keeps the loop alive; `null`, or the host's [`Inbox::close`], releases it.
//! - A message posted while no receiver is set waits for the next one and is
//!   delivered in a later loop turn, never from inside `onMessage` itself.
//!
//! Decisions:
//! - This is a native module (`Scope::native_module`), like every builtin: no hidden
//!   global carries it, nothing of it is reachable except through `import`, and the
//!   JS glue knows nothing about it.
//! - The receiver is a persistent handle in [`Ipc`], and the loop calls it directly;
//!   there is no event code in the glue's `dispatch`.
//! - Only the host type decides whether the module exists ([`Host::IPC`]).

use crate::channel::{Channel, Inbox, Nudge};
use crate::host::Host;
use crate::io::Shared;
use lunarstorm_engine_contract::{ErrorKind, Kind, Outcome, Scope, Vm};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// The specifier scripts import.
pub const MODULE: &str = "lunarstorm:ipc";

/// The per-Runtime state of the channel: the queue the host fills and the script's
/// receiver.
pub struct Ipc<V: Vm> {
    /// What the host has posted and the program has not received.
    pub channel: Arc<Channel>,
    /// Wakes the loop when the receiver changes or a message arrives.
    nudge: Nudge,
    /// The script's receiver, if one is set.
    receiver: RefCell<Option<V::Persistent>>,
}

impl<V: Vm> Ipc<V> {
    /// # Arguments
    /// * `nudge` - wakes the Runtime's loop.
    ///
    /// # Returns
    /// The state, with an open channel and no receiver.
    pub fn new(nudge: Nudge) -> Rc<Self> {
        Rc::new(Self { channel: Arc::new(Channel::default()), nudge, receiver: RefCell::new(None) })
    }

    /// # Returns
    /// The host's side of the channel.
    pub fn inbox(&self) -> Inbox {
        Inbox::new(self.channel.clone(), self.nudge.clone())
    }

    /// # Returns
    /// A handle to the receiver, as a scoped value, or `None` when none is set.
    pub fn receiver<'s>(&self, scope: &V::Scope<'s>) -> Option<V::Value<'s>> {
        self.receiver.borrow().as_ref().map(|handle| scope.local(handle))
    }

    /// Drops the receiver and gives back the loop reference it held.
    ///
    /// Must run before the VM instance is destroyed: the receiver is a persistent
    /// handle into its heap.
    ///
    /// # Arguments
    /// * `refs` - the loop's reference count (`Shared::refs`).
    pub fn clear(&self, refs: &Cell<u32>) {
        let old = self.receiver.borrow_mut().take();
        if old.is_some() {
            refs.set(refs.get().saturating_sub(1));
        }
    }

    /// # Returns
    /// A handle that wakes this Runtime's loop.
    pub fn nudge_handle(&self) -> Nudge {
        self.nudge.clone()
    }

    /// Builds `onMessage`: sets or clears the receiver.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `shared` - loop state; the receiver holds one of its loop references.
    ///
    /// # Returns
    /// The function.
    ///
    /// # Errors
    /// [`lunarstorm_engine_contract::Thrown`] when the function cannot be created.
    pub fn on_message<'s, H: Host>(self: &Rc<Self>, scope: &V::Scope<'s>, shared: &Rc<Shared<H>>) -> Outcome<V::Value<'s>> {
        let (ipc, shared) = (self.clone(), shared.clone());
        scope.function("onMessage", move |call| {
            let scope = call.scope();
            match call.args().first() {
                Some(function) if scope.kind(function) == Kind::Function => {
                    let handle = scope.persist(function);
                    if ipc.receiver.borrow_mut().replace(handle).is_none() {
                        shared.refs.set(shared.refs.get() + 1);
                    }
                    // Messages that arrived before this receiver are delivered in a
                    // later turn, never from inside `onMessage`.
                    ipc.nudge.nudge();
                }
                None => ipc.clear(&shared.refs),
                Some(value) if matches!(scope.kind(value), Kind::Null | Kind::Undefined) => ipc.clear(&shared.refs),
                Some(_) => return Err(scope.throw_error(ErrorKind::Type, "onMessage expects a function or null")),
            }
            Ok(scope.undefined())
        })
    }

    /// Builds the module's exports.
    ///
    /// # Arguments
    /// * `scope` - the context handle the module is being loaded in.
    /// * `shared` - loop state and host.
    ///
    /// # Returns
    /// `(name, function)` for `send` and `onMessage`.
    ///
    /// # Errors
    /// [`lunarstorm_engine_contract::Thrown`] when a function cannot be created.
    pub fn exports<'s, H: Host>(self: &Rc<Self>, scope: &V::Scope<'s>, shared: &Rc<Shared<H>>) -> Outcome<Vec<(&'static str, V::Value<'s>)>> {
        let host = shared.clone();
        let send = scope.function("send", move |call| {
            let scope = call.scope();
            match call.args().first() {
                Some(bytes) if scope.kind(bytes) == Kind::Bytes => {
                    scope.with_bytes(bytes, |bytes| host.host.ipc_send(bytes))?;
                    Ok(scope.undefined())
                }
                _ => Err(scope.throw_error(ErrorKind::Type, "send expects a Uint8Array")),
            }
        })?;
        let on_message = self.on_message(scope, shared)?;
        Ok(vec![("send", send), ("onMessage", on_message)])
    }
}
