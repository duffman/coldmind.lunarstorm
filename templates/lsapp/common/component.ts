import { Event } from './events';
import type { Application } from './application';

export class Component {
  private parent?: Component;
  private app?: Application;
  private readonly children: Component[] = [];
  private released = false;
  readonly onDestroy = new Event<Component>();

  get owner(): Component | undefined { return this.parent; }
  get components(): readonly Component[] { return this.children; }
  get disposed(): boolean { return this.released; }
  get application(): Application {
    const app = this.app ?? this.parent?.application;
    if (!app) throw new Error('Component has no application owner.');
    return app;
  }

  attach(owner: Component | Application): void {
    if (this.released || this.parent || this.app) throw new Error('A component can have one owner during its lifetime.');
    if (owner instanceof Component) {
      if (owner.disposed) throw new Error('Cannot attach to a disposed owner.');
      this.parent = owner;
      owner.children.push(this);
    } else {
      this.app = owner;
    }
  }

  dispose(): void {
    if (this.released) return;
    this.released = true;
    const errors: unknown[] = [];
    for (const child of [...this.children].reverse()) {
      try { child.dispose(); } catch (error) { errors.push(error); }
    }
    try { this.onDestroy.emit(this); } catch (error) { errors.push(error); }
    this.onDestroy.clear();
    if (this.parent) {
      const index = this.parent.children.indexOf(this);
      if (index >= 0) this.parent.children.splice(index, 1);
    }
    if (errors.length) throw new AggregateError(errors, 'Component disposal failed.');
  }
}
