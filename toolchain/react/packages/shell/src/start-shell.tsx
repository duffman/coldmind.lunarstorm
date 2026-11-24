/**
 * @module @coldmind/devtools-shell — the parent of an app on the devtools dev server.
 *
 * The shell owns the page: the one WebSocket to the server (HMR frames and app channels), the
 * React root with an error boundary around the app, and an overlay in a shadow root that shows
 * build, load, render, runtime and hot-update errors. The server detects the package and serves
 * its HMR runtime as `/@devtools/client.js`, so no second socket is opened.
 */

import { useEffect, useMemo, useRef, useSyncExternalStore, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";

import { ShellApp, type AppModule } from "./app-loader.tsx";
import { ShellErrors, toShellError } from "./error-store.ts";
import { attachHotHost, handleHotFrame } from "@coldmind/devtools-shell/hot";
import { ShellOverlay } from "./error-overlay.tsx";
import { ShellTerminal } from "./terminal.tsx";
import { ShellTransport, type ConnectionStatus, type ShellChannel, type TransportOptions } from "./transport.ts";

export type { AppModule } from "./app-loader.tsx";
export { ShellBoundary, type ShellBoundaryProps } from "./error-boundary.tsx";
export { BLOCKING_KINDS, ShellErrors, type ShellError, type ShellErrorInput, type ShellErrorKind } from "./error-store.ts";
export type { HmrUpdate, ServerErrorPayload, ServerFrame } from "./protocol.ts";
export { ShellTransport, defaultSocketUrl, type ConnectionStatus, type ShellChannel, type TransportOptions } from "./transport.ts";

/** Options of {@link startShell}. */
export interface ShellOptions {
  /** Element the app renders into. */
  root: Element;
  /** Imports the app entry; its default export is the root component. */
  load: () => Promise<AppModule>;
  /** An existing transport, or options for a new one. */
  transport?: ShellTransport | TransportOptions;
  /** Show the error overlay, the connection launcher and its terminal. Default `true`. */
  overlay?: boolean;
  /** Report uncaught errors and unhandled rejections from `window`. Default `true`. */
  catchWindowErrors?: boolean;
  /** Rendered in place of the app after a render error. Default: nothing (the overlay explains). */
  fallback?: (error: Error, retry: () => void) => ReactNode;
  /** Dev base the app is served under. Default `/`. */
  base?: string;
  /** Page reload. Default `location.reload()`. */
  reload?: () => void;
  /** Module import used for hot updates. Default: the browser's `import()`. */
  importModule?: (url: string) => Promise<unknown>;
}

/** A running shell. */
export interface ShellHandle {
  readonly transport: ShellTransport;
  readonly errors: ShellErrors;
  /** An app channel on the shared socket. */
  channel<T = unknown>(name: string): ShellChannel<T>;
  /** Unmounts the app and the overlay, detaches listeners and closes the socket. */
  stop(): void;
}

let current: ShellHandle | null = null;

/**
 * Starts the shell: opens the socket, attaches the HMR runtime, mounts the app inside the error
 * boundary and the overlay in a shadow root. Only one shell runs per page; starting a second one
 * stops the first.
 *
 * Recovery: a hot update re-mounts an app that failed to render; a server error clears on the
 * next successful update; an app that failed to load reloads the page on the next update.
 *
 * @example
 * startShell({ root: document.getElementById("root")!, load: () => import("./App.tsx") });
 */
export function startShell(options: ShellOptions): ShellHandle {
  current?.stop();
  const transport = options.transport instanceof ShellTransport ? options.transport : new ShellTransport(options.transport);
  const errors = new ShellErrors();
  const reload = options.reload ?? (() => location.reload());
  const cleanups: Array<() => void> = [];

  let generation = 0;
  const generationListeners = new Set<() => void>();
  let loadFailed = false;
  const onLoadFailed = () => {
    loadFailed = true;
  };

  cleanups.push(
    attachHotHost({
      send: (event, data) => transport.sendNow(event, data),
      reload,
      importModule: options.importModule ?? ((url) => import(/* @vite-ignore */ url)),
      beforeUpdate: () => (loadFailed ? "reload" : "apply"),
      updated: () => {
        errors.clear("server", "hmr");
        generation++;
        for (const listener of [...generationListeners]) listener();
      },
      updateFailed: (path, error) => errors.report({ ...toShellError("hmr", error), location: path }),
      serverError: (err) => {
        const loc = err.loc?.file ? `${err.loc.file}${err.loc.line ? `:${err.loc.line}${err.loc.column ? `:${err.loc.column}` : ""}` : ""}` : err.id;
        errors.report({ kind: "server", message: err.message, stack: err.stack || undefined, frame: err.frame, location: loc });
      },
      pagePath: () => decodeURI(location.pathname),
      base: options.base ?? "/",
    }),
  );
  cleanups.push(transport.onFrame((frame) => void handleHotFrame(frame)));

  if (options.catchWindowErrors ?? true) {
    const onError = (event: ErrorEvent) => errors.report(toShellError("runtime", event.error ?? event.message));
    const onRejection = (event: PromiseRejectionEvent) => errors.report(toShellError("runtime", event.reason));
    window.addEventListener("error", onError);
    window.addEventListener("unhandledrejection", onRejection);
    cleanups.push(() => {
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onRejection);
    });
  }

  const appRoot = createRoot(options.root);
  appRoot.render(
    <ShellApp
      load={options.load}
      errors={errors}
      subscribeGeneration={(listener) => {
        generationListeners.add(listener);
        return () => generationListeners.delete(listener);
      }}
      getGeneration={() => generation}
      onLoadFailed={onLoadFailed}
      fallback={options.fallback}
    />,
  );

  let overlayRoot: Root | null = null;
  let overlayHost: HTMLElement | null = null;
  if (options.overlay ?? true) {
    overlayHost = document.createElement("div");
    overlayHost.setAttribute("data-devtools-shell", "");
    document.body.appendChild(overlayHost);
    overlayRoot = createRoot(overlayHost.attachShadow({ mode: "open" }));
    overlayRoot.render(
      <>
        <ShellOverlay errors={errors} subscribeStatus={(listener) => transport.onStatus(listener)} getStatus={() => transport.status} />
        <ShellTerminal transport={transport} errors={errors} />
      </>,
    );
  }

  transport.start();

  const handle: ShellHandle = {
    transport,
    errors,
    channel: (name) => transport.channel(name),
    stop() {
      for (const cleanup of cleanups.splice(0)) cleanup();
      appRoot.unmount();
      overlayRoot?.unmount();
      overlayHost?.remove();
      transport.close();
      if (current === handle) current = null;
    },
  };
  current = handle;
  return handle;
}

/**
 * The running shell.
 * @throws {Error} When {@link startShell} has not been called on this page.
 */
export function getShell(): ShellHandle {
  if (!current) throw new Error("devtools shell is not running; call startShell() first");
  return current;
}

/**
 * Subscribes a component to an app channel on the shell's socket.
 * @param name - Channel name; it travels as the event `app:<name>`.
 * @param onMessage - Called with every message; the latest function is used without resubscribing.
 * @returns A stable `send` function for the channel.
 * @example
 * const send = useShellChannel<string>("chat", (text) => setLog((log) => [...log, text]));
 */
export function useShellChannel<T = unknown>(name: string, onMessage?: (data: T) => void): (data: T) => boolean {
  const channel = useMemo(() => getShell().channel<T>(name), [name]);
  const handler = useRef(onMessage);
  handler.current = onMessage;
  useEffect(() => channel.subscribe((data) => handler.current?.(data)), [channel]);
  return channel.send;
}

/** The shell socket's connection status, re-rendering on change. */
export function useShellStatus(): ConnectionStatus {
  const { transport } = getShell();
  return useSyncExternalStore(
    (listener) => transport.onStatus(listener),
    () => transport.status,
  );
}
