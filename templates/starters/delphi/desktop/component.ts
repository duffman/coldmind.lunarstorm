import type { Application } from './application';

export type EventHandler<T> = (sender: T) => void;

/** Synchronous typed notifications; the application owns scheduling. */
export class Event<T> {
  private readonly listeners = new Set<EventHandler<T>>();
  subscribe(handler: EventHandler<T>): () => void {
    this.listeners.add(handler);
    return () => { this.listeners.delete(handler); };
  }
  emit(sender: T): void {
    const errors: unknown[] = [];
    for (const handler of [...this.listeners]) {
      try { handler(sender); } catch (error) { errors.push(error); }
    }
    if (errors.length === 1) throw errors[0];
    if (errors.length > 1) throw new AggregateError(errors, 'Event handlers failed.');
  }
  clear(): void { this.listeners.clear(); }
}

export class Component {
  private readonly owned: Component[] = [];
  private released = false;
  readonly onDestroy = new Event<Component>();
  constructor(readonly owner: Component | Application, readonly name: string) {
    if (!name) throw new Error('Components require a name.');
    if (owner.disposed) throw new Error('Cannot attach a component to a disposed owner.');
    owner.adopt(this);
  }
  get application(): Application { return this.owner.application; }
  get disposed(): boolean { return this.released; }
  get components(): readonly Component[] { return this.owned; }
  adopt(component: Component): void {
    if (this.owned.some(child => child.name === component.name)) throw new Error(`Duplicate component name: ${component.name}`);
    this.owned.push(component);
  }
  release(component: Component): void {
    const index = this.owned.indexOf(component);
    if (index >= 0) this.owned.splice(index, 1);
  }
  dispose(): void {
    if (this.released) return;
    this.released = true;
    const errors: unknown[] = [];
    for (const component of [...this.owned].reverse()) {
      try { component.dispose(); } catch (error) { errors.push(error); }
    }
    try { this.onDestroy.emit(this); } catch (error) { errors.push(error); }
    finally { this.onDestroy.clear(); this.owner.release(this); }
    if (errors.length) throw new AggregateError(errors, `Failed to destroy ${this.name}.`);
  }
}
