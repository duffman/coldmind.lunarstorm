/**
 * Dummy component theme: the tag names that have a component. No real theme
 * exists yet; `frontend::VISUAL` must list exactly these keys (a test in
 * `tests/frontend.rs` keeps the two equal), and `vcl.js` builds one placeholder
 * component per key.
 */
export const COMPONENTS = {
  form: 'form',
  panel: 'panel',
  groupbox: 'groupbox',
  scrollbox: 'scrollbox',
  pagecontrol: 'pagecontrol',
  tabsheet: 'tabsheet',
  splitter: 'splitter',
  toolbar: 'toolbar',
  statusbar: 'statusbar',
  label: 'label',
  image: 'image',
  progressbar: 'progressbar',
  button: 'button',
  edit: 'edit',
  memo: 'memo',
  checkbox: 'checkbox',
  radiogroup: 'radiogroup',
  combobox: 'combobox',
  trackbar: 'trackbar',
  listbox: 'listbox',
  listview: 'listview',
  column: 'column',
  treeview: 'treeview',
}
