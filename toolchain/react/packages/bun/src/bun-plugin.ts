/**
 * @module @coldmind/devtools — a Rust `onBeforeParse` plugin for Bun's bundler and dev server.
 *
 * The transform runs natively on Bun's bundler threads. Bun keeps resolution, the module graph,
 * HMR and production bundling; this module loads the native library and registers its hook.
 * Edits are spliced into the original source, so untouched code keeps its position.
 *
 * Bun runs one `onBeforeParse` hook per file, so every stage is configured on one plugin:
 * register a single `devtools(...)` per build.
 */
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import type { BunPlugin } from "bun";

/** One literal substitution: every occurrence of `from` becomes `to`. */
export interface ReplacePair {
  from: string;
  to: string;
}

/** Counters since the plugin was created. */
export interface DevtoolsStats {
  /** Files the hook was called for. */
  files: number;
  /** Files whose source changed. */
  changed: number;
  /** Files parsed by oxc (a define or inject pre-check passed). */
  parsed: number;
  /** Literal substitutions made. */
  replaced: number;
  /** Define references replaced. */
  defined: number;
  /** Classes given a `$inject` getter. */
  injected: number;
}

/** Opaque native state owned by one plugin instance; freed when garbage collected. */
export type NativeState = { readonly __devtoolsState: unique symbol };

/** Surface of the compiled `.node` library. */
export interface DevtoolsNative {
  createState(options: {
    replace?: ReplacePair[];
    define?: { key: string; value: string }[];
    injectAll?: boolean;
  }): NativeState;
  stats(state: NativeState): DevtoolsStats;
  takeWarnings(state: NativeState): string[];
}

/** Options for {@link devtools}. Stages left out are off. */
export interface DevtoolsOptions {
  /**
   * Literal substitutions on the raw text, before parsing; also rewrites strings and comments.
   * `from` must be non-empty. Where matches overlap, the earlier pair wins.
   */
  replace?: ReplacePair[];
  /**
   * Scope-aware constants: dotted paths rooted at a global or `import.meta`, mapped to JS
   * expressions (`{ "process.env.NODE_ENV": '"production"' }`). Only real references change:
   * strings, comments, property names and local bindings of the same name are untouched; a
   * `declare const` still counts as the global; `{ __MODE__ }` expands to `{ __MODE__: ... }`.
   */
  define?: Record<string, string>;
  /**
   * Generates `@coldmind/inject` constructor dependencies: adds `static get $inject()` from
   * constructor parameter types to classes with a decorator or `static $injectable` that do not
   * declare `$inject` (`{ all: true }`: every class). Optional or defaulted parameters become
   * `{ token, optional: true }`. A parameter typed as a primitive, interface, type alias or
   * type-only import leaves the class unchanged and produces a warning. Interfaces must be
   * imported with `import type`.
   */
  inject?: boolean | { all?: boolean };
  /** Files the hook runs on. Defaults to JS/TS sources. */
  filter?: RegExp;
  /** Module namespace the hook runs in. Defaults to `file`. */
  namespace?: string;
  /** Called with each warning at the end of a build. Defaults to `console.warn`. */
  onWarning?: (message: string) => void;
}

/** A Bun plugin that also exposes its native counters and pending warnings. */
export interface DevtoolsPlugin extends BunPlugin {
  /** Counters for this plugin instance, read from native memory. */
  stats(): DevtoolsStats;
  /** Removes and returns warnings not yet reported (the dev server has no build end). */
  takeWarnings(): string[];
}

const nativeDirectory = join(import.meta.dir, "../native");
const defaultFilter = /\.[cm]?[jt]sx?$/;
let cached: DevtoolsNative | undefined;

/**
 * File name of the native library for a platform, as written by `bun run build:native`.
 * @param platform - Defaults to the current platform.
 * @param arch - Defaults to the current architecture.
 * @example
 * nativeFileName("darwin", "arm64"); // "devtools.darwin-arm64.node"
 */
export function nativeFileName(platform: string = process.platform, arch: string = process.arch): string {
  return `devtools.${platform}-${arch}.node`;
}

/**
 * Loads the native library once per process.
 * @returns The library, also used as the `napiModule` Bun calls into.
 * @throws {Error} When the library for this platform has not been built.
 */
export function loadNative(): DevtoolsNative {
  if (cached) return cached;
  const path = join(nativeDirectory, nativeFileName());
  if (!existsSync(path)) {
    throw new Error(`devtools native library missing: ${path}. Run \`bun run --filter @coldmind/devtools build:native\`.`);
  }
  cached = createRequire(import.meta.url)(path) as DevtoolsNative;
  return cached;
}

/**
 * Creates the devtools Bun plugin. Usable in `Bun.build({ plugins })` and, exported as a
 * module's default, as a `bunfig.toml` `[serve.static] plugins` entry for the dev server.
 * @param options - Stages to run and files to run them on.
 * @returns A plugin with its own native state, counters and warnings.
 * @throws {Error} When the native library is missing, a replace `from` is empty, a define key is
 *   not a dotted identifier path, or a define value is not a JS expression.
 * @example
 * const plugin = devtools({
 *   define: { "import.meta.env.MODE": JSON.stringify("production") },
 *   inject: true,
 * });
 * await Bun.build({ entrypoints: ["./src/app.ts"], plugins: [plugin] });
 * plugin.stats(); // { files, changed, parsed, replaced, defined, injected }
 */
export function devtools(options: DevtoolsOptions = {}): DevtoolsPlugin {
  const napiModule = loadNative();
  const inject = options.inject;
  const external = napiModule.createState({
    ...(options.replace ? { replace: options.replace } : {}),
    ...(options.define
      ? { define: Object.entries(options.define).map(([key, value]) => ({ key, value })) }
      : {}),
    ...(inject ? { injectAll: typeof inject === "object" && inject.all === true } : {}),
  });
  const onWarning = options.onWarning ?? ((message: string) => console.warn(message));
  return {
    name: "devtools",
    setup(build) {
      build.onBeforeParse(
        { filter: options.filter ?? defaultFilter, namespace: options.namespace ?? "file" },
        { napiModule, symbol: "devtools_transform", external },
      );
      if (typeof build.onEnd === "function") {
        build.onEnd(() => {
          for (const message of napiModule.takeWarnings(external)) onWarning(message);
        });
      }
    },
    stats: () => napiModule.stats(external),
    takeWarnings: () => napiModule.takeWarnings(external),
  };
}
