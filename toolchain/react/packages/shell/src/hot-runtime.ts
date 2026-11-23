/**
 * @module @coldmind/devermind-shell/hot — the HMR runtime behind `import.meta.hot`.
 *
 * The devermind server serves `/@devermind/client.js` as a re-export of this module when the app
 * depends on the shell, so the hot contexts the server injects into modules and the shell share
 * one registry. This module never opens a socket: the shell attaches a {@link HotHost} that owns
 * the connection and the page's error UI.
 */

import type { HmrUpdate, ServerErrorPayload, ServerFrame } from "./protocol.ts";

/** What the HMR runtime needs from the shell. */
export interface HotHost {
  /** Sends a `custom` event now; `false` when the socket is not open. */
  send(event: string, data: unknown): boolean;
  /** Reloads the page. */
  reload(): void;
  /** Imports a module URL; tests replace the browser's `import()`. */
  importModule(url: string): Promise<unknown>;
  /**
   * Called before an `update` frame is applied. `"reload"` aborts it and reloads the page: the
   * app never finished loading, so its boundaries were never registered.
   */
  beforeUpdate(): "apply" | "reload";
  /** A boundary was hot-updated. */
  updated(update: HmrUpdate): void;
  /** A boundary failed to update (the new module threw, or a fetch failed). */
  updateFailed(path: string, error: unknown): void;
  /** The server reported a compile error. */
  serverError(error: ServerErrorPayload): void;
  /** Decoded path of the current page (`location.pathname`). */
  pagePath(): string;
  /** Dev base the app is served under, ending in `/`. */
  base: string;
}

/** The `import.meta.hot` object of one module (Vite's `ViteHotContext`). */
export interface HotContext {
  /** Survives updates of the module: state handed from the old instance to the new one. */
  readonly data: Record<string, unknown>;
  accept(): void;
  accept(callback: (next: unknown) => void): void;
  accept(dep: string, callback?: (next: unknown) => void): void;
  accept(deps: readonly string[], callback?: (next: unknown[]) => void): void;
  acceptExports(names: string | readonly string[], callback?: (next: unknown) => void): void;
  dispose(callback: (data: Record<string, unknown>) => void | Promise<void>): void;
  prune(callback: (data: Record<string, unknown>) => void | Promise<void>): void;
  decline(): void;
  /** Rejects the current update; the server moves it to this module's importers. */
  invalidate(message?: string): void;
  on(event: string, callback: (data: unknown) => void): void;
  off(event: string, callback: (data: unknown) => void): void;
  send(event: string, data?: unknown): void;
}

interface AcceptCallback {
  deps: string[];
  fn: (next: any) => unknown;
  single?: boolean;
}

interface HotModule {
  acceptCallbacks: AcceptCallback[];
  disposeCallbacks: Array<(data: Record<string, unknown>) => unknown>;
  pruneCallbacks: Array<(data: Record<string, unknown>) => unknown>;
  data: Record<string, unknown>;
  listeners: Array<[string, (data: unknown) => void]>;
}

const hotModules = new Map<string, HotModule>();
const customListeners = new Map<string, Set<(data: unknown) => void>>();
const styleTags = new Map<string, HTMLStyleElement>();
let host: HotHost | null = null;
let currentFirstInvalidatedBy: string | undefined;
let updateChain: Promise<void> = Promise.resolve();

const stripQuery = (url: string) => url.split("?")[0]!;

/**
 * Attaches the shell. Only one host is attached at a time.
 * @returns A function that detaches `next` again.
 */
export function attachHotHost(next: HotHost): () => void {
  host = next;
  return () => {
    if (host === next) host = null;
  };
}

/** Emits a hot event (`vite:beforeUpdate`, a server `custom` event) to `import.meta.hot.on` listeners. */
export function emitHotEvent(event: string, data: unknown): void {
  const set = customListeners.get(event);
  if (set) for (const listener of [...set]) listener(data);
}

/**
 * The `import.meta.hot` of the module `ownerId`. The server injects the call; a re-executed
 * module gets the same `data` and a fresh set of callbacks.
 */
