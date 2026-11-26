//! HTTP connections: reading, taking whole requests out of the read buffer
//! (in place when they arrive whole), and building their JS arguments.

use crate::runtime_public_types::ev;
use crate::host_services_trait::Host;
use lunarstorm_http1::{self as http, Body, Step};
use crate::reactor_io_state::{flag, Entry};
use bytes::Buf;
use lunarstorm_engine_contract::{
    Outcome, Scope, Thrown, Vm,
};
use std::io::{self, Read};
use super::*;

/// Outcome of trying to take one request from an HTTP connection.
pub(crate) enum Take {
    /// A request of this many bytes was dispatched and answered; keep going.
    Next(usize),
    /// The window does not hold a complete request.
    NeedMore,
    /// Stop pumping: the connection is gone, closing, or waiting for an async
    /// response. Carries the bytes consumed by a request dispatched just before.
    Stop(usize),
}

impl<'s, V: Vm, H: Host> Turn<'_, 's, V, H> {
    /// Handles a readable event on an HTTP connection.
    ///
    /// While a response is in flight the socket is deliberately left unread: the
    /// kernel buffer fills and TCP flow control slows the client, instead of this
    /// process buffering without bound.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `hup` - whether the kernel reported the peer's close.
    pub(crate) fn http_readable(&mut self, key: usize, hup: bool) {
        {
            let mut io = self.shared.io.borrow_mut();
            let Some(Entry::Conn(conn)) = io.entries.get_mut(key) else { return };
            conn.flags |= if hup { flag::READABLE | flag::RDHUP } else { flag::READABLE };
            if conn.flags & flag::BUSY != 0 {
                return;
            }
        }
        self.http_pump(key);
    }

    /// Drives an HTTP connection: dispatches buffered requests, then reads and
    /// dispatches until the socket is drained or a handler goes asynchronous.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    pub(crate) fn http_pump(&mut self, key: usize) {
        loop {
            // (1) Requests already buffered on the connection.
            loop {
                match self.http_take(key, None) {
                    Take::Next(_) => {}
                    Take::NeedMore => break,
                    Take::Stop(_) => return,
                }
            }

            // (2) Refill from the socket.
            let (n, in_place) = {
                let mut guard = self.shared.io.borrow_mut();
                let io = &mut *guard;
                let Some(Entry::Conn(conn)) = io.entries.get_mut(key) else { return };
                if conn.flags & flag::READABLE == 0 {
                    return;
                }
                match conn.stream.read(self.scratch) {
                    // End of stream with no request in flight: nothing more can come.
                    Ok(0) => {
                        io.close(key);
                        return;
                    }
                    Ok(n) => {
                        if n < self.scratch.len() && conn.flags & flag::RDHUP == 0 {
                            conn.flags &= !flag::READABLE;
                        }
                        let in_place = conn.read_buf.is_empty();
                        if !in_place {
                            conn.read_buf.extend_from_slice(&self.scratch[..n]);
                        }
                        (n, in_place)
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        conn.flags &= !flag::READABLE;
                        return;
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        io.close(key);
                        return;
                    }
                }
            };
            if !in_place {
                continue;
            }

            // (3) Fast path: parse straight out of the scratch buffer. Only bytes of
            // an incomplete request are copied into the connection's buffer.
            let mut at = 0;
            while at < n {
                match self.http_take(key, Some((at, n))) {
                    Take::Next(used) => at += used,
                    Take::NeedMore => {
                        self.stash(key, at, n);
                        break;
                    }
                    Take::Stop(used) => {
                        self.stash(key, at + used, n);
                        return;
                    }
                }
            }
        }
    }

    /// Saves unparsed scratch bytes on the connection.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `from` - start of the unparsed bytes in the scratch buffer.
    /// * `to` - end of the bytes read.
    pub(crate) fn stash(&mut self, key: usize, from: usize, to: usize) {
        if from < to {
            if let Some(Entry::Conn(conn)) = self.shared.io.borrow_mut().entries.get_mut(key) {
                conn.read_buf.extend_from_slice(&self.scratch[from..to]);
            }
        }
    }

    /// Tries to take one request from a window and dispatch it to JS.
    ///
    /// # Arguments
    /// * `key` - slab key of the connection.
    /// * `window` - `Some((start, end))` to parse that range of the scratch buffer,
    ///   `None` to parse the connection's own buffer.
    ///
    /// # Returns
    /// What the pump should do next; see [`Take`].
    pub(crate) fn http_take(&mut self, key: usize, window: Option<(usize, usize)>) -> Take {
        let (owner, used, args) = {
            let mut guard = self.shared.io.borrow_mut();
            let io = &mut *guard;
            let Some(Entry::Conn(conn)) = io.entries.get_mut(key) else { return Take::Stop(0) };
            if conn.flags & (flag::BUSY | flag::ENDING) != 0 {
                return Take::Stop(0);
            }
            let buf: &[u8] = match window {
                Some((start, end)) => &self.scratch[start..end],
                None => &conn.read_buf,
            };
            let step = if buf.is_empty() { Step::NeedMore } else { http::advance(buf, &mut conn.phase) };
            let request = match step {
                Step::Request(request) => request,
                Step::Bad(status) => {
                    io.http_reject(key, status, &self.shared.host);
                    return Take::Stop(0);
                }
                waiting => {
                    if matches!(waiting, Step::Continue) {
                        let _ = conn.send([b"HTTP/1.1 100 Continue\r\n\r\n", &[], &[]]);
                    }
                    // The batch of pipelined requests ended: send what was corked.
                    if conn.flags & flag::CORKED != 0 {
                        io.uncork(key);
                    }
                    return Take::NeedMore;
                }
            };
            let head = &request.head;
            let mut flags = conn.flags & !(flag::KEEP_ALIVE | flag::HEAD_ONLY | flag::V10)
                | flag::BUSY
                | flag::IN_PUMP;
            if head.keep_alive {
                flags |= flag::KEEP_ALIVE;
            }
            if head.http10 {
                flags |= flag::V10;
            }
            // More requests follow in this window: batch the responses.
            if request.consumed < buf.len() {
                flags |= flag::CORKED;
            }
            let method = &buf[head.method.0 as usize..head.method.1 as usize];
            if method == b"HEAD" {
                flags |= flag::HEAD_ONLY;
            }
            conn.flags = flags;
            let used = request.consumed;
            let args = request_args::<V>(&self.scope, buf, request);
            if window.is_none() {
                conn.read_buf.advance(used);
            }
            (conn.owner, used, args)
        };
        let [method, url, fields, body] = match args {
            Ok(args) => args,
            Err(Thrown) => self.fatal(),
        };
        self.fire(&[self.int(ev::HTTP), self.int(owner as i32), self.int(key as i32), method, url, fields, body]);

        let mut guard = self.shared.io.borrow_mut();
        let io = &mut *guard;
        let Some(Entry::Conn(conn)) = io.entries.get_mut(key) else { return Take::Stop(used) };
        conn.flags &= !flag::IN_PUMP;
        if conn.flags & (flag::BUSY | flag::ENDING) != 0 {
            // The handler went asynchronous or the connection is closing: nothing
            // more will be batched, so release earlier responses now.
            io.uncork(key);
            return Take::Stop(used);
        }
        Take::Next(used)
    }
}

