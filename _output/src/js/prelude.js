// JS-thread glue. Evaluated once per Instance, before the backend artifact.
//
// It receives the native ops object and returns the host object Rust calls into:
//   dispatch(json)  one host envelope (event, request, cancel)
//   fire(id)        a timer is due
//   settle(id, ok, json)  a native call finished
//   flush()         send the property writes collected since the last flush
//   setRestore(json)      carried-over control values (6.2)
//
// It also defines the two built-in modules' contents on `globalThis.__zebra`:
//   @desktop/bridge  setProperty, serve, restored   (what generated control proxies use)
//   @desktop/api     native.<namespace>.<method>(...args) -> Promise
//
// Decision: property writes are collected per form and control and sent once per
// dispatched message, so a handler that sets ten properties costs one envelope.
(function (ops) {
  'use strict';
  /** @type {Map<number, {fn: Function, args: unknown[], repeat: boolean}>} */
  const timers = new Map();
  /** @type {Map<number, {resolve: Function, reject: Function}>} */
  const pending = new Map();
  /** @type {Map<string, Map<string, Record<string, unknown>>>} form -> control -> props */
  const outbox = new Map();
  /** @type {Set<number>} request ids the host cancelled */
  const cancelled = new Set();
  /** @type {null | ((message: object) => unknown)} */
  let handler = null;
  /** @type {Record<string, Record<string, Record<string, unknown>>>} */
  let restore = {};

  /**
   * Formats one console argument.
   * @param {unknown} value
   * @returns {string}
   */
  const format = (value) => {
    if (typeof value === 'string') return value;
    if (value instanceof Error) return `${value.name}: ${value.message}\n${value.stack ?? ''}`;
    try { return JSON.stringify(value); } catch { return String(value); }
  };
  /**
   * Builds one console method.
   * @param {string} level
   * @returns {(...args: unknown[]) => void}
   */
  const logger = (level) => (...args) => ops.log(level, args.map(format).join(' '));
  globalThis.console = { log: logger('info'), info: logger('info'), debug: logger('debug'), warn: logger('warn'), error: logger('error') };

  /**
   * Registers a timer.
   * @param {boolean} repeat
   * @returns {(fn: Function, ms?: number, ...args: unknown[]) => number}
   */
  const schedule = (repeat) => (fn, ms = 0, ...args) => {
    if (typeof fn !== 'function') throw new TypeError('timer callback must be a function');
    const id = ops.timer(Math.max(0, Number(ms) || 0), repeat);
    timers.set(id, { fn, args, repeat });
    return id;
  };
  globalThis.setTimeout = schedule(false);
  globalThis.setInterval = schedule(true);
  globalThis.clearTimeout = globalThis.clearInterval = (id) => {
    if (timers.delete(id)) ops.clearTimer(id);
  };
  if (typeof globalThis.queueMicrotask !== 'function') {
    globalThis.queueMicrotask = (fn) => { Promise.resolve().then(fn); };
  }

  /**
   * Sends one envelope to the host; Rust stamps version and epoch.
   * @param {object} envelope
   */
  const send = (envelope) => ops.send(JSON.stringify(envelope));

  /**
   * Reports an error from a handler or a rejected request.
   * @param {unknown} error
   */
  const report = (error) => ops.error(format(error));

  const bridge = Object.freeze({
    /**
     * Writes one control property; it reaches the form's WebView after the handler.
     * @param {string} form
     * @param {string} control
     * @param {string} prop
     * @param {unknown} value
     */
    setProperty(form, control, prop, value) {
      let controls = outbox.get(form);
      if (!controls) outbox.set(form, (controls = new Map()));
      let props = controls.get(control);
      if (!props) controls.set(control, (props = {}));
      props[prop] = value;
    },
    /**
     * Installs the one message handler (generated form dispatch).
     * @param {(message: {form: string, target: string, event: string, payload: unknown, request: boolean}) => unknown} fn
     */
    serve(fn) {
      handler = fn;
    },
    /**
     * Control values carried over from the previous backend instance.
     * @param {string} form
     * @returns {Record<string, Record<string, unknown>>}
     */
    restored(form) {
      return restore[form] ?? {};
    },
  });

  const native = new Proxy({}, {
    get: (_, ns) => new Proxy({}, {
      get: (_, method) => (...args) => new Promise((resolve, reject) => {
        const id = ops.native(String(ns), String(method), JSON.stringify(args));
        pending.set(id, { resolve, reject });
      }),
    }),
  });

  globalThis.__zebra = Object.freeze({ bridge, native });

  return {
    /**
     * Delivers one host envelope to the backend.
     * @param {string} json
     */
    dispatch(json) {
      const envelope = JSON.parse(json);
      if (envelope.kind === 'cancel') {
        cancelled.add(envelope.id);
        return;
      }
      const dot = envelope.method.indexOf('.');
      const message = {
        form: envelope.windowId,
        target: envelope.method.slice(0, dot),
        event: envelope.method.slice(dot + 1),
        payload: envelope.payload,
        request: envelope.kind === 'request',
      };
      const answer = (payload) => {
        if (cancelled.delete(envelope.id)) return;
        send({ kind: 'response', id: envelope.id, windowId: envelope.windowId, method: envelope.method, payload: payload ?? {} });
      };
      let result;
      try {
        result = handler ? handler(message) : undefined;
      } catch (error) {
        report(error);
        if (message.request) answer({});
        return;
      }
      if (message.request) {
        Promise.resolve(result).then(answer, (error) => { report(error); answer({}); });
      } else if (result && typeof result.then === 'function') {
        result.then(undefined, report);
      }
    },
    /** @param {number} id */
    fire(id) {
      const timer = timers.get(id);
      if (!timer) return;
      if (!timer.repeat) timers.delete(id);
      timer.fn(...timer.args);
    },
    /**
     * @param {number} id
     * @param {boolean} ok
     * @param {string} json
     */
    settle(id, ok, json) {
      const call = pending.get(id);
      if (!call) return;
      pending.delete(id);
      const value = json === '' ? undefined : JSON.parse(json);
      if (ok) call.resolve(value);
      else call.reject(new Error(String(value)));
    },
    flush() {
      for (const [form, controls] of outbox) {
        send({ kind: 'update', windowId: form, method: 'update', payload: { controls: Object.fromEntries(controls) } });
      }
      outbox.clear();
    },
    /** @param {string} json */
    setRestore(json) {
      restore = JSON.parse(json);
    },
  };
})
