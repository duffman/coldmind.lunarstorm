/**
 * The list controls of spec 2.4: listbox, listview with its columns, and
 * treeview.
 *
 * Decision: rows are data, not markup (spec 2.4). A `listview` receives `rows`
 * and a `treeview` receives `nodes` from the form class through the mirror; the
 * view resource declares only the columns.
 */
import { Children, isValidElement, useState, type KeyboardEvent, type ReactElement, type ReactNode } from 'react';
import { cx, frame, useControl, type CommonProps } from './control';
import type { ItemsProps } from './inputs';

/**
 * Moves a selection with the arrow, Home and End keys.
 * @param event the key press
 * @param index the selected index
 * @param count the number of items
 * @return the new index, or `undefined` when the key does not move the selection
 */
function step(event: KeyboardEvent, index: number, count: number): number | undefined {
  const next =
    event.key === 'ArrowDown' ? index + 1
    : event.key === 'ArrowUp' ? Math.max(0, index - 1)
    : event.key === 'Home' ? 0
    : event.key === 'End' ? count - 1
    : undefined;
  if (next === undefined || count === 0) return undefined;
  event.preventDefault();
  return Math.min(next, count - 1);
}

/** A list with one selected item; reports `onChange` with `itemIndex`. */
export function ListBox(literal: ItemsProps): ReactElement {
  const { props, emit } = useControl(literal);
  const items = props.items ?? [];
  const index = props.itemIndex ?? -1;
  const enabled = props.enabled !== false;
  /**
   * Selects an item and reports it.
   * @param next the item's index
   */
  const select = (next: number | undefined): void => {
    if (enabled && next !== undefined && next !== index) emit('onChange', { itemIndex: next });
  };
  return (
    <ul {...frame('listbox', props, emit)} role="listbox" tabIndex={enabled ? (props.tabOrder ?? 0) : -1}
        aria-label={props.caption} aria-disabled={!enabled || undefined} onKeyDown={event => select(step(event, index, items.length))}>
      {items.map((item, at) => (
        <li key={at} role="option" aria-selected={at === index} className={cx('vcl-item', at === index && 'vcl-item-selected')}
            onClick={() => select(at)}>
          {item}
        </li>
      ))}
    </ul>
  );
}

/** Props of `<column>`. */
export interface ColumnProps {
  /** The column's name. */
  name: string;
  /** The header text. */
  caption?: string;
  /** Width in logical pixels. */
  width?: number;
  /** How the column's cells are aligned. */
  alignment?: 'left' | 'center' | 'right';
}

/**
 * A column of a list view. It renders nothing itself: the list view reads its
 * props to draw the header and to align the cells.
 */
export function Column(_props: ColumnProps): null {
  return null;
}

/** Props of `<listview>`. */
export interface ListViewProps extends CommonProps {
  /** Index of the selected row; -1 for none. */
  itemIndex?: number;
  /** The rows, one string per column. Set by the form class, never in markup. */
  rows?: readonly (readonly string[])[];
  /** The `<column>` elements. */
  children?: ReactNode;
}

/**
 * A table with one selected row; reports `onChange` with `itemIndex`, and
 * `onDblClick` for a double click on a row.
 */
export function ListView(literal: ListViewProps): ReactElement {
  const { props, emit } = useControl(literal);
  const columns = Children.toArray(literal.children).filter(isValidElement<ColumnProps>).map(column => column.props);
  const rows = props.rows ?? [];
  const index = props.itemIndex ?? -1;
  const enabled = props.enabled !== false;
  /**
   * Selects a row and reports it.
   * @param next the row's index
   */
  const select = (next: number | undefined): void => {
    if (enabled && next !== undefined && next !== index) emit('onChange', { itemIndex: next });
  };
  return (
    <div {...frame('listview', props, emit)} tabIndex={enabled ? (props.tabOrder ?? 0) : -1}
         onKeyDown={event => select(step(event, index, rows.length))}>
      <table aria-label={props.caption}>
        <thead>
          <tr>
            {columns.map(column => (
              <th key={column.name} style={{ width: column.width, textAlign: column.alignment }}>{column.caption}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, at) => (
            <tr key={at} aria-selected={at === index} className={cx('vcl-item', at === index && 'vcl-item-selected')}
                onClick={() => select(at)} onDoubleClick={() => enabled && emit('onDblClick')}>
              {columns.map((column, cell) => (
                <td key={column.name} style={{ textAlign: column.alignment }}>{row[cell] ?? ''}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** One node of a tree view. */
export interface TreeNode {
  /** Identifies the node; unique within its tree. */
  id: string;
  /** The node's text. */
  caption: string;
  /** The node's children. */
  children?: readonly TreeNode[];
}

/** Props of `<treeview>`. */
export interface TreeViewProps extends CommonProps {
  /** The tree. Set by the form class, never in markup. */
  nodes?: readonly TreeNode[];
  /** The `id` of the selected node. */
  selected?: string;
}

/**
 * A tree with one selected node; reports `onChange` with `selected`, and
 * `onExpanded` with `{ id, expanded }` when a node is opened or closed. Which
 * nodes are open is view state the user controls, so it is kept here.
 */
export function TreeView(literal: TreeViewProps): ReactElement {
  const { props, emit } = useControl(literal);
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const enabled = props.enabled !== false;
  /**
   * Opens or closes a node and reports it.
   * @param id the node's id
   */
  const toggle = (id: string): void => {
    const next = new Set(open);
    const expanded = !next.delete(id);
    if (expanded) next.add(id);
    setOpen(next);
    emit('onExpanded', undefined, { id, expanded });
  };
  /**
   * Renders one level of the tree.
   * @param nodes the nodes of the level
   * @return the list, or `null` for an empty level
   */
  const level = (nodes: readonly TreeNode[] | undefined): ReactElement | null =>
    nodes?.length ? (
      <ul role="group">
        {nodes.map(node => (
          <li key={node.id} role="treeitem" aria-selected={node.id === props.selected}
              aria-expanded={node.children?.length ? open.has(node.id) : undefined}>
            <div className={cx('vcl-item', node.id === props.selected && 'vcl-item-selected')}
                 onClick={() => enabled && node.id !== props.selected && emit('onChange', { selected: node.id })}>
              {node.children?.length ? (
                <button type="button" className="vcl-tree-toggle" aria-label={open.has(node.id) ? 'Collapse' : 'Expand'}
                        disabled={!enabled} onClick={event => { event.stopPropagation(); toggle(node.id); }}>
                  {open.has(node.id) ? '▾' : '▸'}
                </button>
              ) : <span className="vcl-tree-toggle" />}
              {node.caption}
            </div>
            {open.has(node.id) ? level(node.children) : null}
          </li>
        ))}
      </ul>
    ) : null;
  return (
    <div {...frame('treeview', props, emit)} role="tree" aria-label={props.caption} tabIndex={enabled ? (props.tabOrder ?? 0) : -1}>
      {level(props.nodes)}
    </div>
  );
}
