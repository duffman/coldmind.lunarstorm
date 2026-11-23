import { Event, EventLoop } from './events';
import { ServiceContainer } from './services';
import { isViewClass } from './decorators';
import type { DesktopHost } from './host';
import type { LSNode } from './jsx-runtime';
import { View } from './view';

export type ViewType<T extends View = View> = new () => T;
export class Application {
  readonly services = new ServiceContainer();
  readonly onError = new Event<unknown>();
  private readonly loop = new EventLoop();
  private readonly windows = new Set<View>();
  private released = false;
  constructor(readonly host: DesktopHost) {}
  get disposed(): boolean { return this.released; }

  run<T extends View>(type: ViewType<T>, layout: (view: T) => LSNode): T {
    if (this.released) throw new Error('Application has been disposed.');
    if (!isViewClass(type)) throw new Error('Application views require @ViewClass.');
    const view = new type();
    view.initialize(this);
    this.windows.add(view);
    view.onDestroy.subscribe(() => { this.windows.delete(view); });
    try {
      view.load(layout(view));
      this.dispatch(() => view.show());
      return view;
    } catch (error) {
      view.dispose();
      throw error;
    }
  }

  dispatch(message: () => void): void {
    this.loop.dispatch(() => {
      try { message(); } catch (error) { this.reportError(error); }
    });
  }

  reportError(error: unknown): void {
    if (!this.onError.hasListeners) throw error;
    this.onError.emit(error);
  }

  dispose(): void {
    if (this.released) return;
    this.released = true;
    this.loop.dispose();
    const errors: unknown[] = [];
    for (const view of [...this.windows].reverse()) {
      try { view.dispose(); } catch (error) { errors.push(error); }
    }
    try { this.services.dispose(); } catch (error) { errors.push(error); }
    try { this.host.dispose(); } catch (error) { errors.push(error); }
    this.onError.clear();
    if (errors.length) throw new AggregateError(errors, 'Application disposal failed.');
  }
}