export function createHotContext(ownerId: string): HotContext {
  const id = stripQuery(ownerId);
  let mod = hotModules.get(id);
  if (mod) {
    mod.acceptCallbacks = [];
    mod.disposeCallbacks = [];
    mod.pruneCallbacks = [];
    for (const [event, listener] of mod.listeners) customListeners.get(event)?.delete(listener);
    mod.listeners = [];
  } else {
    mod = { acceptCallbacks: [], disposeCallbacks: [], pruneCallbacks: [], data: {}, listeners: [] };
    hotModules.set(id, mod);
  }
  const owner = mod;
  return {
    data: owner.data,
    accept(first?: unknown, second?: unknown) {
      if (first === undefined || typeof first === "function") {
        owner.acceptCallbacks.push({ deps: [id], fn: (first as AcceptCallback["fn"]) ?? (() => {}) });
        return;
      }
      const deps = (Array.isArray(first) ? first : [first]).map((dep) => stripQuery(String(dep)));
      owner.acceptCallbacks.push({
        deps,
        fn: (second as AcceptCallback["fn"]) ?? (() => {}),
        single: typeof first === "string",
      });
    },
    acceptExports(_names, callback) {
      owner.acceptCallbacks.push({ deps: [id], fn: callback ?? (() => {}) });
    },
    dispose(callback) {
      owner.disposeCallbacks.push(callback);
    },
    prune(callback) {
      owner.pruneCallbacks.push(callback);
    },
    decline() {},
    invalidate(message) {
      // The module that started the chain travels along, so the server can tell an invalidation
      // that came back around and reload instead of ping-ponging updates.
      const data = { path: id, message, firstInvalidatedBy: currentFirstInvalidatedBy ?? id };
      emitHotEvent("vite:invalidate", data);
      console.warn(`[devermind] ${id} invalidated${message ? ": " + message : ""}`);
      if (!host?.send("vite:invalidate", data)) reload();
    },
    on(event, callback) {
      let set = customListeners.get(event);
      if (!set) customListeners.set(event, (set = new Set()));
      set.add(callback);
      owner.listeners.push([event, callback]);
    },
    off(event, callback) {
      customListeners.get(event)?.delete(callback);
    },
    send(event, data) {
      host?.send(event, data);
    },
  } as HotContext;
}

/** Writes the CSS of a JS-imported stylesheet into its `<style>` tag (created on first use). */
export function updateStyle(id: string, css: string): void {
  let tag = styleTags.get(id);
  if (!tag) {
    tag = document.createElement("style");
    tag.setAttribute("data-devermind-id", id);
    document.head.appendChild(tag);
    styleTags.set(id, tag);
  }
  tag.textContent = css;
}

/** Removes the `<style>` tag of a pruned stylesheet. */
export function removeStyle(id: string): void {
  styleTags.get(id)?.remove();
  styleTags.delete(id);
}

/**
 * Applies one server frame: queues `update` entries, reloads on `full-reload` for this page,
 * prunes, and hands errors and `custom` events on.
 * @returns A promise that settles once the updates this frame queued have run.
 */
export function handleHotFrame(frame: ServerFrame): Promise<void> {
  switch (frame.type) {
    case "update": {
      if (host?.beforeUpdate() === "reload") {
        reload();
        return Promise.resolve();
      }
      for (const update of frame.updates ?? []) {
        if (update.type === "css-update") swapCss(update);
        else queueUpdate(update);
      }
      return updateChain;
    }
    case "full-reload":
      emitHotEvent("vite:beforeFullReload", frame);
      if (!reloadsThisPage(frame.path)) {
        console.log(`[devermind] ${frame.path} changed, not this page`);
        return Promise.resolve();
      }
      console.log("[devermind] full reload:", frame.reason ?? frame.path ?? "");
      reload();
      return Promise.resolve();
    case "prune":
      emitHotEvent("vite:beforePrune", frame);
      return prunePaths(frame.paths ?? []);
    case "error": {
      const err = frame.err ?? { message: frame.message ?? "unknown server error" };
      emitHotEvent("vite:error", { err });
      host?.serverError(err);
      return Promise.resolve();
    }
    case "custom":
      emitHotEvent(frame.event, frame.data);
      return Promise.resolve();
    default:
      return Promise.resolve();
  }
}

/** Whether a module URL has registered a hot context in this page. */
export function isHotModuleLoaded(url: string): boolean {
  return hotModules.has(stripQuery(url));
}

/** Clears every registration. For tests. */
export function resetHotRuntime(): void {
  hotModules.clear();
  customListeners.clear();
  for (const tag of styleTags.values()) tag.remove();
  styleTags.clear();
  host = null;
  updateChain = Promise.resolve();
}

function reload(): void {
  if (host) host.reload();
  else location.reload();
}

