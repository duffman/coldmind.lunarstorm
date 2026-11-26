// lunarstorm JS glue: the Node-compatible surface, built on the native `ops` object.
//
// This file is one function expression. The Runtime evaluates it once at startup,
// calls it with `ops` and `native` (the builtins already ported to Rust, see
// `src/node/mod.rs`: `native.modules` plus the helpers this file uses), and keeps
// what it returns: the hooks ({ dispatch, ticks, describe, exitCode, invoke }) and
// the module registry ({ modules, internal, exportsOf }). Every event the loop
// delivers enters through `dispatch`.
//
// `ops` holds the Runtime's own natives plus whatever the host registered.
//
// Conventions:
// - Handles (sockets, servers) are identified by the reactor's slab key and looked up
//   in the dense `handles` array.
// - Hot paths (data, request) avoid rest arrays and closures.
// - Builtin modules are plain objects in the returned `modules` registry; the module
//   loader declares each as a native module (`default` is the object, every key a
//   named export). Nothing is published on the global object.
(function (ops, native) {
  'use strict';

  // ------------------------------------------------------------------ events

  /** The native EventEmitter (`src/node/events.rs`) and its hot-path emit. */
  const EventEmitter = native.modules.events;
  const emit2 = native.emit2;

  // ------------------------------------------------------------------ timers

  // The timers, immediates and the nextTick queue are native (`src/node/timers.rs`);
  // the globals are installed there.
  const { nextTick, ticks, runImmediate, fireTimer } = native;

  // ------------------------------------------------------------------ buffer

  // Buffer, TextEncoder and TextDecoder are native (`src/node/buffer.rs`).
  const { asBuffer, utf8Only } = native;

  // ----------------------------------------------------------------- console

  // `console` and the inspector are native (`src/node/console.rs`). `inspect(value)`
  // renders a value uncolored; `live` is the slot `lunarstorm:ui` writes `wrap` to.
  const { inspect, live } = native;

  // ----------------------------------------------------------------- process

  // `process`, `global`, `crypto` and `performance` are native (`src/node/process.rs`).
  const process = native.modules.process;

  // --------------------------------------------------------------------- net

  /** @type {(Socket|Server|undefined)[]} live handles, by reactor slab key */
  const handles = [];

  /**
   * Builds an Error carrying a Node-style `code`.
   * @param {string} message message
   * @param {string} [code] symbolic code such as ECONNRESET
   * @return {Error}
   */
  function codedError(message, code) {
    const err = new Error(message);
    if (code !== undefined) err.code = code;
    return err;
  }

  /** A TCP socket. Emits connect, data, end, drain, error and close. */
  class Socket extends EventEmitter {
    /**
     * @param {{allowHalfOpen?: boolean}} [options] socket options
     */
    constructor(options) {
      super();
      /** Reactor slab key, or -1 when not attached. */
      this._k = -1;
      this.connecting = false;
      this.destroyed = false;
      this.allowHalfOpen = !!(options && options.allowHalfOpen);
      this.remoteAddress = undefined;
      this.remotePort = undefined;
      /** Writes and the end request issued before the connect completed. */
      this._queue = undefined;
      this._ended = false;
    }

    /**
     * Starts an outbound connection.
     * @param {number|{port: number, host?: string}} port port or options
     * @param {string|Function} [host] host name; defaults to 127.0.0.1
     * @param {Function} [cb] called on connect
     * @return {this}
     */
    connect(port, host, cb) {
      if (typeof port === 'object') {
        cb = host;
        host = port.host;
        port = port.port;
      }
      if (typeof host === 'function') {
        cb = host;
        host = undefined;
      }
      this.remoteAddress = host || '127.0.0.1';
      this.remotePort = port | 0;
      this.connecting = true;
      this._k = ops.connect(this.remoteAddress, this.remotePort);
      handles[this._k] = this;
      if (cb) this.once('connect', cb);
      return this;
    }

    /**
     * Sends data.
     * @param {string|Uint8Array} data bytes or UTF-8 text
     * @param {string|Function} [enc] encoding (UTF-8 only) or the callback
     * @param {Function} [cb] called once the data is handed to the kernel
     * @return {boolean} false when the caller should wait for `drain`
     */
    write(data, enc, cb) {
      if (typeof enc === 'function') cb = enc;
      else utf8Only(enc);
      if (this.destroyed || this._ended) {
        nextTick(() => this.emit('error', codedError('write after end', 'ERR_STREAM_WRITE_AFTER_END')));
        return false;
      }
      if (this.connecting) {
        (this._queue || (this._queue = [])).push([data, cb]);
        return true;
      }
      const ok = ops.write(this._k, data);
      if (cb) {
        if (ok) nextTick(cb);
        else this.once('drain', cb);
      }
      return ok;
    }

    /**
     * Sends optional final data, then closes the sending side.
     * @param {string|Uint8Array} [data] final bytes
     * @return {this}
     */
    end(data) {
      if (this._ended || this.destroyed) return this;
      if (data !== undefined) this.write(data);
      this._ended = true;
      if (!this.connecting) ops.end(this._k);
      return this;
    }

    /**
     * Closes the socket at once, discarding unsent data.
     * @param {Error} [err] emitted as `error` before `close`
     * @return {this}
     */
    destroy(err) {
      if (this.destroyed) return this;
      this.destroyed = true;
      this.connecting = false;
      if (this._k >= 0) {
        ops.destroy(this._k);
        handles[this._k] = undefined;
        this._k = -1;
      }
      nextTick(() => {
        if (err) this.emit('error', err);
        this.emit('close', !!err);
      });
      return this;
    }
  }

  /** A TCP server. Emits listening, connection, error and close. */
  class Server extends EventEmitter {
    /**
     * @param {object|Function} [options] options, or the connection listener
     * @param {Function} [onConnection] connection listener
     */
    constructor(options, onConnection) {
      super();
      if (typeof options === 'function') {
        onConnection = options;
        options = undefined;
      }
      this._k = -1;
      /** Whether accepted connections are parsed as HTTP by the reactor. */
      this._http = false;
      this._host = undefined;
      this._port = 0;
      this.allowHalfOpen = !!(options && options.allowHalfOpen);
      this.listening = false;
      if (onConnection) this.on('connection', onConnection);
    }

    /**
     * Binds and starts accepting. Bind errors are emitted as `error`.
     * @param {number|{port?: number, host?: string}} [port] port (0 picks a free one) or options
     * @param {string|Function} [host] address; defaults to 0.0.0.0
     * @param {Function} [cb] called once listening
     * @return {this}
     */
    listen(port, host, cb) {
      if (port !== null && typeof port === 'object') {
        cb = typeof host === 'function' ? host : cb;
        host = port.host;
        port = port.port;
      }
      if (typeof port === 'function') {
        cb = port;
        port = 0;
      }
      if (typeof host === 'function') {
        cb = host;
        host = undefined;
      }
      port = port === undefined ? 0 : Number(port);
      if (!Number.isInteger(port) || port < 0 || port > 65535) throw new RangeError('Port should be >= 0 and < 65536');
      if (this.listening) throw codedError('Listen method has been called more than once', 'ERR_SERVER_ALREADY_LISTEN');
      if (cb) this.once('listening', cb);
      let bound;
      try {
        bound = ops.listen(host || '0.0.0.0', port, this._http);
      } catch (err) {
        nextTick(() => this.emit('error', err));
        return this;
      }
      this._k = bound[0];
      this._port = bound[1];
      this._host = host || '0.0.0.0';
      this.listening = true;
      handles[this._k] = this;
      nextTick(() => this.emit('listening'));
      return this;
    }

    /**
     * @return {{address: string, family: string, port: number}|null} the bound address
     */
    address() {
      if (!this.listening) return null;
      return { address: this._host, family: this._host.includes(':') ? 'IPv6' : 'IPv4', port: this._port };
    }

    /**
     * Stops accepting. Open TCP connections stay open; idle HTTP connections close.
     * @param {Function} [cb] called once closed
     * @return {this}
     */
    close(cb) {
      if (!this.listening) {
        if (cb) nextTick(cb, codedError('Server is not running', 'ERR_SERVER_NOT_RUNNING'));
        return this;
      }
      ops.listenerClose(this._k);
      handles[this._k] = undefined;
      this._k = -1;
      this.listening = false;
      nextTick(() => {
        this.emit('close');
        if (cb) cb();
      });
      return this;
    }
  }

  /**
   * Creates a TCP server.
   * @param {object|Function} [options] options, or the connection listener
   * @param {Function} [onConnection] connection listener
   * @return {Server}
   */
  const createServer = (options, onConnection) => new Server(options, onConnection);

  /**
   * Opens a TCP connection.
   * @param {number|object} port port or options
   * @param {string|Function} [host] host name
   * @param {Function} [cb] called on connect
   * @return {Socket}
   */
  const connect = (port, host, cb) => new Socket(typeof port === 'object' ? port : undefined).connect(port, host, cb);

  const net = { Server, Socket, createServer, connect, createConnection: connect };

  // -------------------------------------------------------------------- http

  // Index-aligned with METHODS in src/http.rs: the reactor sends these as integers.
  const METHODS = ['GET', 'HEAD', 'POST', 'PUT', 'DELETE', 'PATCH', 'OPTIONS'];

  /**
   * Parses a raw header-field block into Node's `headers` object: lowercased names,
   * repeated fields joined with ', ' (cookies with '; ', set-cookie kept as an array).
   * @param {string} raw `name: value` lines
   * @return {Object<string, string|string[]>}
   */
  function parseHeaders(raw) {
    const out = {};
    let at = 0;
    while (at < raw.length) {
      let end = raw.indexOf('\n', at);
      if (end < 0) end = raw.length;
      const colon = raw.indexOf(':', at);
      if (colon > at && colon < end) {
        const name = raw.slice(at, colon).toLowerCase();
        const value = raw.slice(colon + 1, end).trim();
        const prev = out[name];
        if (prev === undefined) out[name] = name === 'set-cookie' ? [value] : value;
        else if (name === 'set-cookie') prev.push(value);
        else out[name] = prev + (name === 'cookie' ? '; ' : ', ') + value;
      }
      at = end + 1;
    }
    return out;
  }

  /**
   * An HTTP request. The reactor delivers it complete: the body is already buffered,
   * and `data`/`end` are emitted once a listener for either is attached.
   */
  class IncomingMessage extends EventEmitter {
    /**
     * @param {string} method request method
     * @param {string} url request target
     * @param {string} raw raw header-field block
     * @param {Uint8Array} [body] request body
     */
    constructor(method, url, raw, body) {
      super();
      this.method = method;
      this.url = url;
      this.httpVersion = '1.1';
      this.complete = true;
      this._raw = raw;
      this._body = body;
      this._headers = undefined;
      this._flowing = false;
    }

    /**
     * Header fields, parsed on first access so a handler that never reads them pays nothing.
     * @return {Object<string, string|string[]>}
     */
    get headers() {
      return this._headers || (this._headers = parseHeaders(this._raw));
    }

    /**
     * Adds a listener; the first `data` or `end` listener starts body delivery.
     * @param {string} type event name
     * @param {Function} fn listener
     * @return {this}
     */
    on(type, fn) {
      super.on(type, fn);
      if (!this._flowing && (type === 'data' || type === 'end')) {
        this._flowing = true;
        queueMicrotask(() => {
          if (this._body !== undefined) emit2(this, 'data', asBuffer(this._body));
          emit2(this, 'end');
        });
      }
      return this;
    }

    /**
     * Iterates the body: `for await (const chunk of req)`.
     * @return {AsyncGenerator<Buffer>}
     */
    async *[Symbol.asyncIterator]() {
      this._flowing = true;
      if (this._body !== undefined) yield asBuffer(this._body);
    }
  }

  /** An HTTP response. `end(body)` alone sends head and body in one write. */
  class ServerResponse extends EventEmitter {
    /**
     * @param {number} key reactor slab key of the connection
     */
    constructor(key) {
      super();
      this._k = key;
      /** @type {Object<string, *>|undefined} header fields by lowercased name */
      this._fields = undefined;
      this.statusCode = 200;
      this.headersSent = false;
      this.finished = false;
    }

    /**
     * Sets a header field. Values with line breaks are rejected (response splitting).
     * @param {string} name field name
     * @param {string|number|string[]} value field value
     * @return {this}
     */
    setHeader(name, value) {
      if (this.headersSent) throw codedError('Cannot set headers after they are sent to the client', 'ERR_HTTP_HEADERS_SENT');
      if (/[\r\n]/.test(name) || /[\r\n]/.test(Array.isArray(value) ? value.join('') : String(value))) {
        throw new TypeError('Invalid character in header "' + name + '"');
      }
      (this._fields || (this._fields = {}))[name.toLowerCase()] = value;
      return this;
    }

    /**
     * @param {string} name field name
     * @return {*} the field value, or undefined
     */
    getHeader(name) {
      return this._fields && this._fields[name.toLowerCase()];
    }

    /**
     * @param {string} name field name
     * @return {boolean} whether the field is set
     */
    hasHeader(name) {
      return this.getHeader(name) !== undefined;
    }

    /**
     * Removes a header field.
     * @param {string} name field name
     */
    removeHeader(name) {
      if (this._fields) delete this._fields[name.toLowerCase()];
    }

    /**
     * Sets the status and header fields. Nothing is sent until write() or end().
     * @param {number} status status code
     * @param {string|object} [reason] reason phrase (ignored) or the header fields
     * @param {object} [fields] header fields
     * @return {this}
     */
    writeHead(status, reason, fields) {
      if (typeof reason === 'object') fields = reason;
      this.statusCode = status;
      if (fields) for (const name in fields) this.setHeader(name, fields[name]);
      return this;
    }

    /**
     * Serializes the header fields. Framing fields (content-length,
     * transfer-encoding), `date` and `connection` are written by the reactor.
     * @return {[string, boolean]} the field lines, and whether `connection: close` was requested
     */
    _head() {
      this.headersSent = true;
      const fields = this._fields;
      let lines = '';
      let close = false;
      if (fields) {
        for (const name in fields) {
          const value = fields[name];
          if (name === 'connection') close = String(value).toLowerCase() === 'close';
          else if (name === 'content-length' || name === 'transfer-encoding' || name === 'date') continue;
          else if (Array.isArray(value)) for (let i = 0; i < value.length; i++) lines += name + ': ' + value[i] + '\r\n';
          else lines += name + ': ' + value + '\r\n';
        }
      }
      return [lines, close];
    }

    /**
     * Streams a piece of the body; the first call sends the head.
     * @param {string|Uint8Array} chunk body bytes
     * @return {boolean} false after end()
     */
    write(chunk) {
      if (this.finished) return false;
      if (!this.headersSent) {
        const head = this._head();
        ops.httpHead(this._k, this.statusCode, head[0], head[1]);
      }
      ops.httpChunk(this._k, chunk);
      return true;
    }

    /**
     * Finishes the response, optionally with a final piece of the body.
     * @param {string|Uint8Array} [chunk] final body bytes
     * @return {this}
     */
    end(chunk) {
      if (this.finished) return this;
      this.finished = true;
      if (this.headersSent) {
        if (chunk !== undefined) ops.httpChunk(this._k, chunk);
        ops.httpEnd(this._k);
      } else {
        const head = this._head();
        ops.httpRespond(this._k, this.statusCode, head[0], chunk, head[1]);
      }
      return this;
    }
  }

  /** An HTTP server. Emits request, listening, error and close. */
  class HttpServer extends Server {
    /**
     * @param {object|Function} [options] options (unused), or the request listener
     * @param {function(IncomingMessage, ServerResponse): void} [onRequest] request listener
     */
    constructor(options, onRequest) {
      super();
      this._http = true;
      if (typeof options === 'function') onRequest = options;
      if (onRequest) this.on('request', onRequest);
    }
  }

  const http = {
    Server: HttpServer,
    IncomingMessage,
    ServerResponse,
    METHODS,
    /**
     * Creates an HTTP server.
     * @param {object|Function} [options] options, or the request listener
     * @param {Function} [onRequest] request listener
     * @return {HttpServer}
     */
    createServer: (options, onRequest) => new HttpServer(options, onRequest),
  };

  // ---------------------------------------------------------------------- fs

  // Operation codes; mirror of `op` in src/fs.rs.
  const FS_READ = 0, FS_READ_UTF8 = 1, FS_WRITE = 2, FS_APPEND = 3, FS_STAT = 4, FS_READDIR = 5, FS_MKDIR = 6,
    FS_MKDIR_P = 7, FS_UNLINK = 8, FS_RMDIR = 9, FS_RENAME = 10, FS_EXISTS = 11, FS_RM_RF = 12;

  /** @type {Map<number, [Function, Function, number]>} pending async requests: resolve, reject, op */
  const fsPending = new Map();
  /** Last request id handed to the reactor; 0 is reserved for synchronous calls. */
  let fsSeq = 0;

  /** Result of stat(): size, times and type tests. */
  class Stats {
    /**
     * @param {[number, number, number]} raw size, mtime in ms, kind (1 file, 2 directory)
     */
    constructor(raw) {
      this.size = raw[0];
      this.mtimeMs = raw[1];
      this._kind = raw[2];
    }

    /** @return {Date} modification time */
    get mtime() {
      return new Date(this.mtimeMs);
    }

    /** @return {boolean} whether the path is a regular file */
    isFile() {
      return this._kind === 1;
    }

    /** @return {boolean} whether the path is a directory */
    isDirectory() {
      return this._kind === 2;
    }
  }

  /**
   * Converts a native fs result to its Node shape.
   * @param {number} op operation code
   * @param {*} value native result
   * @return {*}
   */
  const fsShape = (op, value) => (op === FS_READ ? asBuffer(value) : op === FS_STAT ? new Stats(value) : value);

  /**
   * Runs an fs operation on the worker pool.
   * @param {number} op operation code
   * @param {string} target path
   * @param {string|Uint8Array} [data] payload
   * @return {Promise<*>}
   */
  function fsAsync(op, target, data) {
    return new Promise((resolve, reject) => {
      const id = (fsSeq = (fsSeq % 0x7fffffff) + 1);
      fsPending.set(id, [resolve, reject, op]);
      try {
        ops.fs(op, String(target), data, id);
      } catch (err) {
        fsPending.delete(id);
        reject(err);
      }
    });
  }

  /**
   * Runs an fs operation inline and returns its result.
   * @param {number} op operation code
   * @param {string} target path
   * @param {string|Uint8Array} [data] payload
   * @return {*}
   */
  const fsSync = (op, target, data) => fsShape(op, ops.fs(op, String(target), data, 0));

  /**
   * Picks the read operation for an encoding option.
   * @param {string|{encoding?: string}} [options] encoding or options
   * @return {number} FS_READ or FS_READ_UTF8
   */
  function readOp(options) {
    const enc = typeof options === 'string' ? options : options && options.encoding;
    if (!enc) return FS_READ;
    utf8Only(enc);
    return FS_READ_UTF8;
  }

  /**
   * @param {{recursive?: boolean}} [options] options
   * @return {boolean} whether `recursive` is set
   */
  const recursive = (options) => !!(options && options.recursive);

  /**
   * Builds the three API flavours (promise, callback, sync) from one table, so each
   * operation is declared once.
   * @param {function(number, string, *): *} run fsAsync or fsSync
   * @return {object} the fs functions bound to `run`
   */
  const fsApi = (run) => ({
    readFile: (p, o) => run(readOp(o), p),
    writeFile: (p, data) => run(FS_WRITE, p, data),
    appendFile: (p, data) => run(FS_APPEND, p, data),
    stat: (p) => run(FS_STAT, p),
    readdir: (p) => run(FS_READDIR, p),
    mkdir: (p, o) => run(recursive(o) ? FS_MKDIR_P : FS_MKDIR, p),
    unlink: (p) => run(FS_UNLINK, p),
    rmdir: (p) => run(FS_RMDIR, p),
    rename: (from, to) => run(FS_RENAME, from, String(to)),
    rm: (p, o) => run(recursive(o) ? FS_RM_RF : FS_UNLINK, p),
  });

  const fsPromises = fsApi(fsAsync);
  const fs = { promises: fsPromises, Stats, existsSync: (p) => ops.fs(FS_EXISTS, String(p), undefined, 0) };
  const fsSyncApi = fsApi(fsSync);
  for (const name of Object.keys(fsPromises)) {
    const run = fsPromises[name];
    fs[name + 'Sync'] = fsSyncApi[name];
    // Callback flavour: the last argument is the Node-style callback.
    fs[name] = (...args) => {
      const cb = args.pop();
      if (typeof cb !== 'function') throw new TypeError('The "cb" argument must be a function');
      run(...args).then((value) => cb(null, value), cb);
    };
  }

  // ---------------------------------------------------------------- dispatch

  /**
   * The single entry point for reactor events. Codes mirror `ev` in src/reactor.rs.
   * @param {number} kind event code
   * @param {number} id handle key, timer key or request id
   * @param {*} [a] event argument
   * @param {*} [b] event argument
   * @param {*} [c] event argument
   * @param {*} [d] event argument
   * @param {*} [e] event argument
   * @return {boolean|undefined} for immediates, whether more remain
   */
  function dispatch(kind, id, a, b, c, d, e) {
    switch (kind) {
      case 0: // TIMER (key, oneShot)
        return fireTimer(id, a);
      case 1: // IMMEDIATE
        return runImmediate();
      case 8: { // HTTP (listener, conn, method, url, fields, body)
        const req = new IncomingMessage(typeof b === 'number' ? METHODS[b] : b, c, d, e);
        const res = new ServerResponse(a);
        res.req = req;
        // No listener: answer rather than leave the client hanging.
        if (!emit2(handles[id], 'request', req, res)) res.writeHead(404).end();
        return;
      }
      case 3: { // DATA (conn, chunk)
        const sock = handles[id];
        if (sock) emit2(sock, 'data', asBuffer(a));
        return;
      }
      case 2: { // CONNECTION (listener, conn, address, port)
        const server = handles[id];
        const sock = new Socket(server);
        sock._k = a;
        sock.remoteAddress = b;
        sock.remotePort = c;
        sock.server = server;
        handles[a] = sock;
        emit2(server, 'connection', sock);
        return;
      }
      case 4: { // END (conn)
        const sock = handles[id];
        if (!sock) return;
        emit2(sock, 'end');
        if (!sock.allowHalfOpen) sock.end();
        return;
      }
      case 5: { // CLOSE (conn, message?, code?)
        const sock = handles[id];
        if (!sock) return;
        handles[id] = undefined;
        sock._k = -1;
        sock.destroyed = true;
        sock.connecting = false;
        if (a !== undefined) sock.emit('error', codedError(a, b));
        emit2(sock, 'close', a !== undefined);
        return;
      }
      case 6: { // DRAIN (conn)
        const sock = handles[id];
        if (sock) emit2(sock, 'drain');
        return;
      }
      case 7: { // CONNECT (conn)
        const sock = handles[id];
        if (!sock) return;
        sock.connecting = false;
        const queue = sock._queue;
        sock._queue = undefined;
        // Replay what was written while connecting, then the deferred end().
        const ended = sock._ended;
        sock._ended = false;
        if (queue) for (let i = 0; i < queue.length; i++) sock.write(queue[i][0], queue[i][1]);
        if (ended) sock.end();
        emit2(sock, 'connect');
        return;
      }
      case 9: { // FS (request, value, message?, code?)
        const pending = fsPending.get(id);
        fsPending.delete(id);
        if (b !== undefined) pending[1](codedError(b, c));
        else pending[0](fsShape(pending[2], a));
        return;
      }
    }
  }

  // ----------------------------------------------------------------- modules

  /**
   * The builtin modules by name (without the `node:` prefix). Decision: they are
   * returned to the Runtime, not published on a global, so a script can reach them
   * only through `import`, exactly like `lunarstorm:ipc`. The ported builtins
   * (`node:path`, ...) are already in `native.modules`; this adds the rest.
   */
  const modules = Object.assign(native.modules, {
    net,
    http,
    fs,
    'fs/promises': fsPromises,
  });

  /**
   * What `lunarstorm:internal` exports, for host modules only: the Runtime's natives
   * plus whatever the host registered (`ops`), and the glue's live-region slot
   * (`live`).
   */
  const internal = { ops, live };

  /**
   * Reduces a value to what crosses the host boundary: null, boolean, number,
   * string or bytes. Decision: anything else travels as JSON text, so the host
   * interface stays engine-neutral and holds no JS handles.
   * @param {*} value the value an export returned or resolved to
   * @return {null|boolean|number|string|Uint8Array}
   */
  function plain(value) {
    if (value === undefined || value === null) return null;
    const type = typeof value;
    if (type === 'boolean' || type === 'number' || type === 'string') return value;
    if (value instanceof Uint8Array) return value;
    if (type === 'bigint') return String(value);
    try {
      const json = JSON.stringify(value);
      return json === undefined ? null : json;
    } catch {
      return inspect(value);
    }
  }

  return {
    modules,
    internal,
    /**
     * Lists the named exports of a builtin module.
     * @param {string} name builtin name without the `node:` prefix
     * @return {string} its keys, comma separated
     */
    exportsOf: (name) => Object.keys(modules[name]).join(','),
    dispatch,
    ticks,
    /**
     * Formats a thrown value for the fatal-error report.
     * @param {*} value thrown value or rejection reason
     * @return {string}
     */
    describe: (value) => inspect(value),
    /** @return {number} process.exitCode as an integer */
    exitCode: () => process.exitCode | 0,
    /**
     * Calls an export for the host (`Runtime::call`) and reports how its promise
     * settles through `ops.settled`. A synchronous throw rejects the same promise.
     * @param {Function} fn the export
     * @param {number} id the host's call id
     * @param {...*} args arguments from the host
     * @return {void}
     */
    invoke: (fn, id, ...args) => {
      new Promise((resolve) => resolve(fn(...args))).then(
        (value) => ops.settled(id, true, plain(value)),
        (error) => ops.settled(id, false, inspect(error)),
      );
    },
  };
})
