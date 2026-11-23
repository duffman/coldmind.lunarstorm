import { createContext, useContext, useEffect, useMemo, useState, useSyncExternalStore, type ReactNode } from "react";

import { ShellBoundary } from "./error-boundary.tsx";
import { ShellErrors, toShellError } from "./error-store.ts";

/** The persistent host session shared by the shell and the application's API. */
export interface DesktopConnection {
  start(): Promise<unknown>;
  getBackendEpoch(): number;
  subscribe(method: string, listener: (payload: unknown) => void): () => void;
}

export interface DesktopShell {
  readonly generation: number;
  readonly backendEpoch: number;
}

export interface ShellWrapperProps {
  connection: DesktopConnection;
  children: ReactNode;
  /** A failed entry import needs a fresh module loader after the next update. */
  reloadOnUpdate?: () => boolean;
}

const DesktopShellContext = createContext<DesktopShell | null>(null);

/** Reads the host generation observed by the surrounding desktop shell. */
export function useDesktopShell(): DesktopShell {
  const shell = useContext(DesktopShellContext);
  if (!shell) throw new Error("useDesktopShell requires a ShellWrapper");
  return shell;
}

/** Owns host readiness, error recovery and hot updates for one desktop React root. */
export function ShellWrapper({ connection, children, reloadOnUpdate }: ShellWrapperProps) {
  const [errors] = useState(() => new ShellErrors());
  const [ready, setReady] = useState(false);
  const [generation, setGeneration] = useState(0);
  const [backendEpoch, setBackendEpoch] = useState(0);
  const shell = useMemo(() => ({ generation, backendEpoch }), [generation, backendEpoch]);

  useEffect(() => {
    let active = true;
    let updates = Promise.resolve();
    const cleanups: Array<() => void> = [];
    setReady(false);

    // Host notifications arrive in order. Keep imports, CSS swaps and the
    // following dev.ready in that same order even while a module is loading.
    const enqueue = (update: () => void | Promise<void>) => {
      updates = updates.then(async () => {
        if (active) await update();
      }).catch((error: unknown) => {
        if (active) errors.report(toShellError("hmr", error));
      });
    };
    const subscribe = (method: string, listener: (payload: unknown) => void) => {
      cleanups.push(connection.subscribe(method, listener));
    };

    subscribe("backend.epoch", (payload) => {
      if (typeof payload === "number") setBackendEpoch(payload);
    });
    subscribe("dev.refresh", (payload) => enqueue(async () => {
      const { modules } = payload as { modules: string[] };
      for (const url of modules) {
        const resource = localResource(url);
        await import(/* @vite-ignore */ resource.href);
        if (!active) return;
      }
      const refresh = (globalThis as typeof globalThis & {
        __zebraRefresh?: { performReactRefresh(): void };
      }).__zebraRefresh;
      if (!refresh) throw new Error("React Refresh is unavailable");
      refresh.performReactRefresh();
      errors.clear("server", "hmr");
      setGeneration((value) => value + 1);
    }));
    subscribe("dev.css", (payload) => enqueue(async () => {
      const { urls } = payload as { urls: string[] };
      for (const url of urls) await replaceStylesheet(localResource(url), cleanups);
      if (active) errors.clear("server", "hmr");
    }));
    subscribe("dev.reload", () => enqueue(() => location.reload()));
    subscribe("dev.error", (payload) => enqueue(() => {
      errors.report(toShellError("server", payload));
    }));
    subscribe("dev.ready", () => enqueue(() => {
      errors.clear("server");
      if (reloadOnUpdate?.()) location.reload();
    }));

    const onError = (event: ErrorEvent) => errors.report(toShellError("runtime", event.error ?? event.message));
    const onRejection = (event: PromiseRejectionEvent) => errors.report(toShellError("runtime", event.reason));
    window.addEventListener("error", onError);
    window.addEventListener("unhandledrejection", onRejection);
    cleanups.push(() => {
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onRejection);
    });

    void connection.start().then(() => {
      if (active) {
        errors.clear("load");
        setBackendEpoch(connection.getBackendEpoch());
        setReady(true);
      }
    }, (error: unknown) => {
      if (active) errors.report(toShellError("load", error));
    });

    return () => {
      active = false;
      for (const cleanup of cleanups.splice(0)) cleanup();
    };
  }, [connection, errors, reloadOnUpdate]);

  return (
    <DesktopShellContext.Provider value={shell}>
      {ready && (
        <ShellBoundary
          errors={errors}
          resetKey={generation}
          fallback={(_error, retry) => <button type="button" onClick={retry}>Retry app</button>}
        >
          {children}
        </ShellBoundary>
      )}
      <DesktopErrors errors={errors} connecting={!ready} />
    </DesktopShellContext.Provider>
  );
}

function DesktopErrors({ errors, connecting }: { errors: ShellErrors; connecting: boolean }) {
  const list = useSyncExternalStore(errors.subscribe, errors.list);
  if (!list.length) return connecting ? <p role="status">Connecting to desktop host…</p> : null;
  return (
    <section role="alert" data-desktop-shell-error="" style={{ position: "fixed", inset: 16, zIndex: 2147483647, padding: 24, background: "#241414", color: "#ffcccc", overflow: "auto" }}>
      {list.map((error) => <pre key={error.id} style={{ whiteSpace: "pre-wrap" }}>{error.message}{error.stack ? `\n${error.stack}` : ""}</pre>)}
      <button type="button" onClick={() => errors.clear()}>Dismiss</button>
    </section>
  );
}

function localResource(value: string): URL {
  const resource = new URL(value, location.href);
  if (resource.origin !== location.origin || resource.protocol !== location.protocol || resource.host !== location.host) {
    throw new Error("Invalid update origin");
  }
  return resource;
}

function replaceStylesheet(resource: URL, cleanups: Array<() => void>): Promise<void> {
  return new Promise((resolve, reject) => {
    const previous = Array.from(document.querySelectorAll<HTMLLinkElement>('link[rel="stylesheet"]'))
      .find((link) => new URL(link.href).pathname === resource.pathname);
    const replacement = document.createElement("link");
    replacement.rel = "stylesheet";
    replacement.href = resource.href;
    const finish = (error?: Error) => {
      clearTimeout(timeout);
      replacement.onload = null;
      replacement.onerror = null;
      const index = cleanups.indexOf(cancel);
      if (index >= 0) cleanups.splice(index, 1);
      if (error) {
        replacement.remove();
        reject(error);
      } else {
        previous?.remove();
        resolve();
      }
    };
    const cancel = () => finish(new Error("Stylesheet update cancelled"));
    const timeout = setTimeout(() => finish(new Error("Stylesheet update timed out")), 15000);
    cleanups.push(cancel);
    replacement.onload = () => finish();
    replacement.onerror = () => finish(new Error(`Failed to load stylesheet ${resource.pathname}`));
    document.head.append(replacement);
  });
}
