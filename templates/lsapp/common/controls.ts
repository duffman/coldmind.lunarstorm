import { Component } from './component';
import { Event } from './events';

export interface ControlProps {
  outlet?: Control;
  className?: string;
  align?: 'none' | 'top' | 'bottom' | 'left' | 'right' | 'client';
  enabled?: boolean;
  visible?: boolean;
  children?: unknown;
}
export interface ClickEvent { readonly sender: Button; }
export interface ChangeEvent<T> { readonly sender: Control; readonly value: T; }

export class Control extends Component {
  readonly changed = new Event<Control>();
  private active = true;
  private shown = true;
  align: ControlProps['align'] = 'none';
  className = '';
  constructor(_props: ControlProps = {}) { super(); }
  get enabled(): boolean { return this.active; }
  set enabled(value: boolean) { if (this.active !== value) { this.active = value; this.notify(); } }
  get visible(): boolean { return this.shown; }
  set visible(value: boolean) { if (this.shown !== value) { this.shown = value; this.notify(); } }
  protected notify(): void {
    if (this.disposed || !this.owner) return;
    this.application.dispatch(() => { if (!this.disposed) this.changed.emit(this); });
  }
  override dispose(): void { this.changed.clear(); super.dispose(); }
}
export class Panel extends Control {}
export class WindowLayout extends Panel {}

export interface LabelProps extends ControlProps { caption?: string; }
export class Label extends Control {
  private value = '';
  constructor(_props: LabelProps = {}) { super(); }
  get caption(): string { return this.value; }
  set caption(value: string) { if (this.value !== value) { this.value = value; this.notify(); } }
}
export interface EditProps extends ControlProps { text?: string; placeholder?: string; readOnly?: boolean; onChange?: (event: ChangeEvent<string>) => void; }
export class Edit extends Control {
  private value = '';
  placeholder = '';
  readOnly = false;
  readonly onChange = new Event<ChangeEvent<string>>();
  constructor(_props: EditProps = {}) { super(); }
  get text(): string { return this.value; }
  set text(value: string) { if (this.value !== value) { this.value = value; this.notify(); } }
  input(value: string): void {
    if (this.disposed || !this.enabled || this.readOnly) return;
    this.application.dispatch(() => {
      if (this.disposed) return;
      this.text = value;
      this.onChange.emit({ sender: this, value });
    });
  }
  override dispose(): void { this.onChange.clear(); super.dispose(); }
}
export class Memo extends Edit {}
export interface ButtonProps extends LabelProps { onClick?: (event: ClickEvent) => void; }
export class Button extends Label {
  readonly onClick = new Event<ClickEvent>();
  constructor(_props: ButtonProps = {}) { super(); }
  click(): void {
    if (!this.disposed && this.enabled) this.application.dispatch(() => {
      if (!this.disposed && this.enabled) this.onClick.emit({ sender: this });
    });
  }
  override dispose(): void { this.onClick.clear(); super.dispose(); }
}
export interface StatusBarProps extends ControlProps { text?: string; }
export class StatusBar extends Control {
  private value = '';
  constructor(_props: StatusBarProps = {}) { super(); }
  get text(): string { return this.value; }
  set text(value: string) { if (this.value !== value) { this.value = value; this.notify(); } }
}
export class ToolBar extends Panel {}
