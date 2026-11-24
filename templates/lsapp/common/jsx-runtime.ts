import type { Control, ControlProps } from './controls';
import type { ControlType } from './decorators';

export interface LSNode {
  readonly type: ControlType;
  readonly props: Readonly<ControlProps & Record<string, unknown>>;
  readonly children: readonly LSNode[];
}
export type LSChild = LSNode | readonly LSChild[] | null | undefined | false;

export function h(type: ControlType, props: Record<string, unknown> | null, ...children: LSChild[]): LSNode {
  const flattened: LSNode[] = [];
  const append = (child: LSChild): void => {
    if (!child) return;
    if (Array.isArray(child)) { for (const item of child) append(item); }
    else flattened.push(child as LSNode);
  };
  children.forEach(append);
  return { type, props: props ?? {}, children: flattened };
}

export function Fragment(): never { throw new Error('A view layout requires one root control.'); }

declare global {
namespace JSX {
  export type Element = LSNode;
  export interface ElementClass extends Control {}
  export interface ElementChildrenAttribute { children: {}; }
  export interface IntrinsicElements {}
}

}
