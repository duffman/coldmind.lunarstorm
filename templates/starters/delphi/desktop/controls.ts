import { Component, Event, type EventHandler } from './component';
import type { Form } from './form';

export type Align = 'none' | 'top' | 'bottom' | 'left' | 'right' | 'client';
export interface ControlOptions {
  name: string;
  caption?: string;
  align?: Align;
  width?: number;
  height?: number;
  enabled?: boolean;
  visible?: boolean;
}

export class Control extends Component {
  readonly parent: Control | Form;
  readonly align: Align;
  readonly width?: number;
  readonly height?: number;
  private title: string;
  private active: boolean;
  private shown: boolean;
  constructor(parent: Control | Form, options: ControlOptions) {
    super(parent, options.name);
    this.parent = parent;
    this.align = options.align ?? 'none';
    this.width = options.width;
    this.height = options.height;
    this.title = options.caption ?? '';
    this.active = options.enabled ?? true;
    this.shown = options.visible ?? true;
  }
  get caption(): string { return this.title; }
  set caption(value: string) { this.title = value; this.changed(); }
  get enabled(): boolean { return this.active; }
  set enabled(value: boolean) { this.active = value; this.changed(); }
  get visible(): boolean { return this.shown; }
  set visible(value: boolean) { this.shown = value; this.changed(); }
  protected changed(): void { if (!this.disposed) this.application.host.update(this); }
  protected notify(action: () => void): void {
    if (this.disposed || !this.enabled) return;
    this.application.dispatch(() => { if (!this.disposed) action(); });
  }
}
export interface PanelOptions extends ControlOptions { layout?: 'row' | 'column'; gap?: number; padding?: number; }
export class Panel extends Control {
  readonly layout: 'row' | 'column'; readonly gap: number; readonly padding: number;
  constructor(parent: Control | Form, options: PanelOptions) {
    super(parent, options); this.layout = options.layout ?? 'column'; this.gap = options.gap ?? 8; this.padding = options.padding ?? 0;
  }
}
export class Label extends Control {}
export class StatusBar extends Control {}
export interface ButtonOptions extends ControlOptions { onClick?: EventHandler<Button>; }
export class Button extends Control {
  readonly onClick = new Event<Button>();
  constructor(parent: Control | Form, options: ButtonOptions) { super(parent, options); if (options.onClick) this.onClick.subscribe(options.onClick); }
  click(): void { this.notify(() => this.onClick.emit(this)); }
  override dispose(): void { this.onClick.clear(); super.dispose(); }
}
export interface EditOptions extends ControlOptions { text?: string; placeholder?: string; readOnly?: boolean; onChange?: EventHandler<Edit>; }
export class Edit extends Control {
  readonly onChange = new Event<Edit>();
  readonly placeholder: string; readonly readOnly: boolean;
  private content: string;
  constructor(parent: Control | Form, options: EditOptions) {
    super(parent, options); this.content = options.text ?? ''; this.placeholder = options.placeholder ?? ''; this.readOnly = options.readOnly ?? false;
    if (options.onChange) this.onChange.subscribe(options.onChange);
  }
  get text(): string { return this.content; }
  set text(value: string) { if (value !== this.content) { this.content = value; this.changed(); } }
  input(value: string): void { if (!this.enabled || this.readOnly || this.disposed) return; this.text = value; this.notify(() => this.onChange.emit(this)); }
  override dispose(): void { this.onChange.clear(); super.dispose(); }
}
export class Memo extends Edit {}
export interface ListBoxOptions extends ControlOptions { items?: readonly string[]; itemIndex?: number; onChange?: EventHandler<ListBox>; }
export class ListBox extends Control {
  readonly onChange = new Event<ListBox>();
  private values: readonly string[];
  private index: number;
  constructor(parent: Control | Form, options: ListBoxOptions) {
    super(parent, options); this.values = [...(options.items ?? [])]; this.index = options.itemIndex ?? -1;
    if (options.onChange) this.onChange.subscribe(options.onChange);
  }
  get items(): readonly string[] { return this.values; }
  set items(value: readonly string[]) { this.values = [...value]; if (this.index >= value.length) this.index = -1; this.changed(); }
  get itemIndex(): number { return this.index; }
  set itemIndex(value: number) {
    if (!Number.isInteger(value) || value < -1 || value >= this.values.length) throw new RangeError('Selection is outside the item list.');
    this.index = value; this.changed();
  }
  select(value: number): void { if (!this.enabled || this.disposed) return; this.itemIndex = value; this.notify(() => this.onChange.emit(this)); }
  override dispose(): void { this.onChange.clear(); super.dispose(); }
}
export class ComboBox extends ListBox {}
