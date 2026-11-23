import { Button, Control, Edit, Label, Memo, Panel, StatusBar, ToolBar, WindowLayout } from './controls';
import type { Unsubscribe } from './events';
import { View, type Window } from './view';

/** Native hosts can implement this contract without changing view classes. */
export interface DesktopHost {
  open(window: Window): void;
  close(window: Window): void;
  dispose(): void;
}

interface MountedWindow {
  readonly element: HTMLElement;
  readonly subscriptions: Unsubscribe[];
}

/** Mock desktop host. Each component retains its DOM element for its lifetime. */
export class BrowserHost implements DesktopHost {
  private readonly windows = new Map<Window, MountedWindow>();
  constructor(private readonly root: HTMLElement) {}

  open(window: Window): void {
    if (this.windows.has(window)) return;
    if (!(window instanceof View)) throw new Error('BrowserHost requires a View with a layout.');
    const subscriptions: Unsubscribe[] = [];
    const frame = document.createElement('section');
    frame.className = 'ls-window';
    frame.style.width = `${window.width}px`;
    frame.style.minHeight = `${window.height}px`;
    frame.setAttribute('role', 'region');
    frame.setAttribute('aria-label', window.title);
    const titlebar = document.createElement('header');
    titlebar.className = 'ls-titlebar';
    const title = document.createElement('span');
    title.textContent = window.title;
    const close = document.createElement('button');
    close.type = 'button';
    close.textContent = '×';
    close.setAttribute('aria-label', 'Close window');
    close.addEventListener('click', () => window.application.dispatch(() => window.close()));
    titlebar.append(title, close);
    frame.append(titlebar, this.mount(window.content, subscriptions));
    this.root.append(frame);
    this.windows.set(window, { element: frame, subscriptions });
  }

  private mount(control: Control, subscriptions: Unsubscribe[]): HTMLElement {
    let element: HTMLElement;
    if (control instanceof Button) {
      const button = document.createElement('button');
      button.type = 'button';
      button.addEventListener('click', () => control.click());
      element = button;
    } else if (control instanceof Edit) {
      const edit = control instanceof Memo ? document.createElement('textarea') : document.createElement('input');
      edit.addEventListener('input', () => control.input(edit.value));
      element = edit;
    } else {
      element = document.createElement(control instanceof StatusBar ? 'footer' : 'div');
    }
    const kind = control instanceof WindowLayout ? 'layout' : control instanceof ToolBar ? 'toolbar' : control instanceof Panel ? 'panel' : control instanceof Button ? 'button' : control instanceof Edit ? 'edit' : control instanceof StatusBar ? 'statusbar' : 'label';
    element.className = `ls-${kind}${control.className ? ` ${control.className}` : ''}`;
    if (control instanceof StatusBar) element.setAttribute('role', 'status');
    if (control instanceof ToolBar) element.setAttribute('role', 'toolbar');
    const patch = () => {
      element.hidden = !control.visible;
      element.dataset.align = control.align;
      if (element instanceof HTMLButtonElement || element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement) element.disabled = !control.enabled;
      if (control instanceof Edit && (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement)) {
        if (element.value !== control.text) element.value = control.text;
        element.placeholder = control.placeholder;
        element.readOnly = control.readOnly;
      } else if (control instanceof Label) element.textContent = control.caption;
      else if (control instanceof StatusBar) element.textContent = control.text;
    };
    patch();
    subscriptions.push(control.changed.subscribe(patch));
    for (const child of control.components) {
      if (child instanceof Control) element.append(this.mount(child, subscriptions));
    }
    return element;
  }

  close(window: Window): void {
    const mounted = this.windows.get(window);
    if (!mounted) return;
    mounted.subscriptions.forEach(unsubscribe => unsubscribe());
    mounted.element.remove();
    this.windows.delete(window);
  }

  dispose(): void {
    for (const window of [...this.windows.keys()]) this.close(window);
  }
}
