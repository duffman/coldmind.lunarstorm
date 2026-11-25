// lunarstorm JS glue: the Node-compatible surface, built on the native `ops` object.
//
// This file is one function expression. The Runtime evaluates it once at startup,
// calls it with `ops`, and keeps what it returns: the hooks ({ dispatch, ticks,
// describe, exitCode, invoke }) and the module registry ({ modules, internal,
// exportsOf }). Every event the loop delivers enters through `dispatch`.
//
// `ops` holds the Runtime's own natives plus whatever the host registered. Host
// natives are optional here: the glue reads them defensively (`ops.color`), so it
// runs unchanged under a host that registers none.
//
// Conventions:
// - Handles (sockets, servers) are identified by the reactor's slab key and looked up
//   in the dense `handles` array.
// - Hot paths (data, request, timers) avoid rest arrays and closures.
// - Builtin modules are plain objects in the returned `modules` registry; the module
//   loader declares each as a native module (`default` is the object, every key a
//   named export). Nothing is published on the global object.
(function (ops) {
  'use strict';

  // ------------------------------------------------------------------ events

  /**
   * Node-compatible event emitter.
   *
   * Design: a listener slot holds one function or an array. Arrays are copy-on-write
   * (on/off build a new array), so `emit` iterates without copying and a listener
   * added or removed during an emit does not affect that emit.
   */
  class EventEmitter {
    /**
     * Adds a listener.
     * @param {string} type event name
     * @param {Function} fn listener
     * @return {this}
     */
    on(type, fn) {
      const ev = this._ev || (this._ev = Object.create(null));
      const cur = ev[type];
      ev[type] = cur === undefined ? fn : typeof cur === 'function' ? [cur, fn] : cur.concat(fn);
      return this;
    }

    /**
     * Adds a listener that is removed before its first call.
     * @param {string} type event name
     * @param {Function} fn listener
     * @return {this}
     */
    once(type, fn) {
      const wrap = (...args) => {
        this.off(type, wrap);
        fn.apply(this, args);
      };
      wrap.listener = fn;
      return this.on(type, wrap);
    }

    /**
     * Removes a listener (or the `once` wrapper around it).
     * @param {string} type event name
     * @param {Function} fn listener
     * @return {this}
     */
    off(type, fn) {
      const ev = this._ev;
      const cur = ev && ev[type];
      if (cur === undefined) return this;
      if (typeof cur === 'function') {
        if (cur === fn || cur.listener === fn) ev[type] = undefined;
      } else {
        const i = cur.findIndex((f) => f === fn || f.listener === fn);
        if (i >= 0) ev[type] = cur.length === 2 ? cur[1 - i] : cur.filter((_, j) => j !== i);
      }
      return this;
    }

    /**
     * Calls every listener of an event. An `error` event with no listener throws.
     * @param {string} type event name
     * @param {...*} args listener arguments
     * @return {boolean} whether any listener was called
     */
    emit(type, ...args) {
      const cur = this._ev && this._ev[type];
      if (cur === undefined) {
        if (type === 'error') throw args[0] instanceof Error ? args[0] : new Error('Unhandled error: ' + args[0]);
        return false;
      }
      if (typeof cur === 'function') cur.apply(this, args);
      else for (let i = 0; i < cur.length; i++) cur[i].apply(this, args);
      return true;
    }

    /**
     * Removes all listeners of one event, or of every event.
     * @param {string} [type] event name
     * @return {this}
     */
    removeAllListeners(type) {
      if (type === undefined) this._ev = undefined;
      else if (this._ev) this._ev[type] = undefined;
      return this;
    }

    /**
     * Returns a copy of the listeners of an event.
     * @param {string} type event name
     * @return {Function[]}
     */
    listeners(type) {
      const cur = this._ev && this._ev[type];
      return cur === undefined ? [] : typeof cur === 'function' ? [cur] : cur.slice();
    }

    /**
     * Counts the listeners of an event.
     * @param {string} type event name
     * @return {number}
     */
    listenerCount(type) {
      return this.listeners(type).length;
    }
  }
  EventEmitter.prototype.addListener = EventEmitter.prototype.on;
  EventEmitter.prototype.removeListener = EventEmitter.prototype.off;
  // Node exports the class itself as `events`, with `EventEmitter` as a property.
  EventEmitter.EventEmitter = EventEmitter;

  /**
   * `emit` for hot paths: at most two arguments, no rest-array allocation.
   * @param {EventEmitter} self emitter
   * @param {string} type event name
   * @param {*} [a] first argument
   * @param {*} [b] second argument
   * @return {boolean} whether any listener was called
   */
  function emit2(self, type, a, b) {
    const cur = self._ev && self._ev[type];
    if (cur === undefined) return false;
    if (typeof cur === 'function') cur.call(self, a, b);
    else for (let i = 0; i < cur.length; i++) cur[i].call(self, a, b);
    return true;
  }

  // ------------------------------------------------------------------ timers

  // A timer id is `key * 2^32 + seq`: `key` indexes this table and the reactor's slab,
  // `seq` makes a stale id (already fired or cleared) harmless after the key is reused.
  const TIMER_SHIFT = 4294967296;
  /** @type {(Function|undefined)[]} callbacks of armed timers, by key */
  const timers = [];

  /**
   * Arms a timer.
   * @param {Function} fn callback
   * @param {number} ms delay in milliseconds
   * @param {boolean} repeat whether the timer repeats
   * @param {Array} args extra arguments for the callback
   * @return {number} timer id
   */
  function setTimer(fn, ms, repeat, args) {
    if (typeof fn !== 'function') throw new TypeError('The "callback" argument must be a function');
    const id = ops.timerStart(+ms || 0, repeat);
    timers[Math.floor(id / TIMER_SHIFT)] = args.length ? () => fn(...args) : fn;
    return id;
  }

  /**
   * Cancels a timer. Unknown and stale ids are ignored.
   * @param {number} id timer id
   */
  function clearTimer(id) {
    if (typeof id === 'number' && ops.timerCancel(id)) timers[Math.floor(id / TIMER_SHIFT)] = undefined;
  }

  globalThis.setTimeout = (fn, ms, ...args) => setTimer(fn, ms, false, args);
  globalThis.setInterval = (fn, ms, ...args) => setTimer(fn, ms, true, args);
  globalThis.clearTimeout = globalThis.clearInterval = clearTimer;

  /** @type {{fn: Function, args: Array, live: boolean}[]} immediates queued for the next check phase */
  let immQueue = [];
  /** @type {{fn: Function, args: Array, live: boolean}[]} batch being run by the reactor */
  let immBatch = [];
  /** Index of the next immediate of the batch. */
  let immAt = 0;

  globalThis.setImmediate = (fn, ...args) => {
    if (typeof fn !== 'function') throw new TypeError('The "callback" argument must be a function');
    if (immQueue.length === 0) ops.immediate();
    const handle = { fn, args, live: true };
    immQueue.push(handle);
    return handle;
  };
  globalThis.clearImmediate = (handle) => {
    if (handle) handle.live = false;
  };

  /**
   * Runs one immediate of the current batch. The reactor calls this once per
   * callback so promise jobs run between immediates, as in Node.
   * @return {boolean} whether the batch has more callbacks
   */
  function runImmediate() {
    if (immAt === immBatch.length) {
      // Defect fixed: a fresh array per batch and per drained callback allocated on
      // every immediate of a chain. Decision: the two arrays swap roles and the
      // drained one is cleared with `length = 0`, the same reuse `ticks()` has.
      const spent = immBatch;
      immBatch = immQueue;
      immQueue = spent;
      spent.length = 0;
      immAt = 0;
    }
    const handle = immBatch[immAt++];
    const more = immAt < immBatch.length;
    if (!more) {
      immBatch.length = 0;
      immAt = 0;
    }
    if (handle && handle.live) {
      if (handle.args.length) handle.fn(...handle.args);
      else handle.fn();
    }
    return more;
  }

  /**
   * Callbacks queued by process.nextTick, consumed from `tickAt`.
   *
   * Decision: one array with a read index, reused from the start whenever the
   * drain catches up. The earlier design swapped in a fresh array for every
   * round of the drain, which cost an allocation per callback in a chain where
   * each tick queues the next.
   * @type {Function[]}
   */
  let tickQueue = [];
  /** Index in `tickQueue` of the next callback to run. */
  let tickAt = 0;
  /**
   * Whether `ticks` is draining. A callback queued during the drain is picked up
   * by the drain itself, so the native `ops.tick()` wake-up is skipped for it.
   * A callback that throws leaves this set; that is an uncaught error, after
   * which the loop runs no more JS.
   */
  let ticking = false;

  /**
   * Queues a callback to run before promise jobs, after the current JS call.
   * @param {Function} fn callback
   * @param {...*} args callback arguments
   */
  function nextTick(fn, ...args) {
    if (!ticking && tickQueue.length === 0) ops.tick();
    tickQueue.push(args.length ? () => fn(...args) : fn);
  }

  /** Runs the tick queue until it is empty. Called by the loop. */
  function ticks() {
    ticking = true;
    while (tickAt < tickQueue.length) {
      const fn = tickQueue[tickAt++];
      // Caught up: restart the array before the call, so what `fn` queues lands
      // at index 0 and an endless chain of ticks never grows the queue.
      if (tickAt === tickQueue.length) {
        tickQueue.length = 0;
        tickAt = 0;
      }
      fn();
    }
    ticking = false;
  }

  if (typeof globalThis.queueMicrotask !== 'function') {
    globalThis.queueMicrotask = (fn) => {
      Promise.resolve().then(fn);
    };
  }

  // ------------------------------------------------------------------ buffer

  /**
   * Rejects encodings the runtime does not implement instead of mis-decoding.
   * @param {string} [enc] encoding name
   */
  function utf8Only(enc) {
    if (enc !== undefined && !/^utf-?8$/i.test(enc)) throw new TypeError('Unsupported encoding: ' + enc);
  }

  /** Node's Buffer: a Uint8Array with UTF-8 helpers. Only UTF-8 is implemented. */
  class Buffer extends Uint8Array {
    /**
     * Creates a buffer from a string, array, typed array or ArrayBuffer.
     * @param {string|ArrayLike<number>|ArrayBuffer} value source
     * @param {string} [enc] encoding of a string source
     * @return {Buffer}
     */
    static from(value, enc) {
      if (typeof value === 'string') {
        utf8Only(enc);
        return asBuffer(ops.utf8Encode(value));
      }
      return asBuffer(new Uint8Array(value));
    }

    /**
     * Allocates a zero-filled buffer.
     * @param {number} size length in bytes
     * @return {Buffer}
     */
    static alloc(size) {
      return new Buffer(size);
    }

    /**
     * Joins buffers.
     * @param {Uint8Array[]} list parts
     * @param {number} [total] combined length, when known
     * @return {Buffer}
     */
    static concat(list, total) {
      if (list.length === 1 && total === undefined) return asBuffer(list[0]);
      if (total === undefined) {
        total = 0;
        for (let i = 0; i < list.length; i++) total += list[i].length;
      }
      const out = new Buffer(total);
      let at = 0;
      for (let i = 0; i < list.length && at < total; i++) {
        const part = at + list[i].length > total ? list[i].subarray(0, total - at) : list[i];
        out.set(part, at);
        at += part.length;
      }
      return out;
    }

    /**
     * Byte length of a string in UTF-8, or of a buffer.
     * @param {string|Uint8Array} value input
     * @return {number}
     */
    static byteLength(value) {
      return typeof value === 'string' ? ops.utf8Encode(value).length : value.byteLength;
    }

    /**
     * Tests whether a value is a Buffer.
     * @param {*} value input
     * @return {boolean}
     */
    static isBuffer(value) {
      return value instanceof Buffer;
    }

    /**
     * Decodes the buffer as UTF-8 (invalid sequences become U+FFFD).
     * @param {string} [enc] encoding
     * @return {string}
     */
    toString(enc) {
      utf8Only(enc);
      return ops.utf8Decode(this);
    }
  }

  /**
   * Re-types a Uint8Array as a Buffer without copying its bytes.
   * @param {Uint8Array} bytes array from native code
   * @return {Buffer}
   */
  function asBuffer(bytes) {
    return Object.setPrototypeOf(bytes, Buffer.prototype);
  }

  globalThis.Buffer = Buffer;
  globalThis.TextEncoder = class TextEncoder {
    /** @return {string} always "utf-8" */
    get encoding() {
      return 'utf-8';
    }

    /**
     * Encodes a string as UTF-8.
     * @param {string} [text] input
     * @return {Uint8Array}
     */
    encode(text = '') {
      return ops.utf8Encode(String(text));
    }
  };
  globalThis.TextDecoder = class TextDecoder {
    /**
     * @param {string} [enc] encoding; only UTF-8 is implemented
     */
    constructor(enc) {
      if (enc !== undefined && !/^utf-?8$/i.test(enc)) throw new RangeError('Unsupported encoding: ' + enc);
    }

    /** @return {string} always "utf-8" */
    get encoding() {
      return 'utf-8';
    }

    /**
     * Decodes UTF-8 bytes.
     * @param {ArrayBuffer|ArrayBufferView} [input] bytes
     * @return {string}
     */
    decode(input) {
      if (input === undefined) return '';
      if (input instanceof Uint8Array) return ops.utf8Decode(input);
      return ops.utf8Decode(
        ArrayBuffer.isView(input) ? new Uint8Array(input.buffer, input.byteOffset, input.byteLength) : new Uint8Array(input),
      );
    }
  };

  // ----------------------------------------------------------------- console

  /**
   * Formats an error with its name, message and stack.
   * @param {Error} err error
   * @return {string}
   */
  function errorText(err) {
    const head = err.name + (err.message ? ': ' + err.message : '');
    const stack = typeof err.stack === 'string' ? err.stack.trimEnd() : '';
    return !stack ? head : stack.startsWith(head) ? stack : head + '\n' + stack;
  }

  /**
   * Wraps text in one SGR color when coloring is on.
   * @param {boolean} c whether to color
   * @param {string} code SGR parameter
   * @param {string} text text to wrap
   * @return {string}
   */
  const paint = (c, code, text) => (c ? '\x1b[' + code + 'm' + text + '\x1b[0m' : text);

  /**
   * Renders a value the way `console.log` shows it.
   * @param {*} v value
   * @param {number} depth nesting level; 0 is the top
   * @param {object[]} seen ancestors, for cycle detection
   * @return {string}
   */
  function inspect(v, depth, seen, c) {
    const t = typeof v;
    if (t === 'string') return depth === 0 ? v : paint(c, '32', "'" + v + "'");
    if (t === 'function') return paint(c, '36', v.name ? '[Function: ' + v.name + ']' : '[Function (anonymous)]');
    if (t === 'bigint') return paint(c, '33', v + 'n');
    if (t === 'symbol') return paint(c, '32', v.toString());
    if (t !== 'object' || v === null) return paint(c, v === null ? '1' : v === undefined ? '90' : '33', String(v));
    if (v instanceof Error) return errorText(v);
    if (v instanceof Date) return paint(c, '35', isNaN(v) ? 'Invalid Date' : v.toISOString());
    if (v instanceof RegExp) return paint(c, '31', String(v));
    if (seen.includes(v)) return paint(c, '36', '[Circular]');
    const list = Array.isArray(v) || ArrayBuffer.isView(v);
    if (depth > 2) return paint(c, '36', list ? '[Array]' : '[Object]');
    seen.push(v);
    const name = v.constructor && v.constructor.name;
    let out;
    if (list) {
      const parts = [];
      const shown = Math.min(v.length, 100);
      for (let i = 0; i < shown; i++) parts.push(inspect(v[i], depth + 1, seen, c));
      if (v.length > shown) parts.push('... ' + (v.length - shown) + ' more items');
      out = (Array.isArray(v) ? '' : name + '(' + v.length + ') ') + (parts.length ? '[ ' + parts.join(', ') + ' ]' : '[]');
    } else if (v instanceof Map || v instanceof Set) {
      const parts = [];
      if (v instanceof Map) for (const [k, x] of v) parts.push(inspect(k, depth + 1, seen, c) + ' => ' + inspect(x, depth + 1, seen, c));
      else for (const x of v) parts.push(inspect(x, depth + 1, seen, c));
      out = name + '(' + v.size + ') ' + (parts.length ? '{ ' + parts.join(', ') + ' }' : '{}');
    } else {
      const parts = [];
      for (const k of Object.keys(v)) {
        parts.push((/^[A-Za-z_$][\w$]*$/.test(k) ? k : "'" + k + "'") + ': ' + inspect(v[k], depth + 1, seen, c));
      }
      const tag = name === undefined ? '[Object: null prototype] ' : name && name !== 'Object' ? name + ' ' : '';
      out = tag + (parts.length ? '{ ' + parts.join(', ') + ' }' : '{}');
    }
    seen.pop();
    return out;
  }

  /**
   * Joins console arguments, honouring printf-style placeholders in the first one.
   * @param {Array} args arguments
   * @param {boolean} [c] color inspected values
   * @return {string}
   */
  function format(args, c) {
    let i = 0;
    let out = '';
    if (typeof args[0] === 'string' && args.length > 1 && args[0].includes('%')) {
      i = 1;
      out = args[0].replace(/%([sdifjoO%])/g, (match, kind) => {
        if (kind === '%') return '%';
        if (i >= args.length) return match;
        const a = args[i++];
        switch (kind) {
          case 's':
            return typeof a === 'string' ? a : inspect(a, 1, [], c);
          case 'd':
            return typeof a === 'bigint' ? a + 'n' : String(Number(a));
          case 'i':
            return String(parseInt(a, 10));
          case 'f':
            return String(parseFloat(a));
          case 'j':
            return JSON.stringify(a);
          default:
            return inspect(a, 0, [], c);
        }
      });
    } else if (args.length) {
      i = 1;
      out = inspect(args[0], 0, [], c);
    }
    for (; i < args.length; i++) out += ' ' + inspect(args[i], 0, [], c);
    return out;
  }

  /**
   * Whether console output to each fd is colored; decided once at startup by the
   * CLI (`--color`, FORCE_COLOR, NO_COLOR, tty). Index 0 is unused.
   * @type {boolean[]}
   */
  // `ops.color` is a host binding (the CLI host registers it); other hosts get
  // uncolored console output.
  const depth = ops.color || [0, 0];
  const tint = [false, depth[0] > 0, depth[1] > 0];

  /**
   * Hook the `lunarstorm:ui` module installs while progress bars are on screen.
   * `wrap(fd, text)` erases the bars, prints the line and redraws them, so console
   * output scrolls above a live region. `null` (the normal case) costs one
   * property load per console call.
   * @type {{wrap: ?function(number, string): void}}
   */
  const live = { wrap: null };

  /**
   * Builds a console method that writes one formatted line.
   * @param {number} fd 1 for stdout, 2 for stderr
   * @return {function(...*): void}
   */
  const printer = (fd) => (...args) => {
    const text = format(args, tint[fd]) + '\n';
    if (live.wrap === null) ops.print(fd, text);
    else live.wrap(fd, text);
  };
  globalThis.console = {
    log: printer(1),
    info: printer(1),
    debug: printer(1),
    error: printer(2),
    warn: printer(2),
    /**
     * Prints a message and the current stack to stderr.
     * @param {...*} args message
     */
    trace(...args) {
      ops.print(2, 'Trace: ' + format(args, tint[2]) + '\n' + new Error().stack);
    },
  };

  // ----------------------------------------------------------------- process

  const process = new EventEmitter();
  Object.assign(process, {
    argv: ops.argv,
    pid: ops.pid,
    platform: ops.platform,
    arch: ops.arch,
    version: 'v' + ops.version,
    versions: { lunarstorm: ops.version },
    exitCode: undefined,
    cwd: ops.cwd,
    nextTick,
    /**
     * Ends the process now.
     * @param {number} [code] exit status; defaults to process.exitCode, then 0
     */
    exit(code) {
      ops.exit((code === undefined ? process.exitCode : code) | 0);
    },
    /** @return {number} seconds since the process started */
    uptime: () => ops.now() / 1000,
    hrtime: {
      /** @return {bigint} nanoseconds since the process started */
      bigint: () => BigInt(Math.round(ops.now() * 1e6)),
    },
    stdout: {
      /**
       * Writes text to stdout.
       * @param {*} text output
       * @return {boolean} always true
       */
      write(text) {
        ops.print(1, String(text));
        return true;
      },
    },
    stderr: {
      /**
       * Writes text to stderr.
       * @param {*} text output
       * @return {boolean} always true
       */
      write(text) {
        ops.print(2, String(text));
        return true;
      },
    },
  });
  // The environment is copied from the OS on first use: most scripts never read it.
  Object.defineProperty(process, 'env', {
    enumerable: true,
    configurable: true,
    get() {
      const env = ops.env();
      Object.defineProperty(process, 'env', { value: env, writable: true, enumerable: true });
      return env;
    },
  });
  globalThis.process = process;
  if (typeof globalThis.crypto !== 'object') {
    globalThis.crypto = {
      /**
       * Fills a typed array with random bytes from the host's entropy source.
       * @param {Uint8Array} array integer typed array of at most 65536 bytes
       * @return {Uint8Array} the same array
       */
      getRandomValues(array) {
        const bytes = new Uint8Array(array.buffer, array.byteOffset, array.byteLength);
        bytes.set(ops.random(bytes.length));
        return array;
      },
    };
  }
  globalThis.global = globalThis;
  if (typeof globalThis.performance !== 'object') globalThis.performance = { now: ops.now };

  // -------------------------------------------------------------------- path

  /** POSIX path helpers. */
  const path = {
    sep: '/',
    delimiter: ':',
    /**
     * @param {string} p path
     * @return {boolean} whether the path starts at the root
     */
    isAbsolute: (p) => p.charCodeAt(0) === 47,
    /**
     * Resolves `.` and `..` segments and repeated slashes.
     * @param {string} p path
     * @return {string}
     */
    normalize(p) {
      const abs = p.charCodeAt(0) === 47;
      const out = [];
      for (const part of p.split('/')) {
        if (part === '' || part === '.') continue;
        if (part !== '..') out.push(part);
        else if (out.length && out[out.length - 1] !== '..') out.pop();
        else if (!abs) out.push('..');
      }
      let joined = out.join('/');
      if (joined && p.endsWith('/')) joined += '/';
      return abs ? '/' + joined : joined || '.';
    },
    /**
     * Joins segments and normalizes the result.
     * @param {...string} parts segments
     * @return {string}
     */
    join(...parts) {
      return path.normalize(parts.filter(Boolean).join('/') || '.');
    },
    /**
     * Resolves segments right to left into an absolute path.
     * @param {...string} parts segments
     * @return {string}
     */
    resolve(...parts) {
      let joined = '';
      for (let i = parts.length - 1; i >= 0 && joined.charCodeAt(0) !== 47; i--) {
        if (parts[i]) joined = parts[i] + '/' + joined;
      }
      if (joined.charCodeAt(0) !== 47) joined = ops.cwd() + '/' + joined;
      const out = path.normalize(joined);
      return out.length > 1 && out.endsWith('/') ? out.slice(0, -1) : out;
    },
    /**
     * @param {string} p path
     * @return {string} the directory part
     */
    dirname(p) {
      const trimmed = p.replace(/\/+$/, '');
      if (trimmed === '') return p.charCodeAt(0) === 47 ? '/' : '.';
      const i = trimmed.lastIndexOf('/');
      return i < 0 ? '.' : i === 0 ? '/' : trimmed.slice(0, i);
    },
    /**
     * @param {string} p path
     * @param {string} [ext] suffix to strip
     * @return {string} the last segment
     */
    basename(p, ext) {
      const trimmed = p.replace(/\/+$/, '');
      const base = trimmed.slice(trimmed.lastIndexOf('/') + 1);
      return ext && base.endsWith(ext) && base !== ext ? base.slice(0, -ext.length) : base;
    },
    /**
     * @param {string} p path
     * @return {string} the extension including its dot, or ''
     */
    extname(p) {
      const base = path.basename(p);
      const i = base.lastIndexOf('.');
      return i <= 0 ? '' : base.slice(i);
    },
  };

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
      case 0: { // TIMER (key, oneShot)
        const fn = timers[id];
        if (a) timers[id] = undefined;
        fn();
        return;
      }
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

  const timersModule = {
    setTimeout: globalThis.setTimeout,
    setInterval: globalThis.setInterval,
    setImmediate: globalThis.setImmediate,
    clearTimeout: clearTimer,
    clearInterval: clearTimer,
    clearImmediate: globalThis.clearImmediate,
  };
  /**
   * The builtin modules by name (without the `node:` prefix). Decision: they are
   * returned to the Runtime, not published on a global, so a script can reach them
   * only through `import`, exactly like `lunarstorm:ipc`.
   */
  const modules = {
    events: EventEmitter,
    net,
    http,
    fs,
    'fs/promises': fsPromises,
    path,
    process,
    buffer: { Buffer },
    timers: timersModule,
  };

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
      return inspect(value, 0, []);
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
    describe: (value) => inspect(value, 0, []),
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
        (error) => ops.settled(id, false, inspect(error, 0, [])),
      );
    },
  };
})
