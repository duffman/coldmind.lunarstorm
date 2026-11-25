// Dummy WebView runtime: no React, no real components. No real theme exists yet;
// this gives the generated pages the `h`, `C` and `mount` they import.

/** The visual tags; kept equal to `COMPONENTS` in `index.ts`. */
const TAGS = [
  'form', 'panel', 'groupbox', 'scrollbox', 'pagecontrol', 'tabsheet', 'splitter', 'toolbar', 'statusbar', 'label',
  'image', 'progressbar', 'button', 'edit', 'memo', 'checkbox', 'radiogroup', 'combobox', 'trackbar', 'listbox',
  'listview', 'column', 'treeview',
];

/**
 * Creates an element.
 * @param {Function} type the component
 * @param {object} props the props
 * @param {...object} children the child elements
 * @return {{type: Function, props: object}} the element
 */
export const h = (type, props, ...children) => ({ type, props: children.length ? { ...props, children } : { ...props } });

/** One placeholder component per tag: a `div` carrying the control's name. */
export const C = Object.fromEntries(
  TAGS.map(tag => [tag, ({ name, children } = {}) => ({ type: 'div', props: { 'data-name': name, className: `vcl-${tag}`, children } })]),
);

/**
 * Mounts the page's form. Does nothing.
 * @param {Function} _load loads a form module by name
 */
export const mount = _load => {};
