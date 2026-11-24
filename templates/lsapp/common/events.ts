export type Unsubscribe = () => void;

/** Snapshot delivery permits subscription changes during an event. */
export class Event<T> {
  private readonly listeners = new Set<(event: T) => void>();

  get hasListeners(): boolean { return this.listeners.size > 0; }

  subscribe(listener: (event: T) => void): Unsubscribe {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  emit(event: T): void {
    for (const listener of [...this.listeners]) listener(event);
  }

  clear(): void { this.listeners.clear(); }
}

/** Nested messages run after the current message finishes. */
export class EventLoop {
  private readonly queue: (() => void)[] = [];
  private processing = false;
  private released = false;

  dispatch(message: () => void): void {
    if (this.released) return;
    this.queue.push(message);
    if (this.processing) return;
    this.processing = true;
    try {
      while (this.queue.length && !this.released) this.queue.shift()!();
    } catch (error) {
      this.queue.length = 0;
      throw error;
    } finally {
      this.processing = false;
    }
  }

  dispose(): void {
    this.released = true;
    this.queue.length = 0;
  }
}
