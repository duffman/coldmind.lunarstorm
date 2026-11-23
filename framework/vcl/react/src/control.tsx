/**
 * What every VCL component shares: reading its properties through the mirror,
 * reporting events, and the common props of spec 2.4.
 *
 * Decisions:
 * - `align` is Delphi's docking mapped onto flexbox. A container lays its
 *   children out in one direction (`layout`, default `column`); `top`/`bottom`
 *   dock at the start/end of a column, `left`/`right` at the start/end of a row,
 *   and `client` takes the space that is left. It is done with CSS classes
 *   (`vcl-align-*`), so the look stays in the stylesheet.
 * - `visible={false}` is the `hidden` attribute, not unmounting: a hidden control
 *   keeps its DOM state (scroll position, caret).
 * - `popupMenu` is a native menu. The control only tells the host that the user
 *   asked for it (`onContextPopup`); the host owns and shows the menu.
 */
import { createContext, useCallback, useContext, useSyncExternalStore, type CSSProperties, type MouseEvent } from 'react';
import type { Mirror } from './mirror';

/** Where a control docks in its container (spec 2.4). */
export type Align = 'none' | 'top' | 'bottom' | 'left' | 'right' | 'client';

/** The props every visual control accepts (spec 2.4). */
export interface CommonProps {
  /** The control's name, unique within its form. */
  name: string;
  /** The control's caption; what it means depends on the control. */
  caption?: string;
  /** Where the control docks in its container. */
  align?: Align;
  /** Width in logical pixels. */
  width?: number;
  /** Height in logical pixels. */
  height?: number;
  /** `false` disables the control. */
  enabled?: boolean;
  /** `false` hides the control. */
  visible?: boolean;
  /** Tooltip text. */
  hint?: string;
  /** Position in the tab order. */
  tabOrder?: number;
  /** Name of the native popup menu shown on a context click. */
  popupMenu?: string;
}

/** Reports one event of a control: `(event, changedProps, args)`. */
export type Emit = (event: string, props?: Record<string, unknown>, args?: unknown) => void;

/** The mirror of the form being rendered. */
export const MirrorContext = createContext<Mirror | null>(null);

/**
 * Connects a component to its control.
 * @param literal the props from the view resource
 * @return the current props (literal, overridden by the mirror) and the event reporter
 */
export function useControl<P extends { name: string }>(literal: P): { props: P; emit: Emit } {
  const mirror = useContext(MirrorContext);
  if (!mirror) throw new Error(`<${literal.name}> is rendered outside a form`);
  const { name } = literal;
  const subscribe = useCallback((listener: () => void) => mirror.subscribe(name, listener), [mirror, name]);
  const read = useCallback(() => mirror.values(name), [mirror, name]);
  const live = useSyncExternalStore(subscribe, read, read);
  const emit = useCallback<Emit>((event, props, args) => mirror.emit(name, event, props, args), [mirror, name]);
  return { props: { ...literal, ...live }, emit };
}

/**
 * Joins class names, skipping the falsy ones.
 * @param names class names or falsy values
 * @return the class attribute
 */
export function cx(...names: (string | false | null | undefined)[]): string {
  return names.filter(Boolean).join(' ');
}

/** The DOM attributes the common props turn into. */
export interface Frame {
  /** `vcl-<kind>` and the docking class. */
  className: string;
  /** Fixed width and height, when set. */
  style: CSSProperties;
  /** The control's name, for styling and inspection. */
  'data-name': string;
  /** Set when `visible` is `false`. */
  hidden: boolean;
  /** The tooltip. */
  title: string | undefined;
  /** Asks the host for the popup menu; present only with `popupMenu`. */
  onContextMenu: ((event: MouseEvent) => void) | undefined;
}

/**
 * Turns the common props into DOM attributes.
 * @param kind the control's element name, e.g. `button`
 * @param props the control's current props
 * @param emit the control's event reporter
 * @param extra more class names
 * @return attributes to spread on the control's root element
 */
export function frame(kind: string, props: CommonProps, emit: Emit, ...extra: (string | false | undefined)[]): Frame {
  const style: CSSProperties = {};
  if (props.width !== undefined) style.width = props.width;
  if (props.height !== undefined) style.height = props.height;
  return {
    className: cx(`vcl-${kind}`, `vcl-align-${props.align ?? 'none'}`, props.enabled === false && 'vcl-disabled', ...extra),
    style,
    'data-name': props.name,
    hidden: props.visible === false,
    title: props.hint || undefined,
    onContextMenu: props.popupMenu
      ? event => {
          event.preventDefault();
          emit('onContextPopup');
        }
      : undefined,
  };
}
