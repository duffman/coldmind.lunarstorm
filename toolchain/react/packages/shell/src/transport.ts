/** @module @coldmind/devermind-shell/transport — the page's single WebSocket to the devermind server. */

import { APP_EVENT_PREFIX, HMR_PATH, parseFrame, type ServerFrame } from "./protocol.ts";

/**
 * Connection state.
 * - `connecting`: first connection not established yet (retries on an interval).
 * - `open`: connected.
 * - `lost`: was connected, the server went away; polling until it answers, then the page reloads.
 * - `closed`: closed by {@link ShellTransport.close}.
 */
export type ConnectionStatus = "connecting" | "open" | "lost" | "closed";

/** A typed app channel on the shared socket. */
export interface ShellChannel<T = unknown> {
  /** Name of the channel; it travels as the event `app:<name>`. */
  readonly name: string;
  /**
   * Sends `data` to the server. Queued while the socket is not open.
   * @returns `false` when the queue is full and the message was dropped.
   */
  send(data: T): boolean;
  /** Calls `listener` for every message on this channel. Returns the unsubscribe function. */
  subscribe(listener: (data: T) => void): () => void;
}

/** Collaborators the transport needs from the browser; tests replace them. */
export interface TransportOptions {
  /** Socket URL; defaults to `ws(s)://<page host>/__devermind_hmr`. */
  url?: string;
  /** WebSocket constructor; defaults to the global one. */
  WebSocket?: new (url: string) => WebSocket;
  /** Restart probe; defaults to the global `fetch`. */
  fetch?: (url: string, init?: RequestInit) => Promise<unknown>;
  /** Reload after a server restart; defaults to `location.reload()`. */
  reload?: () => void;
  /** Delay between connection attempts and restart probes, in ms. Default 1000. */
  retryMs?: number;
  /** Maximum queued outgoing messages while not open. Default 100. */
  maxQueue?: number;
}

/** The default socket URL for the current page. */
export function defaultSocketUrl(loc: Pick<Location, "protocol" | "host"> = location): string {
  return `${loc.protocol === "https:" ? "wss" : "ws"}://${loc.host}${HMR_PATH}`;
}

/**
 * Owns the page's one connection to the dev server. Every frame goes to {@link ShellTransport.onFrame}
 * listeners (the HMR runtime); `custom` frames also go to {@link ShellTransport.on} listeners of
 * their event, which is how app channels share the socket.
 *
 * After a connection that was open closes, the server restarted (config change) or stopped: the
 * transport probes the socket URL over HTTP until it answers, then reloads the page, because the
 * page's modules belong to the old server. A first connection that fails is retried instead.
 *
 * @example
 * const transport = new ShellTransport();
 * transport.onFrame((frame) => console.log(frame.type));
 * transport.start();
 * transport.channel<{ text: string }>("chat").send({ text: "hi" });
 */
export class ShellTransport {
  readonly url: string;
  #options: Required<Omit<TransportOptions, "url">>;
  #socket: WebSocket | null = null;
  #status: ConnectionStatus = "connecting";
  #hadConnection = false;
  #queue: string[] = [];
  #retry: ReturnType<typeof setTimeout> | null = null;
  #frameListeners = new Set<(frame: ServerFrame) => void>();
  #statusListeners = new Set<(status: ConnectionStatus) => void>();
  #eventListeners = new Map<string, Set<(data: unknown) => void>>();

