// Framework-owned shell runtime, injected into every form's WebView before any page
// script runs (spec 6: the IPC session starts before the form mounts).
//
// It is the IPC client and update queue that <ShellDebug> and <Shell> wrap:
//   window.zebra.state           control -> latest property values
//   window.zebra.emit(c, e, p)   send a control event with the values it changed
//   window.zebra.onUpdate(fn)    property updates, (controls) => void
//   window.zebra.onRefresh(fn)   dev.refresh of this form, (stamp) => void
//
// The host replaces __ZEBRA_BOOT__ with {windowId, epoch, mode}.
(() => {
  'use strict';
  const boot = __ZEBRA_BOOT__;
  // The browser host (mode 'browser') is a dev tool, so it gets the overlay as well.
  const debug = boot.mode !== 'release';
  /** @type {Record<string, Record<string, unknown>>} */
  const state = Object.create(null);
  const updateListeners = new Set();
  const refreshListeners = new Set();
  /** Host messages that arrived before the form registered its listeners. */
  const queue = [];
  let epoch = boot.epoch;

  /**
   * Posts one envelope to the host.
   * @param {object} envelope kind, method, payload
   */
  const post = (envelope) => {
    const text = JSON.stringify({ version: 1, epoch, windowId: boot.windowId, ...envelope });
    if (window.ipc) window.ipc.postMessage(text);
    else queueMicrotask(() => window.ipc.postMessage(text));
  };

  /**
   * Merges property values into the state.
   * @param {Record<string, Record<string, unknown>>} controls
   */
  const merge = (controls) => {
    for (const [name, props] of Object.entries(controls)) state[name] = Object.assign(state[name] ?? {}, props);
  };

  /** @type {HTMLElement | null} */
  let overlay = null;
  /**
   * Shows or clears the dev overlay (debug shell only).
   * @param {string | null} text
   * @param {string} tone 'error' | 'info'
   */
  const show = (text, tone = 'error') => {
    if (!debug) return;
    if (text === null) {
      overlay?.remove();
      overlay = null;
      return;
    }
    if (!overlay) {
      overlay = document.createElement('pre');
      overlay.setAttribute('role', 'alert');
      overlay.style.cssText =
        'position:fixed;inset:auto 12px 12px 12px;max-height:45vh;overflow:auto;margin:0;padding:12px 14px;' +
        'font:12px/1.45 ui-monospace,Menlo,monospace;white-space:pre-wrap;border-radius:8px;z-index:2147483647;' +
        'box-shadow:0 8px 30px rgba(0,0,0,.35)';
      (document.body ?? document.documentElement).appendChild(overlay);
    }
    overlay.style.background = tone === 'error' ? '#2b0f12' : '#0f1d2b';
    overlay.style.color = tone === 'error' ? '#ffd7d9' : '#d6ecff';
    overlay.textContent = text;
  };

  /**
   * Applies one host envelope.
   * @param {{kind: string, method: string, payload: any}} envelope
   */
  const apply = (envelope) => {
    const { kind, method, payload } = envelope;
    if (kind === 'hello' || kind === 'update') {
      // The host's hello carries the current backend epoch; events are stamped with it.
      if (kind === 'hello') epoch = envelope.epoch;
      const controls = payload.controls ?? {};
      merge(controls);
      updateListeners.forEach((fn) => fn(controls));
      return;
    }
    if (kind === 'error') {
      console.warn('[zebra] host rejected a message:', payload.message);
      return;
    }
    if (kind !== 'dev') return;
    switch (method) {
      case 'backend.epoch':
        epoch = payload.epoch;
        break;
      case 'dev.refresh':
        refreshListeners.forEach((fn) => fn(payload.stamp));
        break;
      case 'dev.css':
        document.querySelectorAll('link[rel="stylesheet"]').forEach((link) => {
          const url = new URL(link.href);
          url.searchParams.set('t', String(payload.stamp));
          link.href = url.toString();
        });
        break;
      case 'dev.error':
        show(payload.errors.map((e) => (e.file ? `${e.file}\n${e.message}` : e.message)).join('\n\n'));
        break;
      case 'dev.ready':
        show(null);
        break;
      case 'dev.reload':
        location.reload();
        break;
      case 'dev.restart':
        show('Restarting the application…', 'info');
        break;
    }
  };

  /**
   * Entry point the host calls through evaluate_script.
   * @param {object} envelope
   */
  window.__zebraReceive = (envelope) => {
    if (envelope.kind === 'dev' || updateListeners.size > 0) apply(envelope);
    else {
      if (envelope.kind === 'hello') epoch = envelope.epoch;
      if (envelope.kind === 'hello' || envelope.kind === 'update') merge(envelope.payload.controls ?? {});
      queue.push(envelope);
    }
  };

  if (debug) {
    window.addEventListener('error', (e) => show(`${e.message}\n${e.error?.stack ?? ''}`));
    window.addEventListener('unhandledrejection', (e) => show(String(e.reason?.stack ?? e.reason)));
  }

  window.zebra = Object.freeze({
    windowId: boot.windowId,
    mode: boot.mode,
    state,
    get epoch() { return epoch; },
    /**
     * Sends a control event.
     * @param {string} control control name
     * @param {string} event event name, e.g. onClick
     * @param {Record<string, unknown>} [props] values the event changed
     * @param {unknown} [args] event arguments
     */
    emit(control, event, props, args) {
      if (props) merge({ [control]: props });
      post({ kind: 'event', method: `${control}.${event}`, payload: { props: props ?? {}, args } });
    },
    /**
     * Subscribes to property updates; queued updates are delivered once the first
     * listener registers.
     * @param {(controls: Record<string, Record<string, unknown>>) => void} fn
     * @returns {() => void} unsubscribe
     */
    onUpdate(fn) {
      updateListeners.add(fn);
      if (updateListeners.size === 1) queue.splice(0).forEach(apply);
      return () => updateListeners.delete(fn);
    },
    /**
     * Subscribes to dev.refresh of this form.
     * @param {(stamp: number) => void} fn
     * @returns {() => void} unsubscribe
     */
    onRefresh(fn) {
      refreshListeners.add(fn);
      return () => refreshListeners.delete(fn);
    },
  });

  post({ kind: 'hello', method: 'session.start', payload: {} });
})();
