/**
 * @module @coldmind/devtools/tauri — typed frontend API for tauri-plugin-devtools.
 *
 * Supply `invoke` from `@tauri-apps/api/core`. This entry does not load Bun, Node-API,
 * or a native `.node` library into the webview. It only sends source text over Tauri IPC.
 */

/** The supported parser syntax; output retains TypeScript and JSX syntax. */
export type SourceLoader = "js" | "jsx" | "ts" | "tsx";

/** Source transformations; the same options as the Bun plugin, without bundler callbacks. */
export interface TauriTransformOptions {
  /** Literal substitution, including strings and comments. `from` must not be empty. */
  replace?: { from: string; to: string }[];
  /** Scope-aware global references mapped to JavaScript expression source. */
  define?: Record<string, string>;
  /** `true` selects marked classes; `{ all: true }` selects every class. */
  inject?: boolean | { all?: boolean };
}

/** Input source and options. `filename` is a warning label, never a filesystem path to open. */
export interface TransformRequest {
  source: string;
  loader: SourceLoader;
  /** Defaults to `<source>`. */
  filename?: string;
  options?: TauriTransformOptions;
}

/** Transformed text, counters for this request only, and injection warnings. */
export interface TransformResult {
  code: string;
  changed: boolean;
  parsed: boolean;
  replaced: number;
  defined: number;
  injected: number;
  warnings: string[];
}

/** Compatible with `invoke` from `@tauri-apps/api/core`; injected to avoid a Bun/Tauri dependency. */
export type TauriInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

/** Text-only Tauri adapter. It neither executes source nor starts an HTTP/HMR server. */
export interface TauriDevtools {
  /** Rejects if permission is missing or the transformation options are invalid. */
  transform(request: TransformRequest): Promise<TransformResult>;
}

/**
 * Creates a typed client for `plugin:devtools|transform`.
 * Register `tauri_plugin_devtools::init()` in Rust and grant `devtools:default`
 * in the webview capability before invoking it. The returned source is not transpiled.
 *
 * @example
 * import { invoke } from "@tauri-apps/api/core";
 * import { createTauriDevtools } from "@coldmind/devtools/tauri";
 * const devtools = createTauriDevtools(invoke);
 * const result = await devtools.transform({
 *   source: "export const mode = __MODE__;",
 *   loader: "ts",
 *   options: { define: { __MODE__: JSON.stringify("development") } },
 * });
 */
export function createTauriDevtools(invoke: TauriInvoke): TauriDevtools {
  return {
    transform: (request) => invoke<TransformResult>("plugin:devtools|transform", { request }),
  };
}
