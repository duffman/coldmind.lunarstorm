/** @module @coldmind/devtools-shell/app — loads the app entry and renders it inside the boundary. */

import { useEffect, useState, useSyncExternalStore, type ComponentType, type ReactNode } from "react";

import { ShellBoundary } from "./error-boundary.tsx";
import { toShellError, type ShellErrors } from "./error-store.ts";

/** An app entry module: its default export is the root component. */
export interface AppModule {
  default: ComponentType;
}

/** Props of {@link ShellApp}. */
export interface ShellAppProps {
  /** Imports the app entry, e.g. `() => import("./App.tsx")`. */
  load: () => Promise<AppModule>;
  errors: ShellErrors;
  /** Hot update counter; a change re-mounts a failed app. */
  subscribeGeneration: (listener: () => void) => () => void;
  getGeneration: () => number;
  /** Called once the entry failed to load, so the next update reloads the page. */
  onLoadFailed: () => void;
  fallback?: (error: Error, retry: () => void) => ReactNode;
}

type Loaded = { state: "loading" } | { state: "ready"; App: ComponentType } | { state: "failed" };

/**
 * Imports the app once and renders its default export inside {@link ShellBoundary}. A failed
 * import is reported as a `load` error and renders nothing: the boundaries inside the app never
 * registered, so the shell reloads the page on the next update instead.
 */
export function ShellApp({ load, errors, subscribeGeneration, getGeneration, onLoadFailed, fallback }: ShellAppProps) {
  const generation = useSyncExternalStore(subscribeGeneration, getGeneration);
  const [loaded, setLoaded] = useState<Loaded>({ state: "loading" });

  useEffect(() => {
    let live = true;
    load().then(
      (mod) => {
        if (!live) return;
        if (typeof mod?.default !== "function") {
          errors.report({ kind: "load", message: "The app entry has no default export component" });
          onLoadFailed();
          setLoaded({ state: "failed" });
          return;
        }
        errors.clear("load");
        setLoaded({ state: "ready", App: mod.default });
      },
      (error: unknown) => {
        if (!live) return;
        errors.report(toShellError("load", error));
        onLoadFailed();
        setLoaded({ state: "failed" });
      },
    );
    return () => {
      live = false;
    };
  }, [load, errors, onLoadFailed]);

  if (loaded.state !== "ready") return null;
  const { App } = loaded;
  return (
    <ShellBoundary errors={errors} resetKey={generation} fallback={fallback}>
      <App />
    </ShellBoundary>
  );
}
