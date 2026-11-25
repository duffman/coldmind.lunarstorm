/**
 * Captures what the console tab shows: `console.*` calls, uncaught errors, unhandled rejections, and
 * `fetch`/`XMLHttpRequest` traffic. Every patch forwards to the original, so the browser devtools still see it all.
 *
 * Decision: one module-level store (`capture`) rather than React state, so logging starts before the app renders
 * (`installCapture()` in the entry file) and survives the panel being closed.
 */

/** What an entry is; console levels map 1:1, `net` is a request, `input`/`result` are REPL lines. */
export type EntryType = "log" | "info" | "warn" | "error" | "debug" | "net" | "input" | "result";

/** A finished network request seen by the fetch/XHR patches. `status` is 0 when it failed before a response. */
export interface NetRecord {
  method: string;
  url: string;
  status: number;
  ms: number;
  requestHeaders: [string, string][];
  requestBody?: string;
  via: "fetch" | "xhr";
  error?: string;
}

/** One console line. `args` are the live values passed to `console.*`, rendered lazily by `Value`. */
export interface LogEntry {
  id: number;
  time: number;
  type: EntryType;
  args: unknown[];
  net?: NetRecord;
}

/** Console methods that are patched. */
const LEVELS = ["log", "info", "warn", "error", "debug"] as const;

/** Entries kept; older ones are dropped first. */
export const MAX_ENTRIES = 1000;

/** Request headers as pairs, from any `HeadersInit`. */
function headerPairs(h: HeadersInit | undefined): [string, string][] {
  if (!h) return [];
  if (h instanceof Headers) return [...h.entries()];
  return Array.isArray(h) ? h.map(([k, v]) => [k, v] as [string, string]) : Object.entries(h);
}

/** The log store: append, subscribe, clear, and install/uninstall the global patches. */
export class Capture {
  private list: LogEntry[] = [];
  private pending: LogEntry[] = [];
  private listeners = new Set<() => void>();
  private nextId = 1;
  private scheduled = false;
  private undo: (() => void)[] = [];

  /** Current entries, oldest first. The array is replaced on change, so it works as a `useSyncExternalStore` snapshot. */
  readonly snapshot = (): LogEntry[] => this.list;

  /**
   * Registers a change listener.
   * @param fn called after entries change
   * @return unsubscribe
   */
  readonly subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  /**
   * Appends an entry.
   * Decision: listeners are told in a microtask, batched. React logs from inside render (warnings, caught errors);
   * a synchronous setState there would warn "Cannot update a component while rendering a different component".
   * @param type entry type
   * @param args values to show
   * @param net request details for `net` entries
   */
  push(type: EntryType, args: unknown[], net?: NetRecord): void {
    this.pending.push({ id: this.nextId++, time: Date.now(), type, args, net });
    if (this.scheduled) return;
    this.scheduled = true;
    queueMicrotask(() => this.flush());
  }

  /** Applies pending entries now and notifies. Tests call it to skip the microtask. */
  flush(): void {
    this.scheduled = false;
    if (!this.pending.length) return;
    const next = this.list.concat(this.pending);
    this.pending = [];
    this.list = next.length > MAX_ENTRIES ? next.slice(next.length - MAX_ENTRIES) : next;
    for (const fn of this.listeners) fn();
  }

  /** Removes every entry. */
  clear(): void {
    this.pending = [];
    this.list = [];
    for (const fn of this.listeners) fn();
  }

  /** True while the global patches are in place. */
  get installed(): boolean {
    return this.undo.length > 0;
  }

  /**
   * Patches `console`, `fetch`, `XMLHttpRequest` and the error events on `target`. Calling it again is a no-op.
   * @param target the global object to patch
   */
  install(target: typeof globalThis = globalThis): void {
    if (this.installed) return;
    const con = target.console;
    for (const level of LEVELS) {
      const orig = con[level];
      const patched = (...args: unknown[]) => {
        orig.apply(con, args);
        this.push(level, args);
      };
      con[level] = patched;
      this.undo.push(() => {
        if (con[level] === patched) con[level] = orig;
      });
    }

    if (typeof target.addEventListener === "function") {
      const onError = (e: ErrorEvent) =>
        this.push("error", e.error !== undefined && e.error !== null ? ["Uncaught", e.error] : [`Uncaught ${e.message}`, `at ${e.filename}:${e.lineno}:${e.colno}`]);
      const onRejection = (e: PromiseRejectionEvent) => this.push("error", ["Unhandled rejection:", e.reason]);
      target.addEventListener("error", onError);
      target.addEventListener("unhandledrejection", onRejection);
      this.undo.push(() => {
        target.removeEventListener("error", onError);
        target.removeEventListener("unhandledrejection", onRejection);
      });
    }

    const origFetch = target.fetch;
    if (typeof origFetch === "function") {
      const patched: typeof fetch = async (input, init) => {
        const req = input instanceof Request ? input : undefined;
        const method = (init?.method ?? req?.method ?? "GET").toUpperCase();
        const url = req ? req.url : String(input);
        const record: NetRecord = {
          method, url, status: 0, ms: 0, via: "fetch",
          requestHeaders: headerPairs(init?.headers ?? req?.headers),
          requestBody: typeof init?.body === "string" ? init.body : init?.body instanceof URLSearchParams ? init.body.toString() : undefined,
        };
        const start = performance.now();
        try {
          const res = await origFetch.call(target, input, init);
          record.status = res.status;
          return res;
        } catch (e) {
          record.error = e instanceof Error ? e.message : String(e);
          throw e;
        } finally {
          record.ms = Math.round(performance.now() - start);
          this.push("net", [], record);
        }
      };
      target.fetch = patched;
      this.undo.push(() => {
        if (target.fetch === patched) target.fetch = origFetch;
      });
    }

    const Xhr = target.XMLHttpRequest;
    if (typeof Xhr === "function") {
      const { open, send, setRequestHeader } = Xhr.prototype;
      const self = this;
      type Tracked = XMLHttpRequest & { __lsdm?: NetRecord };
      Xhr.prototype.open = function (this: Tracked, method: string, url: string | URL, ...rest: unknown[]) {
        this.__lsdm = { method: method.toUpperCase(), url: String(url), status: 0, ms: 0, requestHeaders: [], via: "xhr" };
        return (open as (...a: unknown[]) => void).call(this, method, url, ...rest);
      } as typeof open;
      Xhr.prototype.setRequestHeader = function (this: Tracked, name: string, value: string) {
        this.__lsdm?.requestHeaders.push([name, value]);
        return setRequestHeader.call(this, name, value);
      };
      Xhr.prototype.send = function (this: Tracked, body?: Document | XMLHttpRequestBodyInit | null) {
        const record = this.__lsdm;
        if (record) {
          if (typeof body === "string") record.requestBody = body;
          const start = performance.now();
          this.addEventListener("loadend", () => {
            record.status = this.status;
            record.ms = Math.round(performance.now() - start);
            if (this.status === 0) record.error = "request failed";
            self.push("net", [], record);
          });
        }
        return send.call(this, body);
      };
      this.undo.push(() => Object.assign(Xhr.prototype, { open, send, setRequestHeader }));
    }
  }

  /** Restores everything `install` patched. */
  uninstall(): void {
    for (const fn of this.undo.splice(0).reverse()) fn();
  }
}

/** The shared store the dev mode panel reads. */
export const capture = new Capture();

/**
 * Starts capturing now, before the app renders, so startup logs and requests are kept. Optional: `<DevMode>`
 * installs on mount anyway.
 */
export function installCapture(): void {
  capture.install();
}
