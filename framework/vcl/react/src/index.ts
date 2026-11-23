/**
 * The React VCL: one component per visual control of the model (spec 2.4), the
 * property mirror that feeds them, and the shells that wrap a generated form.
 *
 * The non-visual components (`mainmenu`, `popupmenu`, `menuitem`, `actionlist`,
 * `action`, `trayicon`, `timer`) have no component here: they are native, created
 * by the host from the host configuration.
 */
export { Mirror, type Bridge, type Values } from './mirror';
export { MirrorContext, useControl, type Align, type CommonProps, type Emit } from './control';
export {
  Form, GroupBox, PageControl, Panel, ScrollBox, Splitter, StatusBar, TabSheet, ToolBar,
  type FormProps, type LayoutProps, type PageControlProps,
} from './containers';
export {
  Button, CheckBox, ComboBox, Edit, Image, Label, Memo, ProgressBar, RadioGroup, TrackBar,
  type ButtonProps, type CheckBoxProps, type EditProps, type ImageProps, type ItemsProps, type MemoProps, type RangeProps,
} from './inputs';
export {
  Column, ListBox, ListView, TreeView,
  type ColumnProps, type ListViewProps, type TreeNode, type TreeViewProps,
} from './lists';
export { Shell, ShellDebug, type ShellProps } from './shell';

import { Form, GroupBox, PageControl, Panel, ScrollBox, Splitter, StatusBar, TabSheet, ToolBar } from './containers';
import { Button, CheckBox, ComboBox, Edit, Image, Label, Memo, ProgressBar, RadioGroup, TrackBar } from './inputs';
import { Column, ListBox, ListView, TreeView } from './lists';

/**
 * The component of each element name. The frontend generator (spec 4b) emits one
 * of these per element of a view resource; an element name missing here is a
 * non-visual component and is not rendered.
 */
export const COMPONENTS = {
  form: Form,
  panel: Panel,
  groupbox: GroupBox,
  scrollbox: ScrollBox,
  pagecontrol: PageControl,
  tabsheet: TabSheet,
  splitter: Splitter,
  toolbar: ToolBar,
  statusbar: StatusBar,
  label: Label,
  image: Image,
  progressbar: ProgressBar,
  button: Button,
  edit: Edit,
  memo: Memo,
  checkbox: CheckBox,
  radiogroup: RadioGroup,
  combobox: ComboBox,
  trackbar: TrackBar,
  listbox: ListBox,
  listview: ListView,
  column: Column,
  treeview: TreeView,
} as const;
