/** @module @coldmind/devtools-shell/errors — the errors the shell shows, from every layer. */

/**
 * Where an error came from.
 * - `server`: compile error frame from the dev server.
 * - `load`: the app entry (or one of its imports) failed to load.
 * - `render`: thrown while rendering; caught by the shell's error boundary.
 * - `runtime`: uncaught error or unhandled rejection (event handlers, timers, async code).
 * - `hmr`: a hot update failed to apply.
 */
export type ShellErrorKind = "server" | "load" | "render" | "runtime" | "hmr";

/** One reported error. */
export interface ShellError {
  readonly id: number;
  readonly kind: ShellErrorKind;
  readonly message: string;
  readonly stack?: string;
  /** Source location, `file:line:column` when known. */
  readonly location?: string;
  /** Code frame from the compiler. */
  readonly frame?: string;
  /** React component stack of a render error. */
  readonly componentStack?: string;
  readonly time: number;
}

/** Fields a reporter supplies; `id` and `time` are assigned. */
export type ShellErrorInput = Omit<ShellError, "id" | "time">;

/** Kinds that replace the app with an error view instead of only listing the error. */
export const BLOCKING_KINDS: ReadonlySet<ShellErrorKind> = new Set(["server", "load", "render"]);

/**
 * Turns anything thrown into the fields of a {@link ShellErrorInput}.
 * @param kind - The layer that caught it.
 */
export function toShellError(kind: ShellErrorKind, thrown: unknown): ShellErrorInput {
  if (thrown instanceof Error) return { kind, message: thrown.message || thrown.name, stack: thrown.stack };
  if (thrown && typeof thrown === "object" && "message" in thrown) {
    return { kind, message: String((thrown as { message: unknown }).message) };
  }
  return { kind, message: String(thrown) };
}

/**
 * Error list shared by the error boundary, the HMR host, the window listeners and the overlay.
 * The snapshot is immutable, so React reads it with `useSyncExternalStore`.
 *
 * @example
 * const errors = new ShellErrors();
 * errors.subscribe(() => console.log(errors.list().length));
 * errors.report({ kind: "runtime", message: "boom" });
 */
export class ShellErrors {
  #items: readonly ShellError[] = [];
  #listeners = new Set<() => void>();
  #nextId = 1;
  #maxRuntime: number;

  /** @param maxRuntime - How many `runtime` errors to keep; older ones drop first. Default 20. */
  constructor(maxRuntime = 20) {
    this.#maxRuntime = maxRuntime;
  }

  /** Current errors, oldest first. The same array until something changes. */
  list = (): readonly ShellError[] => this.#items;

  /** Calls `listener` after every change. Returns the unsubscribe function. */
  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  /** Whether an error of a {@link BLOCKING_KINDS} kind is present. */
  blocking(): boolean {
    return this.#items.some((e) => BLOCKING_KINDS.has(e.kind));
  }

  /** Adds an error. A repeat of the newest error (same kind and message) is not added twice. */
  report(input: ShellErrorInput): ShellError {
    const last = this.#items.at(-1);
    if (last && last.kind === input.kind && last.message === input.message) return last;
    const error: ShellError = { ...input, id: this.#nextId++, time: Date.now() };
    let items = [...this.#items, error];
    const runtime = items.filter((e) => e.kind === "runtime");
    if (runtime.length > this.#maxRuntime) {
      const drop = new Set(runtime.slice(0, runtime.length - this.#maxRuntime));
      items = items.filter((e) => !drop.has(e));
    }
    this.#set(items);
    return error;
  }

  /** Removes the errors of the given kinds, or all errors without arguments. */
  clear(...kinds: ShellErrorKind[]): void {
    const next = kinds.length ? this.#items.filter((e) => !kinds.includes(e.kind)) : [];
    if (next.length !== this.#items.length) this.#set(next);
  }

  /** Removes one error. */
  dismiss(id: number): void {
    const next = this.#items.filter((e) => e.id !== id);
    if (next.length !== this.#items.length) this.#set(next);
  }

  #set(items: readonly ShellError[]): void {
    this.#items = items;
    for (const listener of [...this.#listeners]) listener();
  }
}
