import { Component, Event } from './component';
import { Button, ComboBox, Control, Edit, Label, ListBox, Memo, Panel, StatusBar } from './controls';
import type { Application } from './application';
import type { ControlDefinition } from './jsx-runtime';

export interface FormOptions { name: string; caption: string; width?: number; height?: number; }
export interface CloseQuery { readonly sender: Form; cancel: boolean; }
export class Form extends Component {
  readonly caption: string; readonly width: number; readonly height: number;
  readonly onCreate = new Event<Form>();
  readonly onShow = new Event<Form>();
  readonly onCloseQuery = new Event<CloseQuery>();
  private initialized = false;
  private shown = false;
  constructor(owner: Application, options: FormOptions) {
    super(owner, options.name); this.caption = options.caption; this.width = options.width ?? 1000; this.height = options.height ?? 680;
  }
  get visible(): boolean { return this.shown; }
  load(definition: ControlDefinition): void {
    if (this.initialized || this.components.length) throw new Error('A form definition can only be loaded once before showing.');
    try { this.instantiate(this, definition, new Set()); }
    catch (error) { for (const component of [...this.components]) component.dispose(); throw error; }
  }
  private instantiate(parent: Control | Form, definition: ControlDefinition, names: Set<string>): void {
    const name = definition.props.name;
    if (typeof name !== 'string' || names.has(name)) throw new Error(`Form control names must be unique: ${String(name)}`);
    names.add(name);
    const constructors = { panel: Panel, button: Button, edit: Edit, memo: Memo, label: Label, statusbar: StatusBar, listbox: ListBox, combobox: ComboBox };
    const Constructor = constructors[definition.type];
    if (!Constructor) throw new Error(`Unknown desktop control: ${definition.type}`);
    const control = new Constructor(parent, definition.props as unknown as { name: string });
    if (definition.children.length && !(control instanceof Panel)) throw new Error(`Only panels can contain controls: ${name}`);
    for (const child of definition.children) this.instantiate(control, child, names);
  }
  find<T extends Control = Control>(name: string): T {
    const visit = (owner: Component): Control | undefined => {
      for (const child of owner.components) {
        if (child.name === name && child instanceof Control) return child;
        const found = visit(child); if (found) return found;
      }
    };
    const found = visit(this);
    if (!found) throw new Error(`Unknown control: ${name}`);
    return found as T;
  }
  show(): void {
    if (this.disposed) throw new Error('Cannot show a disposed form.');
    if (this.shown) return;
    if (!this.initialized) { this.initialized = true; this.onCreate.emit(this); }
    this.shown = true;
    try { this.application.host.open(this); this.onShow.emit(this); }
    catch (error) { this.shown = false; throw error; }
  }
  close(): boolean {
    if (this.disposed) return true;
    const query: CloseQuery = { sender: this, cancel: false };
    this.onCloseQuery.emit(query);
    if (query.cancel) return false;
    this.dispose(); return true;
  }
  override dispose(): void {
    if (this.disposed) return;
    let closeError: unknown;
    if (this.shown) {
      try { this.application.host.close(this); } catch (error) { closeError = error; }
      this.shown = false;
    }
    this.onCreate.clear(); this.onShow.clear(); this.onCloseQuery.clear();
    try { super.dispose(); }
    catch (error) { if (closeError) throw new AggregateError([closeError, error], 'Form teardown failed.'); throw error; }
    if (closeError) throw closeError;
  }
}
