//! Socket readiness: dispatching a kernel event, accepting connections, reading
//! TCP sockets and flushing their write buffers.

use crate::runtime_public_types::ev;
use crate::host_services_trait::{Host, Log};
use crate::reactor_io_state::{flag, Entry, Fault, Wake, WAKER};
use lunarstorm_engine_contract::{
    Scope, Thrown, Vm,
};
use mio::event::Event;
use std::io::{self, Read};
use super::*;

impl<'s, V: Vm, H: Host> Turn<'_, 's, V, H> {
    /// Handles one readiness event.
    ///
    /// A key can be reused within one batch of events, so an event may arrive for a
    /// newer handle than the one it was raised for. That is harmless by construction:
    /// a spurious read returns `WouldBlock` and a spurious flush finds nothing queued.
    ///
    /// # Arguments
    /// * `event` - the event from mio.
    pub(crate) fn on_event(&mut self, event: &Event) {
        let token = event.token();
        if token == WAKER {
            // Posted tasks share this waker; the step drains them after the batch.
            self.drain_pool();
            return;
        }
        let key = token.0;
        let (listener, http) = match self.shared.io.borrow().entries.get(key) {
            Some(Entry::Listener { http, .. }) => (true, *http),
            Some(Entry::Conn(conn)) => (false, conn.flags & flag::HTTP != 0),
            _ => return,
        };
        if listener {
            self.accept(key, http);
        } else {
            if event.is_writable() {
                self.on_writable(key);
            }
            let hup = event.is_read_closed() || event.is_error();
            if event.is_readable() || hup {
                if http {
                    self.http_readable(key, hup);
                } else {
                    self.tcp_read(key, hup);
                }
            }
        }
        self.drain_deferred();
    }

    /// Accepts every pending connection on a listener.
    ///
    /// # Arguments
    /// * `key` - slab key of the listener.
    /// * `http` - whether its connections speak HTTP.
    pub(crate) fn accept(&mut self, key: usize, http: bool) {
        loop {
            let accepted = {
                let mut io = self.shared.io.borrow_mut();
                let Some(Entry::Listener { sock, .. }) = io.entries.get(key) else { return };
                match sock.accept() {
                    Ok((stream, peer)) => io.adopt(stream, key, http).map(|conn| (conn, peer)),
                    Err(e) => Err(e),
                }
            };
            match accepted {
                // HTTP connections have no JS object until a request arrives.
                Ok(_) if http => {}
                Ok((conn, peer)) => self.fire(&[
                    self.int(ev::CONNECTION),
                    self.int(key as i32),
                    self.int(conn as i32),
                    self.text(&peer.ip().to_string()),
                    self.int(i32::from(peer.port())),
                ]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                // Typically EMFILE or ECONNABORTED: drop this connection, keep serving.
                Err(e) => {
                    self.shared.host.log(Log::Warning(&format!("accept failed: {e}")));
                    return;
                }
            }
        }
    }

    /// Handles a writable event.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    pub(crate) fn on_writable(&mut self, key: usize) {
        let wake = self.shared.io.borrow_mut().writable(key);
        match wake {
            Wake::Quiet => {}
            Wake::Connected => self.fire(&[self.int(ev::CONNECT), self.int(key as i32)]),
            Wake::Failed(fault) => self.fire_close(key, Some(fault)),
        }
    }

    /// Reads a TCP socket and delivers the bytes to JS.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `hup` - whether the kernel reported the peer's close; when set, the socket is
    ///   read to end of stream instead of stopping at a short read.
    pub(crate) fn tcp_read(&mut self, key: usize, hup: bool) {
        loop {
            let read = {
                let mut io = self.shared.io.borrow_mut();
                let Some(Entry::Conn(conn)) = io.entries.get_mut(key) else { return };
                // After end of stream there is nothing to read; before the connect
                // completes, the writable handler reports success or failure.
                if conn.flags & (flag::READ_EOF | flag::CONNECTING) != 0 {
                    return;
                }
                conn.stream.read(self.scratch)
            };
            match read {
                Ok(0) => {
                    self.shared.io.borrow_mut().read_eof(key);
                    self.fire(&[self.int(ev::END), self.int(key as i32)]);
                    self.shared.io.borrow_mut().finish_read(key);
                    return;
                }
                Ok(n) => {
                    let chunk = match self.scope.bytes_copy(&self.scratch[..n]) {
                        Ok(chunk) => chunk,
                        Err(Thrown) => self.fatal(),
                    };
                    self.fire(&[self.int(ev::DATA), self.int(key as i32), chunk]);
                    if n < self.scratch.len() && !hup {
                        return;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    self.shared.io.borrow_mut().close(key);
                    self.fire_close(key, Some(Fault::new(&e, "read", "")));
                    return;
                }
            }
        }
    }
}
