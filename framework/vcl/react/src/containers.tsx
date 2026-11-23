/**
 * The container controls of spec 2.4: form, panel, groupbox, scrollbox,
 * pagecontrol, tabsheet, splitter, toolbar and statusbar.
 */
import {
  Children,
  createContext,
  isValidElement,
  useContext,
  useRef,
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent,
  type ReactElement,
  type ReactNode,
} from 'react';
import { cx, frame, useControl, type CommonProps } from './control';

/** Props of the containers that lay children out (spec 2.4). */
export interface LayoutProps extends CommonProps {
  /** The direction children are laid out in; `column` when absent. */
  layout?: 'row' | 'column';
  /** Space between children in logical pixels. */
  gap?: number;
  /** Space around the children in logical pixels. */
  padding?: number;
  /** The contained controls. */
  children?: ReactNode;
}

/**
 * Turns the layout props into a style.
 * @param props the container's props
 * @param base the style from the common props
 * @return the container's style
 */
function layoutStyle(props: LayoutProps, base: CSSProperties): CSSProperties {
  const style = { ...base };
  if (props.gap !== undefined) style.gap = props.gap;
  if (props.padding !== undefined) style.padding = props.padding;
  return style;
}

/** Props of `<form>` that the WebView renders; the rest are the native window's. */
export interface FormProps {
  /** The form's name. */
  name: string;
  /** CSS cursor name over the form. */
  cursor?: string;
  /** The form's controls. */
  children?: ReactNode;
}

/**
 * The form's client area. The window itself (caption, size, menu) is native and
 * created by the host from the host configuration (spec 6).
 *
 * Enter clicks the button marked `default` and Escape the one marked `cancel`,
 * as in Delphi, unless the focused control uses the key itself.
 */
export function Form(props: FormProps): ReactElement {
  /**
   * Routes Enter and Escape to the default and cancel buttons.
   * @param event the key press
   */
  const keyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    const marker = event.key === 'Enter' ? 'data-default' : event.key === 'Escape' ? 'data-cancel' : null;
    if (!marker || event.defaultPrevented) return;
    const target = event.target as HTMLElement;
    // A memo uses Enter for a new line and a button clicks itself on Enter.
    if (event.key === 'Enter' && (target.tagName === 'TEXTAREA' || target.tagName === 'BUTTON')) return;
    const button = event.currentTarget.querySelector<HTMLButtonElement>(`button[${marker}]:not(:disabled)`);
    if (!button) return;
    event.preventDefault();
    button.click();
  };
  return (
    <div className="vcl-form" data-name={props.name} data-layout="column" style={{ cursor: props.cursor }} onKeyDown={keyDown}>
      {props.children}
    </div>
  );
}

/** A plain container. Its caption is not drawn. */
export function Panel(literal: LayoutProps): ReactElement {
  const { props, emit } = useControl(literal);
  const attributes = frame('panel', props, emit);
  return (
    <div {...attributes} data-layout={props.layout ?? 'column'} style={layoutStyle(props, attributes.style)}>
      {literal.children}
    </div>
  );
}

/** A container that draws a titled frame. */
export function GroupBox(literal: LayoutProps): ReactElement {
  const { props, emit } = useControl(literal);
  const attributes = frame('groupbox', props, emit);
  return (
    <fieldset {...attributes} disabled={props.enabled === false}>
      {props.caption ? <legend>{props.caption}</legend> : null}
      <div className="vcl-groupbox-body" data-layout={props.layout ?? 'column'} style={layoutStyle(props, {})}>
        {literal.children}
      </div>
    </fieldset>
  );
}

/** A container that scrolls its content. */
export function ScrollBox(literal: LayoutProps): ReactElement {
  const { props, emit } = useControl(literal);
  const attributes = frame('scrollbox', props, emit);
  return (
    <div {...attributes} data-layout={props.layout ?? 'column'} style={layoutStyle(props, attributes.style)}>
      {literal.children}
    </div>
  );
}

/** Props of `<pagecontrol>`. */
export interface PageControlProps extends CommonProps {
  /** Name of the tab sheet that is shown; the first one when absent. */
  activePage?: string;
  /** The `<tabsheet>` elements. */
  children?: ReactNode;
}

/** The name of the page a page control shows, for its tab sheets. */
const ActivePage = createContext<string | undefined>(undefined);

