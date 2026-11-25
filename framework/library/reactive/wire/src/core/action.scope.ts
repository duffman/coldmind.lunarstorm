import { observeReturn } from '@coldmind/wire/src/core/action.errors.ts';
import type { Unsubscribe } from '@coldmind/wire/src/core/action.types.ts';

/** Owns resources, including cleanup that arrives after synchronous closure. */
export class Scope {
  readonly #report: (error: unknown) => void;
  readonly #cleanups = new Set<Unsubscribe>();
  #unlink: Unsubscribe | undefined;
  #closed = false;

  constructor(report: (error: unknown) => void) {
    this.#report = report;
  }

  get closed(): boolean { return this.#closed; }

  add(cleanup: Unsubscribe): Unsubscribe {
    let active = true;
    const release = () => {
      if (!active) return;
      active = false;
      this.#cleanups.delete(release);
      try { observeReturn(cleanup(), this.#report); }
      catch (error) { this.#report(error); }
    };
    if (this.#closed) release();
    else this.#cleanups.add(release);
    return release;
  }

  child(): Scope {
    const child = new Scope(this.#report);
    const release = this.add(() => child.close());
    child.#unlink = () => this.#cleanups.delete(release);
    if (child.closed) child.#unlink();
    return child;
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#unlink?.();
    this.#unlink = undefined;
    const cleanups = [...this.#cleanups];
    this.#cleanups.clear();
    for (const cleanup of cleanups) cleanup();
  }
}
