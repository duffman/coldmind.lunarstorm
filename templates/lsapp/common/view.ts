import { Component } from './component';
import { Button, Control, Edit, Panel } from './controls';
import { isViewAction, type ControlType } from './decorators';
import type { LSNode } from './jsx-runtime';
import type { Application } from './application';
import { connectServices } from './services';

export interface CloseQueryEvent { readonly sender: Window; cancel: boolean; }

export class Window extends Component {
  title = 'Application';
  width = 800;
  height = 560;
  private opened = false;
  get visible(): boolean { return this.opened; }
  protected onCreate(): void {}
  protected onShow(): void {}
  protected onCloseQuery(_event: CloseQueryEvent): void {}

  initialize(application: Application): void {
    this.attach(application);
    connectServices(this, application.services);
  }

  show(): void {
    if (this.disposed) throw new Error('Cannot show a disposed window.');
    if (this.opened) return;
    this.onCreate();
    this.application.host.open(this);
    this.opened = true;
    this.onShow();
  }

  close(): boolean {
    if (this.disposed) return true;
    const event: CloseQueryEvent = { sender: this, cancel: false };
    this.onCloseQuery(event);
    if (event.cancel) return false;
    this.dispose();
    return true;
  }

  override dispose(): void {
    if (this.disposed) return;
    if (this.opened) this.application.host.close(this);
    this.opened = false;
    super.dispose();
  }
}

export class View extends Window {
  private readonly outlets = new Map<PropertyKey, Control>();
  private loaded = false;
  private root?: Control;
  get content(): Control {
    if (!this.root) throw new Error('View has no layout.');
    return this.root;
  }

  resolveOutlet<T extends Control>(key: PropertyKey, type: ControlType<T>): T {
    if (this.disposed) throw new Error('Cannot resolve an outlet on a disposed view.');
    const current = this.outlets.get(key);
    if (current) return current as T;
    const control = new type();
    this.outlets.set(key, control);
    return control;
  }

  load(node: LSNode): void {
    if (this.loaded) throw new Error('A view layout can only be loaded once.');
    const used = new Set<Control>();
    try {
      this.root = this.instantiate(node, this, used);
      for (const outlet of this.outlets.values()) {
        if (!used.has(outlet)) throw new Error('A declared outlet was accessed but not connected to the layout.');
      }
      this.loaded = true;
    } catch (error) {
      for (const child of [...this.components]) child.dispose();
      throw error;
    }
  }

  private instantiate(node: LSNode, owner: Component, used: Set<Control>): Control {
    const control = node.props.outlet ?? new node.type();
    if (!(control instanceof node.type)) throw new Error('Outlet type does not match its JSX component.');
    if (used.has(control)) throw new Error('An outlet can only be connected once.');
    if (node.children.length && !(control instanceof Panel)) throw new Error('Only containers can own child controls.');
    used.add(control);
    control.attach(owner);
    for (const [key, value] of Object.entries(node.props)) {
      if (key === 'outlet' || key === 'children') continue;
      if (key === 'onClick' && control instanceof Button) {
        if (!isViewAction(value)) throw new Error('Control events must reference a @ViewAction method.');
        control.onClick.subscribe(value);
      } else if (key === 'onChange' && control instanceof Edit) {
        if (!isViewAction(value)) throw new Error('Control events must reference a @ViewAction method.');
        control.onChange.subscribe(value);
      } else {
        if (!(key in control)) throw new Error(`Unsupported component property: ${key}`);
        Reflect.set(control, key, value);
      }
    }
    for (const child of node.children) this.instantiate(child, control, used);
    return control;
  }

  override dispose(): void {
    const errors: unknown[] = [];
    try { super.dispose(); } catch (error) { errors.push(error); }
    for (const outlet of this.outlets.values()) {
      try { outlet.dispose(); } catch (error) { errors.push(error); }
    }
    this.outlets.clear();
    if (errors.length) throw new AggregateError(errors, 'View disposal failed.');
  }
}
