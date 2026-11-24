// Backend Worker of the browser host (spec 6.3, Browser): it gives the backend the same
// `@desktop/bridge` and `@desktop/api` contract the JS thread's prelude does, over
// postMessage. The server rewrites the backend's bare imports to /__zebra/bridge.js and
// /__zebra/api.js, which read `globalThis.__zebra` set up here before the import.
/** @type {((message: object) => unknown) | null} the generated form dispatch */
let handler = null;
/** @type {Map<string, Map<string, Record<string, unknown>>>} form -> control -> values */
const outbox = new Map();
/** @type {Map<number, {resolve: Function, reject: Function}>} native calls in flight */
const pending = new Map();
let nextCall = 0;
let flushScheduled = false;

/** Sends the property writes collected since the last flush, one update per form. */
const flush = () => {
  flushScheduled = false;
  for (const [form, controls] of outbox) postMessage({ type: 'update', form, controls: Object.fromEntries(controls) });
  outbox.clear();
};

const bridge = Object.freeze({
  /**
   * Queues one property write; writes made in one handler go out as one update.
   * @param {string} form form name
   * @param {string} control control name
   * @param {string} prop property name
   * @param {unknown} value new value
   */
  setProperty(form, control, prop, value) {
    let controls = outbox.get(form);
    if (!controls) outbox.set(form, (controls = new Map()));
    controls.set(control, { ...(controls.get(control) ?? {}), [prop]: value });
    // A macrotask runs after the handler's microtasks, like the native host's flush.
    if (!flushScheduled) {
      flushScheduled = true;
      setTimeout(flush, 0);
    }
  },
  /**
   * Installs the one message handler.
   * @param {(message: object) => unknown} fn generated form dispatch
   */
  serve(fn) {
    handler = fn;
  },
  /**
   * Values carried over from a previous backend; the browser host never reloads.
   * @returns {Record<string, Record<string, unknown>>} always empty
   */
  restored() {
    return {};
  },
});

/** `native.<ns>.<method>(...args)` asks the page host and resolves with its answer. */
const native = new Proxy({}, {
  get: (_, ns) => new Proxy({}, {
    get: (_, method) => (...args) => new Promise((resolve, reject) => {
      const id = ++nextCall;
      pending.set(id, { resolve, reject });
      postMessage({ type: 'native', id, ns: String(ns), method: String(method), args });
    }),
  }),
});

globalThis.__zebra = Object.freeze({ bridge, native });

/**
 * Reports a backend error to the page host.
 * @param {unknown} error thrown value
 */
const report = (error) => postMessage({ type: 'error', message: String(error?.stack ?? error) });
console.log = (...args) => postMessage({ type: 'log', message: args.join(' ') });

onmessage = ({ data }) => {
  if (data.type === 'settle') {
    const call = pending.get(data.id);
    if (!call) return;
    pending.delete(data.id);
    if (data.ok) call.resolve(data.value);
    else call.reject(new Error(String(data.value)));
    return;
  }
  const { envelope } = data;
  const dot = envelope.method.indexOf('.');
  const message = {
    form: envelope.windowId,
    target: envelope.method.slice(0, dot),
    event: envelope.method.slice(dot + 1),
    payload: envelope.payload,
    request: false,
  };
  try {
    const result = handler?.(message);
    if (result && typeof result.then === 'function') result.then(undefined, report);
  } catch (error) {
    report(error);
  }
};

import('/__zebra/backend.js').then(() => postMessage({ type: 'loaded' }), report);
