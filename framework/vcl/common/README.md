# @coldmind/vcl.common

The common controls of the Delphi-style VCL, each a `Control` subclass from `@coldmind/vcl.core` with `@prop` properties,
`on*` event slots and a `render()` to plain DOM: `Button`, `Label`, `TextEdit`, `Memo`, `CheckBox`, `ListBox<T>`. Each
control is about 15 to 80 lines; styling is the `vcl-*` classes of the core stylesheet.

## Public API

`src/index.ts` re-exports all six; subpaths `./button`, `./label`, `./text`, `./memo`, `./checkbox`, `./listbox` load one.

| Class | Properties | Events |
|---|---|---|
| `Button` | `caption`, `kind` (`normal` / `primary`), `modalResult`; `click()` | `onClick` |
| `Label` | `caption`, `tone` (`normal` / `muted` / `danger`) | none |
| `TextEdit` | `text`, `placeholder`, `password`, `readOnly`, `maxLength`, `autoFocus` | `onChange`, `onEnter` |
| `Memo` | `text`, `placeholder`, `readOnly`, `rows` (4) | `onChange` |
| `CheckBox` | `caption`, `checked` | `onChange` |
| `ListBox<T>` | `items` (set a new array), `itemIndex` (-1), `emptyText`, `format`; `selected`, `select(i)` | `onSelect`, `onActivate` |

```tsx
// @jsxImportSource @coldmind/vcl.core
build(this, <>
  <TextEdit ref={c => (this.input = c)} placeholder="Note" />
  <Button caption="Add" kind="primary" onClick={() => this.add(this.input.text)} />
  <ListBox<string> ref={c => (this.list = c)} emptyText="No notes" onActivate={l => this.open(l.selected)} />
</>);
```

## How it fits

Depends on `@coldmind/vcl.core` (`Control`, `prop`, `cx`) and `@coldmind/vcl.app` (`Form`, `ModalResult`, used by `Button`);
`react` >= 19 is a peer. No workspace package imports it yet. It is the class-based counterpart of the stateless
`@lunarstorm/vcl` components (`framework/vcl/react`, spec 2.4 table); only six of that table's controls exist here.
`doc/plans/main.md` does not describe this package.

## Scripts

- `pnpm --filter @coldmind/vcl.common typecheck` runs `tsc --noEmit -p tsconfig.json`.
- No `test` or `build` script; no tests in the package.

## Decisions and hazards

- `Button.click()` does nothing when disabled; with a `modalResult` other than `"none"` it walks up `parent` to the first
  `Form` and sets the result there, which closes a modal form (Delphi `ModalResult` on a button).
- Edit controls write the DOM value into `text`/`checked` before calling `onChange`, so handlers read current state from
  the instance, never from the event.
- `ListBox.items` is notified by identity: mutating the array in place does not re-render. Shrinking `items` clamps
  `itemIndex`. `select` clamps to `-1..length-1` and fires `onSelect` only on a change. Arrow keys move the selection;
  Enter and double-click fire `onActivate`.
- Event slots are called through `invoke`, so a throwing or rejecting handler goes to `handleException` (the
  `Application`'s error banner) instead of breaking React.
- No container control (Panel) lives here, although `vcl.core`'s stylesheet has `.vcl-panel` and the `build` doc example
  uses `<Panel>`.
- `ListBox.format` is a plain field, not a `@prop`: reassigning it does not re-render.
