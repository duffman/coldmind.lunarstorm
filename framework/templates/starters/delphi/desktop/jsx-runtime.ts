import type { ButtonOptions, ControlOptions, EditOptions, ListBoxOptions, PanelOptions } from './controls';
export interface ControlDefinition { readonly type: keyof ControlElements; readonly props: Record<string, unknown>; readonly children: readonly ControlDefinition[]; }
export interface ControlElements {
  panel: PanelOptions;
  label: ControlOptions;
  statusbar: ControlOptions;
  button: ButtonOptions;
  edit: EditOptions;
  memo: EditOptions;
  listbox: ListBoxOptions;
  combobox: ListBoxOptions;
}
export function h(type: keyof ControlElements, props: Record<string, unknown> | null, ...children: (ControlDefinition | ControlDefinition[] | null | false)[]): ControlDefinition {
  if (!(type in elementNames)) throw new Error(`Unknown desktop control: ${type}`);
  return { type, props: props ?? {}, children: children.flat().filter((child): child is ControlDefinition => Boolean(child)) };
}
const elementNames = { panel: true, label: true, statusbar: true, button: true, edit: true, memo: true, listbox: true, combobox: true };
export function jsx(type: keyof ControlElements, props: Record<string, unknown>): ControlDefinition {
  const { children, ...options } = props;
  return h(type, options, ...(children ? [children as ControlDefinition | ControlDefinition[]] : []));
}
export const jsxs = jsx;
export const jsxDEV = jsx;
export namespace h {
  export namespace JSX {
    export type Element = ControlDefinition;
    export interface ElementChildrenAttribute { children: unknown; }
    export type IntrinsicElements = { [K in keyof ControlElements]: ControlElements[K] & { children?: ControlDefinition | ControlDefinition[] } };
  }
}
export namespace JSX {
  export type Element = ControlDefinition;
  export interface ElementChildrenAttribute { children: unknown; }
  export type IntrinsicElements = h.JSX.IntrinsicElements;
}