// An edited page reloads only the tabs showing it; index.html is the SPA shell behind every
// route, so it always reloads.
function reloadsThisPage(path: string | undefined): boolean {
  if (!path || !path.endsWith(".html") || path === "/index.html" || !host) return true;
  const page = host.pagePath();
  const payload = host.base + path.slice(1);
  return page === payload || (page.endsWith("/") && page + "index.html" === payload);
}

function queueUpdate(update: HmrUpdate): void {
  updateChain = updateChain.then(() => applyUpdate(update)).catch(() => {});
}

/**
 * Applies one queued boundary update without racing later frames. Dispose handlers run before
 * the timestamped import; accept handlers are captured before re-execution replaces them.
 * Failed updates reach the host overlay and server diagnostics instead of rejecting the queue.
 */
async function applyUpdate(update: HmrUpdate): Promise<void> {
  const boundary = stripQuery(update.path);
  const acceptedUrl = update.acceptedPath ?? update.path;
  const accepted = stripQuery(acceptedUrl);
  const mod = hotModules.get(boundary);
  if (!mod) {
    // In the server's graph, but this page never loaded it (a lazy route): nothing to swap.
    console.debug(`[devermind] ${boundary} is not loaded here, ignoring update`);
    return;
  }
  const isSelf = accepted === boundary;
  const accepts = mod.acceptCallbacks.filter((cb) => cb.deps.includes(accepted));
  if (accepts.length === 0) {
    console.log(`[devermind] ${boundary} has no accept handler for ${accepted}, reloading`);
    reload();
    return;
  }
  const disposed = hotModules.get(accepted);
  const disposes = disposed ? disposed.disposeCallbacks.slice() : [];
  emitHotEvent("vite:beforeUpdate", { type: "update", updates: [update] });
  try {
    for (const dispose of disposes) await dispose(disposed!.data);
    const url = acceptedUrl + (acceptedUrl.includes("?") ? "&" : "?") + "t=" + update.timestamp;
    let next: unknown;
    try {
      next = host ? await host.importModule(url) : await import(/* @vite-ignore */ url);
    } catch (error) {
      if (update.isWithinCircularImport) {
        // An accepted module inside an import loop cannot recover its execution order.
        console.info(`[devermind] ${accepted} failed to apply HMR within a circular import, reloading`);
        reload();
        return;
      }
      throw error;
    }
    try {
      currentFirstInvalidatedBy = update.firstInvalidatedBy;
      for (const cb of accepts) {
        if (isSelf || cb.single) await cb.fn(next);
        else await cb.fn(cb.deps.map((dep) => (dep === accepted ? next : undefined)));
      }
    } finally {
      currentFirstInvalidatedBy = undefined;
    }
    emitHotEvent("vite:afterUpdate", { type: "update", updates: [update] });
    host?.updated(update);
    console.log(`[devermind] hot updated ${update.path}`);
  } catch (error) {
    emitHotEvent("vite:error", { err: { message: String(error) } });
    host?.updateFailed(update.path, error);
    host?.send("oj:hmr-result", { ok: false, path: update.path, error: String(error).slice(0, 4096) });
  }
}

/** Runs module disposal before prune callbacks, preserving each module's hot-data handoff. */
async function prunePaths(paths: readonly string[]): Promise<void> {
  for (const path of paths) {
    const mod = hotModules.get(stripQuery(path));
    if (!mod) continue;
    for (const dispose of mod.disposeCallbacks) await dispose(mod.data);
    for (const prune of mod.pruneCallbacks) await prune(mod.data);
  }
}

/**
 * Replaces linked CSS only after its replacement loads, avoiding an unstyled intermediate page.
 * JS-imported styles instead rerun their wrapper; an unknown stylesheet requires a full reload.
 */
function swapCss(update: HmrUpdate): void {
  const links = [...document.querySelectorAll<HTMLLinkElement>("link[rel=stylesheet]")];
  const link = links.find((l) => new URL(l.href, location.href).pathname === update.path);
  if (link) {
    const next = link.cloneNode() as HTMLLinkElement;
    next.href = update.path + "?t=" + update.timestamp;
    next.addEventListener("load", () => link.remove());
    link.after(next);
    return;
  }
  // A stylesheet imported from JS lives in a <style> its module wrapper wrote; re-running the
  // wrapper swaps it in place instead of reloading.
  if (styleTags.has(update.path)) {
    const url = update.path + "?import&t=" + update.timestamp;
    (host ? host.importModule(url) : import(/* @vite-ignore */ url)).catch((error: unknown) => {
      host?.updateFailed(update.path, error);
      reload();
    });
    return;
  }
  reload();
}
