// The desktop host's glue: the module `lunarstorm:desktop`, evaluated once per
// backend instance, on the first import of `@desktop/bridge` or `@desktop/api`.
//
// It gives the backend its two modules:
//   @desktop/bridge  setProperty, serve, restored   (what the form runtime uses)
//   @desktop/api     native.<namespace>.<method>(...args) -> Promise
// and gives the host two hooks, through `ops.desktopHooks`:
//   dispatch(json)        one envelope from the desktop process (event, request, cancel)
//   settle(id, ok, json)  a native call finished
//
// Decisions:
// - Property writes are collected per form and control and sent by a promise job
//   queued at the first write. Everything a handler writes before it returns or
//   awaits goes out as one envelope per form, and a write made from a timer or
//   after an `await` is sent without the host having to ask for a flush.
// - Timers, `console` and the rest of the globals are the Runtime's own; this
//   file adds nothing to the global object.
const { ops } = __lunarstorm.internal;

/** @type {Map<number, {resolve: Function, reject: Function}>} native calls in flight */
const pending = new Map();
/** @type {Map<string, Map<string, Record<string, unknown>>>} form -> control -> props */
const outbox = new Map();
/** @type {Set<number>} request ids the desktop process cancelled */
const cancelled = new Set();
/** @type {null | ((message: object) => unknown)} the form runtime's dispatch */
let handler = null;
/** Whether a flush of the outbox is already queued. */
let flushing = false;
/** Id of the last native call. */
let lastCall = 0;
/** @type {Record<string, Record<string, Record<string, unknown>>>} carried-over control values */
const restore = JSON.parse(ops.desktopRestore());

/**
 * Describes a thrown value for the error report.
 * @param {unknown} value what was thrown
 * @returns {string} name, message and stack for an error; JSON otherwise
 */
const format = (value) => {
  if (value instanceof Error) return `${value.name}: ${value.message}\n${value.stack ?? ''}`;
  if (typeof value === 'string') return value;
  try { return JSON.stringify(value); } catch { return String(value); }
};

/**
 * Sends one envelope to the desktop process, which stamps version and epoch.
 * @param {object} envelope kind, windowId, method, payload and, for a response, id
 */
const send = (envelope) => ops.desktopSend(JSON.stringify(envelope));

/**
 * Reports an error from a handler or a rejected request.
 * @param {unknown} error what was thrown
 */
const report = (error) => ops.desktopError(format(error));

/** Sends the property writes collected since the last flush, one envelope per form. */
const flush = () => {
  flushing = false;
  for (const [form, controls] of outbox) {
    send({ kind: 'update', windowId: form, method: 'update', payload: { controls: Object.fromEntries(controls) } });
  }
  outbox.clear();
};

export const bridge = Object.freeze({
  /**
   * Writes one control property; it reaches the form's WebView once the running
   * code returns or awaits.
   * @param {string} form form name
   * @param {string} control control name
   * @param {string} prop property name
   * @param {unknown} value the new value
   */
  setProperty(form, control, prop, value) {
    let controls = outbox.get(form);
    if (!controls) outbox.set(form, (controls = new Map()));
    let props = controls.get(control);
    if (!props) controls.set(control, (props = {}));
    props[prop] = value;
    if (!flushing) {
      flushing = true;
      Promise.resolve().then(flush);
    }
  },
  /**
   * Installs the one message handler (the form runtime's dispatch).
   * @param {(message: {form: string, target: string, event: string, payload: unknown, request: boolean}) => unknown} fn
   */
  serve(fn) {
    handler = fn;
  },
  /**
   * Control values carried over from the previous backend instance (spec 6.2).
   * @param {string} form form name
   * @returns {Record<string, Record<string, unknown>>} control -> props
   */
  restored(form) {
    return restore[form] ?? {};
  },
});

export const native = new Proxy({}, {
  get: (_, namespace) => new Proxy({}, {
    get: (_, method) => (...args) => new Promise((resolve, reject) => {
      const id = ++lastCall;
      pending.set(id, { resolve, reject });
      ops.desktopNative(id, String(namespace), String(method), JSON.stringify(args));
    }),
  }),
});

/**
 * Delivers one envelope from the desktop process to the backend.
 * @param {string} json the envelope
 */
const dispatch = (json) => {
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
  /**
   * Answers a request, unless it was cancelled meanwhile.
   * @param {unknown} payload the handler's answer
   */
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
};

/**
 * Settles the promise of a native call.
 * @param {number} id the call's id
 * @param {boolean} ok whether it succeeded
 * @param {string} json the result or the error message as JSON; '' for undefined
 */
const settle = (id, ok, json) => {
  const call = pending.get(id);
  if (!call) return;
  pending.delete(id);
  const value = json === '' ? undefined : JSON.parse(json);
  if (ok) call.resolve(value);
  else call.reject(new Error(String(value)));
};

ops.desktopHooks(dispatch, settle);