/**
 * A row of tabs over one visible page. Selecting a tab reports `onChange` with
 * the new `activePage`.
 */
export function PageControl(literal: PageControlProps): ReactElement {
  const { props, emit } = useControl(literal);
  const sheets = Children.toArray(literal.children).filter(isValidElement<LayoutProps>);
  const active = props.activePage ?? sheets[0]?.props.name;
  return (
    <div {...frame('pagecontrol', props, emit)}>
      <div className="vcl-tabs" role="tablist">
        {sheets.map(sheet => (
          <Tab key={sheet.props.name} sheet={sheet.props} selected={sheet.props.name === active} enabled={props.enabled !== false}
               select={() => emit('onChange', { activePage: sheet.props.name })} />
        ))}
      </div>
      <ActivePage.Provider value={active}>{literal.children}</ActivePage.Provider>
    </div>
  );
}

/**
 * One tab of a page control. It reads its sheet's live caption, so a caption the
 * form class changes shows on the tab.
 * @param props `sheet`: the sheet's literal props; `selected`: whether it is the
 *   active page; `enabled`: whether the page control is enabled; `select`: called
 *   when the user picks the tab
 */
function Tab(props: { sheet: LayoutProps; selected: boolean; enabled: boolean; select: () => void }): ReactElement | null {
  const sheet = useControl(props.sheet).props;
  if (sheet.visible === false) return null;
  return (
    <button type="button" role="tab" className={cx('vcl-tab', props.selected && 'vcl-tab-active')} aria-selected={props.selected}
            disabled={!props.enabled || sheet.enabled === false} onClick={() => !props.selected && props.select()}>
      {sheet.caption ?? sheet.name}
    </button>
  );
}

/** One page of a page control; shown only while it is the active page. */
export function TabSheet(literal: LayoutProps): ReactElement {
  const { props, emit } = useControl(literal);
  const active = useContext(ActivePage);
  const attributes = frame('tabsheet', { ...props, align: 'client' }, emit);
  return (
    <div {...attributes} role="tabpanel" hidden={attributes.hidden || active !== props.name}
         data-layout={props.layout ?? 'column'} style={layoutStyle(props, attributes.style)}>
      {literal.children}
    </div>
  );
}

/**
 * Sits between two aligned siblings and resizes the one before it by dragging.
 * The new size is layout the user chose, not a control property, so it is not
 * reported to the backend.
 */
export function Splitter(literal: CommonProps): ReactElement {
  const { props, emit } = useControl(literal);
  const start = useRef<{ at: number; size: number; horizontal: boolean; target: HTMLElement } | null>(null);
  /**
   * Starts a drag.
   * @param event the pointer press
   */
  const down = (event: PointerEvent<HTMLDivElement>): void => {
    const target = event.currentTarget.previousElementSibling as HTMLElement | null;
    const parent = event.currentTarget.parentElement;
    if (!target || !parent || props.enabled === false) return;
    const horizontal = parent.dataset.layout === 'row';
    const box = target.getBoundingClientRect();
    start.current = { at: horizontal ? event.clientX : event.clientY, size: horizontal ? box.width : box.height, horizontal, target };
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  /**
   * Resizes the sibling while dragging.
   * @param event the pointer move
   */
  const move = (event: PointerEvent<HTMLDivElement>): void => {
    const drag = start.current;
    if (!drag) return;
    const size = Math.max(24, drag.size + (drag.horizontal ? event.clientX : event.clientY) - drag.at);
    drag.target.style.flex = `0 0 ${size}px`;
  };
  return (
    <div {...frame('splitter', props, emit)} role="separator" onPointerDown={down} onPointerMove={move}
         onPointerUp={() => (start.current = null)} />
  );
}

/** A row of buttons. */
export function ToolBar(literal: CommonProps & { children?: ReactNode }): ReactElement {
  const { props, emit } = useControl(literal);
  return (
    <div {...frame('toolbar', { ...props, align: props.align ?? 'top' }, emit)} role="toolbar" data-layout="row">
      {literal.children}
    </div>
  );
}

/** A status line; docks at the bottom unless `align` says otherwise. */
export function StatusBar(literal: CommonProps): ReactElement {
  const { props, emit } = useControl(literal);
  return (
    <div {...frame('statusbar', { ...props, align: props.align ?? 'bottom' }, emit)} role="status">
      {props.caption}
    </div>
  );
}
