# @lunarstorm/vcl

The React side of the VCL for model projects (spec 2.4): one stateless React component per visual control of the model
(`panel`, `button`, `edit`, `listview`, ...), the property mirror that feeds them, and the `Shell` / `ShellDebug` wrappers
around a generated form. Controls hold no state of their own; every value comes from the view resource's literal props,
overridden by what the backend wrote.

## Public API

`src/index.ts` (stylesheet is the separate `@lunarstorm/vcl/vcl.css`, the only `sideEffects` file):

- `COMPONENTS`: element name -> component (23 entries: `form panel groupbox scrollbox pagecontrol tabsheet splitter
  toolbar statusbar label image progressbar button edit memo checkbox radiogroup combobox trackbar listbox listview column
  treeview`). Native ones (`mainmenu`, `popupmenu`, `menuitem`, `actionlist`, `action`, `trayicon`, `timer`) are absent.
- Components: `Form, Panel, GroupBox, ScrollBox, PageControl, TabSheet, Splitter, ToolBar, StatusBar, Label, Image,
  ProgressBar, Button, Edit, Memo, CheckBox, RadioGroup, ComboBox, TrackBar, ListBox, ListView, Column, TreeView`.
- `Mirror`, `Bridge`, `MirrorContext`, `useControl`, `CommonProps`, `Align`, `Shell`, `ShellDebug`.

```tsx
const bridge = window.zebra;                       // host-injected Bridge
const ShellView = bridge.mode === "release" ? Shell : ShellDebug;
root.render(<ShellView bridge={bridge} forms={{ Main: MainForm }} />);
// MainForm, as 4b generates it:
<Form name="Main"><Button name="Save" caption="Save" default /><ListView name="Files"><Column name="c1" caption="Name" /></ListView></Form>
```

## How it fits

No workspace dependencies; `react` and `react-dom` >= 19 are peers. `templates/zebra.framework` bundles it into
`dist/vcl.js` (its `src/main.tsx` imports `COMPONENTS`, `Shell`, `ShellDebug`, `Bridge`) and every build copies that into
`frontend/`. Serves spec 2.4 (controls), 4b (frontend generation), 4d (bridge) and 6 (shell, `window.zebra`). Its backend
twin is `@desktop/forms`: a proxy write there becomes a `Bridge.onUpdate` here; a user event goes back through
`Mirror.emit`. A different stack from `@coldmind/vcl.*`, which is the class-based, live-object VCL.

## Scripts

- `pnpm --filter @lunarstorm/vcl test` runs `bun test` (`src/vcl.test.tsx`, server-side render with a fake bridge).
- `pnpm --filter @lunarstorm/vcl typecheck` runs `tsc --noEmit -p tsconfig.json`. No `build` script.

## Decisions and hazards

- The mirror is the only control state: `useControl(literal)` returns `{ ...literal, ...mirrorValues }` via
  `useSyncExternalStore`, so a backend write and a user edit cannot disagree.
- `Mirror.#merge` skips a value equal (`Object.is`) to the current one: the echo of the control's own edit must not re-render
  or the text caret resets. `emit` mirrors the changed values first, then sends.
- Inputs always report their event with the changed values (`onChange` + `text`, `onClick` + `checked`), connected
  handler or not, because the backend proxy must hold the current value.
- `align` is flexbox, via `vcl-align-*` classes and `data-layout`. Fix recorded in `vcl.css`: align-less containers in a
  row used to be centred and shrank, giving their `client` child no height; only leaf controls are centred now.
- `visible={false}` sets `hidden`, not unmount, to keep scroll and caret. `popupMenu` only emits `onContextPopup`; the
  host shows the native menu.
- Rows are data: `listview` takes `rows` and `treeview` takes `nodes` through the mirror; the view resource declares columns only.
- `Shell` shows one sentence on a render error; `ShellDebug` adds the stack (release builds must not leak paths). A window
  whose form was not generated throws inside the boundary instead of rendering empty.
- A control rendered outside a `MirrorContext` throws `<name> is rendered outside a form`.
