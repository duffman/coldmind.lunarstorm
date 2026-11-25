/**
 * Dummy component theme: one placeholder component per visual tag, so the
 * pipeline builds and its tests run while no real component theme exists.
 *
 * Decision: each component renders nothing. The pipeline only needs the set
 * of tag names; `frontend::VISUAL` must list exactly the keys below (a test in
 * `tests/frontend.rs` keeps the two equal).
 */

/**
 * A placeholder component.
 * @return {null} nothing is rendered
 */
const Dummy = (): null => null;

export const COMPONENTS = {
  form: Dummy,
  panel: Dummy,
  groupbox: Dummy,
  scrollbox: Dummy,
  pagecontrol: Dummy,
  tabsheet: Dummy,
  splitter: Dummy,
  toolbar: Dummy,
  statusbar: Dummy,
  label: Dummy,
  image: Dummy,
  progressbar: Dummy,
  button: Dummy,
  edit: Dummy,
  memo: Dummy,
  checkbox: Dummy,
  radiogroup: Dummy,
  combobox: Dummy,
  trackbar: Dummy,
  listbox: Dummy,
  listview: Dummy,
  column: Dummy,
  treeview: Dummy,
}