/// Builds the JS arguments for a request. Only allocates JS values; no JS code runs,
/// which is why it may be called while the loop state is borrowed.
///
/// # Arguments
/// * `scope` - the context handle.
/// * `buf` - the window the request was parsed from.
/// * `request` - the parsed request.
///
/// # Returns
/// The method (an index into [`http::METHODS`], or the token for other methods), the
/// URL, the raw header-field block (parsed lazily in JS) and the body (`undefined`
/// when absent).
///
/// # Errors
/// [`Thrown`] when allocation fails.
pub(crate) fn request_args<'s, V: Vm>(
    scope: &V::Scope<'s>,
    buf: &[u8],
    request: http::Request,
) -> Outcome<[V::Value<'s>; 4]> {
    let head = &request.head;
    let text = |span: (u32, u32)| String::from_utf8_lossy(&buf[span.0 as usize..span.1 as usize]);
    let method = text(head.method);
    let method = match http::METHODS.iter().position(|known| *known == method) {
        Some(index) => scope.int(index as i32),
        None => scope.string(&method)?,
    };
    let url = scope.string(&text(head.path))?;
    let fields = scope.string(&text(head.fields))?;
    let body = match request.body {
        Body::None => scope.undefined(),
        Body::Span(start, end) => scope.bytes_copy(&buf[start..end])?,
        Body::Owned(bytes) => scope.bytes(bytes)?,
    };
    Ok([method, url, fields, body])
}
