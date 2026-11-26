// In-page host for running a model project in a browser tab (spec 6.3, Browser).
//
// It stands in for the native host: it gives the unchanged shell a `window.ipc`, runs the
// backend in a module Worker, validates WebView and backend messages against the form's
// table, keeps the property mirror, drives `timer` components and answers `os.info`;
// every other native call is refused.
//
// Decisions:
// - One tab shows the main form only. Other forms, menus, tray icons and window
//   properties have no browser equivalent: their values are mirrored, their native calls
//   are refused, not faked.
// - onCloseQuery is not sent: `beforeunload` cannot wait for an asynchronous answer.
// - This script runs before the shell (the server injects it first), so `window.ipc`
//   exists when the shell posts its hello.
//
// The server replaces __ZEBRA_CONFIG__ with the host configuration.
(() => {
  'use strict';
  const config = __ZEBRA_CONFIG__;
  const form = config.application.mainForm;
  const table = config.forms.find((f) => f.name === form);
  /** The browser host never reloads the backend, so the epoch is fixed. */
  const epoch = 1;
  /** Kinds the native host applies itself instead of posting them to the WebView. */
  const NATIVE_KINDS = ['menuitem', 'trayicon', 'timer'];
  /** @type {Record<string, Record<string, unknown>>} control -> latest values */
  const mirror = {};
  /** @type {Map<string, {interval: number, enabled: boolean, handle: number | undefined}>} */
  const timers = new Map();
  let ready = false;

  /**
   * Delivers one envelope to the shell.
   * @param {object} envelope kind, method, payload
   */
  const deliver = (envelope) => window.__zebraReceive({ version: 1, windowId: form, epoch, ...envelope });

  /**
   * Records values in the mirror.
   * @param {string} name control name
   * @param {Record<string, unknown>} props values
   */
  const remember = (name, props) => {
    mirror[name] = Object.assign(mirror[name] ?? {}, props);
  };

  /**
   * Checks control values against the form's table.
   * @param {string} name control name
   * @param {Record<string, unknown>} props values
   * @returns {boolean} true when every property is declared for the control
   */
  const declared = (name, props) => !!table.controls[name] && Object.keys(props).every((p) => table.controls[name].props.includes(p));

  const worker = new Worker('/__zebra/worker.js', { type: 'module' });
  /** @type {() => void} */
  let markLoaded;
  const loaded = new Promise((resolve) => (markLoaded = resolve));

  /**
   * Sends an event to the backend once it has loaded.
   * @param {string} method `<target>.<event>`
   * @param {unknown} payload event payload
   */
  const toBackend = (method, payload) =>
    loaded.then(() => worker.postMessage({ type: 'dispatch', envelope: { kind: 'event', windowId: form, epoch, method, payload } }));

  /**
   * (Re)starts a timer component from its current values.
   * @param {string} name timer name
   */
  const arm = (name) => {
    const timer = timers.get(name);
    clearInterval(timer.handle);
    timer.handle = timer.enabled && timer.interval > 0 ? setInterval(() => toBackend(`${name}.onTimer`, {}), timer.interval) : undefined;
  };

  /**
   * Answers a native call a browser can serve.
   * @param {string} ns namespace, e.g. `os`
   * @param {string} method method name
   * @returns {{ok: boolean, value: unknown}} result or error message
   */
  const native = (ns, method) => {
    const allowed = config.permissions?.native ?? [];
    if (!allowed.includes(`${ns}.${method}`) && !allowed.includes(`${ns}.*`)) {
      return { ok: false, value: `native method ${ns}.${method} is not permitted by the host configuration` };
    }
    if (ns === 'os' && method === 'info') {
      return { ok: true, value: { platform: 'browser', arch: 'web', family: 'web', locale: navigator.language } };
    }
    return { ok: false, value: `${ns}.${method} is not available in the browser host` };
  };

  /**
   * Applies a backend update: timers here, other native components to the mirror only,
   * controls to the WebView (the same routing as the native host's apply_update).
   * @param {string} target form the update is for
   * @param {Record<string, Record<string, unknown>>} controls values by control
   */
  const applyUpdate = (target, controls) => {
    if (target !== form) return console.error(`[zebra] update for ${target} ignored: the browser host shows ${form} only`);
    const bad = Object.entries(controls).find(([name, props]) => !declared(name, props));
    if (bad) return console.error('[zebra] refused backend update for', bad[0]);
    const web = {};
    for (const [name, props] of Object.entries(controls)) {
      remember(name, props);
      const kind = table.controls[name].type;
      if (kind === 'timer' && timers.has(name)) {
        Object.assign(timers.get(name), props);
        arm(name);
      } else if (!NATIVE_KINDS.includes(kind)) web[name] = props;
    }
    if (ready && Object.keys(web).length > 0) deliver({ kind: 'update', method: 'update', payload: { controls: web } });
  };

  worker.onmessage = ({ data }) => {
    switch (data.type) {
      case 'loaded':
        markLoaded();
        break;
      case 'update':
        applyUpdate(data.form, data.controls);
        break;
      case 'native':
        worker.postMessage({ type: 'settle', id: data.id, ...native(data.ns, data.method) });
        break;
      case 'error':
        console.error('[backend]', data.message);
        deliver({ kind: 'dev', method: 'dev.error', payload: { errors: [{ message: data.message }] } });
        break;
      case 'log':
        console.log('[backend]', data.message);
        break;
    }
  };

  window.ipc = {
    /**
     * Receives one envelope from the shell.
     * @param {string} text JSON envelope
     */
    postMessage(text) {
      const envelope = JSON.parse(text);
      if (envelope.windowId !== form) return;
      if (envelope.kind === 'hello') {
        ready = true;
        const controls = Object.fromEntries(Object.entries(mirror).filter(([name]) => !NATIVE_KINDS.includes(table.controls[name]?.type)));
        deliver({ kind: 'hello', method: 'session.ready', payload: { controls } });
        return;
      }
      if (envelope.kind !== 'event') return;
      const dot = envelope.method.indexOf('.');
      const control = envelope.method.slice(0, dot);
      const event = envelope.method.slice(dot + 1);
      const props = envelope.payload?.props ?? {};
      if (!declared(control, props) || !table.controls[control].events.includes(event)) {
        console.error('[zebra] refused WebView message', envelope.method);
        return;
      }
      remember(control, props);
      toBackend(envelope.method, envelope.payload);
    },
  };

  for (const timer of config.timers ?? []) {
    if (timer.owner !== form) continue;
    timers.set(timer.name, { interval: timer.interval, enabled: timer.enabled !== false, handle: undefined });
    arm(timer.name);
  }
  if (table.events.includes('onCreate')) toBackend(`${form}.onCreate`, {});
  if (table.events.includes('onShow')) toBackend(`${form}.onShow`, {});
  if (table.events.includes('onResize')) {
    window.addEventListener('resize', () => toBackend(`${form}.onResize`, { width: innerWidth, height: innerHeight }));
  }
  document.title = table.props.caption ?? form;
})();
