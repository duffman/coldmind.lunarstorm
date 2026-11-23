import { Button, ComboBox, Control, Edit, Label, ListBox, Memo, Panel, StatusBar } from './controls';
import type { Form } from './form';
import type { DesktopHost } from './host';

/** Browser preview adapter; each retained control owns one stable DOM element. */
export class BrowserHost implements DesktopHost {
  private readonly windows = new Map<Form, HTMLElement>();
  private readonly elements = new Map<Control, HTMLElement>();
  private readonly subscriptions = new Map<Control, () => void>();
  constructor(private readonly root: HTMLElement) { root.classList.add('desktop-host'); }

  open(form: Form): void {
    if (this.windows.has(form)) return;
    const window = document.createElement('section');
    window.className = 'desktop-form'; window.dataset.name = form.name;
    window.style.width = `${form.width}px`; window.style.height = `${form.height}px`;
    window.setAttribute('aria-label', form.caption);
    const titlebar = document.createElement('header'); titlebar.className = 'desktop-titlebar';
    const title = document.createElement('span'); title.textContent = form.caption;
    const close = document.createElement('button'); close.type = 'button'; close.textContent = '×'; close.setAttribute('aria-label', 'Close window');
    close.addEventListener('click', () => form.application.dispatch(() => { form.close(); }));
    titlebar.append(title, close);
    const client = document.createElement('div'); client.className = 'desktop-client';
    for (const component of form.components) if (component instanceof Control) client.append(this.create(component));
    window.append(titlebar, client); this.root.append(window); this.windows.set(form, window);
  }

  private create(control: Control): HTMLElement {
    let element: HTMLElement;
    if (control instanceof Panel) {
      element = document.createElement('div'); element.className = 'desktop-panel';
      element.style.display = 'flex'; element.style.flexDirection = control.layout;
      element.style.gap = `${control.gap}px`; element.style.padding = `${control.padding}px`;
      for (const child of control.components) if (child instanceof Control) element.append(this.create(child));
    } else if (control instanceof Button) {
      const button = document.createElement('button'); button.type = 'button'; button.className = 'desktop-button';
      button.addEventListener('click', () => control.click()); element = button;
    } else if (control instanceof Edit) {
      const input = control instanceof Memo ? document.createElement('textarea') : document.createElement('input');
      input.className = control instanceof Memo ? 'desktop-memo' : 'desktop-edit';
      input.placeholder = control.placeholder; input.readOnly = control.readOnly;
      input.addEventListener('input', () => control.input(input.value)); element = input;
    } else if (control instanceof ListBox) {
      const select = document.createElement('select'); select.className = control instanceof ComboBox ? 'desktop-combobox' : 'desktop-listbox';
      if (!(control instanceof ComboBox)) select.size = 8;
      select.addEventListener('change', () => control.select(select.selectedIndex)); element = select;
    } else {
      element = document.createElement(control instanceof StatusBar ? 'footer' : 'label');
      element.className = control instanceof StatusBar ? 'desktop-statusbar' : 'desktop-label';
      if (control instanceof StatusBar) { element.setAttribute('role', 'status'); element.setAttribute('aria-live', 'polite'); }
    }
    element.dataset.name = control.name;
    element.dataset.align = control.align;
    if (!(control instanceof Label) && !(control instanceof Panel) && !(control instanceof StatusBar)) element.setAttribute('aria-label', control.caption || control.name);
    if (control.align === 'client') { element.style.flex = '1 1 0'; element.style.minWidth = '0'; element.style.minHeight = '0'; }
    else element.style.flexShrink = '0';
    if (control.width !== undefined) element.style.width = `${control.width}px`;
    if (control.height !== undefined) element.style.height = `${control.height}px`;
    this.elements.set(control, element);
    this.subscriptions.set(control, control.onDestroy.subscribe(() => this.remove(control)));
    this.update(control);
    return element;
  }

  update(control: Control): void {
    const element = this.elements.get(control); if (!element) return;
    element.hidden = !control.visible;
    // Explicit display style on panels must respect the hidden attribute too.
    if (control instanceof Panel) element.style.display = control.visible ? 'flex' : 'none';
    if ('disabled' in element) (element as HTMLButtonElement).disabled = !control.enabled;
    if (control instanceof Edit) {
      const input = element as HTMLInputElement | HTMLTextAreaElement;
      if (input.value !== control.text) input.value = control.text;
    } else if (control instanceof ListBox) {
      const select = element as HTMLSelectElement;
      const changed = select.options.length !== control.items.length || control.items.some((item, index) => select.options[index]?.text !== item);
      if (changed) select.replaceChildren(...control.items.map(item => { const option = document.createElement('option'); option.textContent = item; return option; }));
      select.selectedIndex = control.itemIndex;
    } else if (!(control instanceof Panel)) element.textContent = control.caption;
  }

  private remove(control: Control): void {
    this.elements.get(control)?.remove(); this.elements.delete(control);
    this.subscriptions.get(control)?.(); this.subscriptions.delete(control);
  }
  close(form: Form): void { this.windows.get(form)?.remove(); this.windows.delete(form); }
  dispose(): void {
    for (const window of this.windows.values()) window.remove();
    this.windows.clear(); this.elements.clear();
    for (const unsubscribe of this.subscriptions.values()) unsubscribe(); this.subscriptions.clear();
  }
}