  constructor(options: TransportOptions = {}) {
    this.url = options.url ?? defaultSocketUrl();
    this.#options = {
      WebSocket: options.WebSocket ?? globalThis.WebSocket,
      fetch: options.fetch ?? ((url, init) => globalThis.fetch(url, init)),
      reload: options.reload ?? (() => location.reload()),
      retryMs: options.retryMs ?? 1000,
      maxQueue: options.maxQueue ?? 100,
    };
  }

  /** Current connection state. */
  get status(): ConnectionStatus {
    return this.#status;
  }

  /** Opens the socket. Calling it again while a socket exists does nothing. */
  start(): void {
    if (this.#socket || this.#status === "closed") return;
    const socket = new this.#options.WebSocket(this.url);
    this.#socket = socket;
    socket.addEventListener("open", () => {
      this.#hadConnection = true;
      this.#setStatus("open");
      for (const frame of this.#queue.splice(0)) socket.send(frame);
    });
    socket.addEventListener("message", (event) => {
      const frame = parseFrame((event as MessageEvent).data);
      if (frame) this.#dispatch(frame);
    });
    socket.addEventListener("close", () => {
      if (this.#socket !== socket) return;
      this.#socket = null;
      if (this.#status === "closed") return;
      if (this.#hadConnection) {
        this.#setStatus("lost");
        void this.#waitForRestart();
      } else {
        this.#retry = setTimeout(() => {
          this.#retry = null;
          this.start();
        }, this.#options.retryMs);
      }
    });
  }

  /** Closes the socket and stops retrying. Listeners stay registered but receive nothing more. */
  close(): void {
    this.#setStatus("closed");
    if (this.#retry) clearTimeout(this.#retry);
    this.#retry = null;
    const socket = this.#socket;
    this.#socket = null;
    socket?.close();
  }

  /** Calls `listener` with every frame. Returns the unsubscribe function. */
  onFrame(listener: (frame: ServerFrame) => void): () => void {
    this.#frameListeners.add(listener);
    return () => this.#frameListeners.delete(listener);
  }

  /** Calls `listener` on every status change. Returns the unsubscribe function. */
  onStatus(listener: (status: ConnectionStatus) => void): () => void {
    this.#statusListeners.add(listener);
    return () => this.#statusListeners.delete(listener);
  }

  /** Calls `listener` with the data of every `custom` frame named `event`. Returns the unsubscribe function. */
  on(event: string, listener: (data: unknown) => void): () => void {
    let set = this.#eventListeners.get(event);
    if (!set) this.#eventListeners.set(event, (set = new Set()));
    set.add(listener);
    return () => set.delete(listener);
  }

  /**
   * Sends a `custom` frame. Queued while the socket is not open; dropped once the queue holds
   * `maxQueue` frames.
   * @returns Whether the frame was sent or queued.
   */
  send(event: string, data: unknown): boolean {
    const frame = JSON.stringify({ type: "custom", event, data });
    if (this.#socket && this.#status === "open") {
      this.#socket.send(frame);
      return true;
    }
    if (this.#status === "closed" || this.#queue.length >= this.#options.maxQueue) return false;
    this.#queue.push(frame);
    return true;
  }

  /** Sends now if open, else reports `false` without queueing (HMR messages must not arrive late). */
  sendNow(event: string, data: unknown): boolean {
    if (!this.#socket || this.#status !== "open") return false;
    this.#socket.send(JSON.stringify({ type: "custom", event, data }));
    return true;
  }

  /** An app channel: messages travel as the `custom` event `app:<name>`. */
  channel<T = unknown>(name: string): ShellChannel<T> {
    const event = APP_EVENT_PREFIX + name;
    return {
      name,
      send: (data) => this.send(event, data),
      subscribe: (listener) => this.on(event, listener as (data: unknown) => void),
    };
  }

  #dispatch(frame: ServerFrame): void {
    for (const listener of [...this.#frameListeners]) listener(frame);
    if (frame.type !== "custom") return;
    const listeners = this.#eventListeners.get(frame.event);
    if (listeners) for (const listener of [...listeners]) listener(frame.data);
  }

  #setStatus(status: ConnectionStatus): void {
    if (this.#status === status) return;
    this.#status = status;
    for (const listener of [...this.#statusListeners]) listener(status);
  }

  /**
   * Probes the HTTP equivalent of the lost socket endpoint, then reloads the old module graph.
   * A resolved response establishes reachability, not application health. Closing the transport
   * prevents further probes/reloads; an already-running fetch is not aborted.
   */
  async #waitForRestart(): Promise<void> {
    const probe = this.url.replace(/^ws/, "http").split("?")[0]!;
    while (this.#status === "lost") {
      try {
        await this.#options.fetch(probe, { mode: "no-cors", headers: { Accept: "text/x-vite-ping" } });
        if (this.#status === "lost") this.#options.reload();
        return;
      } catch {
        await new Promise((resolve) => setTimeout(resolve, this.#options.retryMs));
      }
    }
  }
}
