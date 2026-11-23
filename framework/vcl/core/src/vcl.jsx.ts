/**
 * TSX that builds controls: a form's layout as markup, as a DFM is in Delphi. A file opts in with a
 * `@jsxImportSource @coldmind/vcl.core` pragma; its tags are `Control` classes, its attributes their
 * properties and `on*` events, its children child controls. JSX only describes; `build` creates.
 *
 * Controls' own `render()` keeps React's JSX: this runtime is only for files that carry the pragma.
 * @example
 * build(this, <>
 *   <Panel align="top" direction="row">
 *     <TextEdit ref={c => (this.input = c)} className="vcl-grow" />
 *     <Button caption="Add" onClick={() => this.add()} />
 *   </Panel>
 * </>);
 */
import type { Assignable, Component } from "@coldmind/vcl.core/src/vcl.component.ts";
import type { Control } from "@coldmind/vcl.core/src/vcl.control.tsx";

/** A tag: a concrete control class. */
export type ControlClass<C extends Control = Control> = new (owner: Component, name?: string) => C;

export const Fragment: unique symbol = Symbol.for("@coldmind/vcl.core.fragment");

export type ElementProps = {
  readonly children?: VclChildren;
  readonly ref?: (control: Control) => unknown;
  readonly name?: string;
  readonly [key: string]: unknown;
};

/** What a tag evaluates to: a description, not a control. */
export type VclElement = { readonly type: ControlClass | typeof Fragment; readonly props: ElementProps };
export type VclChild = VclElement | boolean | null | undefined;
export type VclChildren = VclChild | readonly VclChildren[];

/** The attributes of a control's tag. */
export type JsxProps<C extends Control> = Omit<Assignable<C>, "parent"> & {
  /** Called with the control once it and its children exist. Method syntax: a `Form` stays a `Control`. */
  ref?(control: C): unknown;
  children?: VclChildren;
};

export function jsx(type: ControlClass | typeof Fragment, props: ElementProps): VclElement {
  return { type, props };
}

export { jsx as jsxs, jsx as jsxDEV };

/**
 * Creates the controls `tree` describes, owned by `owner` and placed in `parent`, each with its
 * attributes set in one batch. Returns the top-level controls.
 */
export function build(parent: Control, tree: VclChildren, owner: Component = parent): Control[] {
  const created: Control[] = [];
  for (const element of elements(tree)) {
    const { children, ref, name, ...values } = element.props;
    if (element.type === Fragment) {
      created.push(...build(parent, children, owner));
      continue;
    }
    const control = new element.type(owner, name ?? "");
    control.beginUpdate();
    try {
      for (const [key, value] of Object.entries(values)) Reflect.set(control, key, value);
      control.parent = parent;
    } finally {
      control.endUpdate();
    }
    build(control, children, owner);
    ref?.(control);
    created.push(control);
  }
  return created;
}

const isList = (tree: VclChildren): tree is readonly VclChildren[] => Array.isArray(tree);

function* elements(tree: VclChildren): Generator<VclElement> {
  if (isList(tree)) for (const t of tree) yield* elements(t);
  else if (typeof tree === "object" && tree !== null) yield tree;
}

export declare namespace JSX {
  type Element = VclElement;
  type ElementClass = Control;
  /** The tag's attributes are the instance's `jsxProps` type. */
  interface ElementAttributesProperty {
    jsxProps: {};
  }
  interface ElementChildrenAttribute {
    children: {};
  }
  interface IntrinsicElements {}
  interface IntrinsicAttributes {}
}
