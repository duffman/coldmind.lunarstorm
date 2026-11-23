import { Component, Event } from './component';
import type { DesktopHost } from './host';
import type { Form } from './form';

export class Application {
  private readonly owned: Component[] = [];
  private readonly queue: (() => void)[] = [];
  private processing = false;
  private released = false;
  private started = false;
  readonly onException = new Event<unknown>();
  constructor(readonly host: DesktopHost) {}
  get application(): Application { return this; }
  get disposed(): boolean { return this.released; }
  get components(): readonly Component[] { return this.owned; }
  adopt(component: Component): void {
    if (this.owned.some(child => child.name === component.name)) throw new Error(`Duplicate component name: ${component.name}`);
    this.owned.push(component);
  }
  release(component: Component): void { const index = this.owned.indexOf(component); if (index >= 0) this.owned.splice(index, 1); }
  run(mainForm: Form): void {
    if (this.started || this.released) throw new Error('Application can run once.');
    if (mainForm.application !== this) throw new Error('Main form belongs to a different application.');
    this.started = true;
    mainForm.show();
  }
  dispatch(action: () => void): void {
    if (this.released) return;
    this.queue.push(action);
    if (this.processing) return;
    this.processing = true;
    try { while (this.queue.length && !this.released) this.queue.shift()!(); }
    catch (error) { this.queue.length = 0; this.onException.emit(error); throw error; }
    finally { this.processing = false; }
  }
  dispose(): void {
    if (this.released) return;
    this.released = true; this.queue.length = 0;
    const errors: unknown[] = [];
    for (const component of [...this.owned].reverse()) {
      try { component.dispose(); } catch (error) { errors.push(error); }
    }
    try { this.host.dispose(); } catch (error) { errors.push(error); }
    this.onException.clear();
    if (errors.length) throw new AggregateError(errors, 'Application teardown failed.');
  }
}
