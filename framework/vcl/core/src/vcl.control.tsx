/**
 * Control: a component with a place on screen. `parent`/`controls` is the visual tree, separate from
 * ownership. `ControlHost` is the one bridge to React: one subscription per control, so a property
 * change re-renders that control only.
 *
 * Rule: `render()` uses no React state and no hooks. All state lives on the instance.
 */
import { memo, useSyncExternalStore, type ReactNode } from "react";
import { Component, prop } from "@coldmind/vcl.core/src/vcl.component.ts";
import type { JsxProps } from "@coldmind/vcl.core/src/vcl.jsx.ts";

/** Placement inside the parent, as Delphi's `Align`. The parent's `direction` decides the axis. */
export type Align = "none" | "top" | "bottom" | "left" | "right" | "client";

export abstract class Control extends Component {
  @prop accessor visible = true;
  @prop accessor enabled = true;
  @prop accessor align: Align = "none";
  /** Extra class on the control's root element. */
  @prop accessor className = "";

  /** Type only: the attributes of this control's tag in VCL TSX (`vcl.jsx.ts`). */
  declare readonly jsxProps: JsxProps<this>;

  #parent: Control | undefined;
  readonly #controls: Control[] = [];

  get parent(): Control | undefined {
    return this.#parent;
  }

  /** @throws {Error} When `next` is this control or one of its descendants. */
  set parent(next: Control | undefined) {
    if (next === this.#parent) return;
    for (let p = next; p; p = p.#parent)
      if (p === this) throw new Error(`${this.describeName()}: a control cannot be placed inside itself`);
    const previous = this.#parent;
    if (previous) {
      previous.#controls.splice(previous.#controls.indexOf(this), 1);
      previous.changed();
    }
    this.#parent = next;
    if (next) {
      next.#controls.push(this);
      next.changed();
    }
    this.changed();
  }

  /** Child controls, in insertion order. */
  get controls(): readonly Control[] {
    return this.#controls;
  }

  abstract render(): ReactNode;

  /** The children, each behind its own `ControlHost`. */
  protected renderControls(): ReactNode {
    return this.#controls.map(c => <ControlHost key={c.id} control={c} />);
  }

  override destroy(): void {
    for (const child of [...this.#controls]) child.parent = undefined;
    this.parent = undefined;
    super.destroy();
  }
}

/** Subscribes the calling React component to `component`; returns its version. */
export function useComponent(component: Component): number {
  return useSyncExternalStore(component.subscribe, component.version, component.version);
}

/** Renders a control and re-renders it, alone, when it changes. */
export const ControlHost = memo(function ControlHost({ control }: { control: Control }): ReactNode {
  useComponent(control);
  return control.visible && !control.destroyed ? control.render() : null;
});
