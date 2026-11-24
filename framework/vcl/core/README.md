# @coldmind/vcl.core

The kernel of the Delphi-style VCL: `Component` (owner tree, `@prop`, change notification), `Control` (a component with a
place on screen), `ControlHost` (the one bridge to React), `Kernel` (per-process registry, root injector, in-process bus)
and a TSX runtime that builds control trees. State and behaviour live on instances; React only renders them.

## Public API

`src/index.ts`, plus subpath exports `./jsx-runtime` and `./jsx-dev-runtime` (both `src/vcl.jsx.ts`):

- `Component(owner?, name?)`: `components`, `findComponent`, `changed`, `beginUpdate`/`endUpdate`, `assign(values)`, `own(cleanup)`,
  `loaded()`, `handleException`, `destroy`, and `subscribe`/`version` for `useSyncExternalStore`.
- `prop` (accessor decorator), `load(component, container?)`, `propsOf`, `describe` (owner tree as `ComponentDefinition`).
- `Control`: `visible`, `enabled`, `align`, `className`, `parent`, `controls`, abstract `render()`, `renderControls()`;
  `ControlHost`, `useComponent`, `Align`.
- `Kernel` / `kernel`: `container` (`@coldmind/inject`), `bus` (`SignalBus`), `onException`, `onTrace`.
- `build(parent, tree, owner?)`, types `ControlClass`, `JsxProps`, `VclElement`; `css`, `cx`, `installStyles(document)`.

```tsx
class Button extends Control {
  @prop accessor caption = "";
  onClick: ((sender: Button) => unknown) | undefined;
  render() { return <button onClick={() => this.invoke(this.onClick, this)}>{this.caption}</button>; }
}
// in a file with `@jsxImportSource @coldmind/vcl.core`:
build(this, <Panel align="top"><Button caption="Add" onClick={() => this.add()} /></Panel>);
```

## How it fits

Depends on `@coldmind/inject` and `@coldmind/bus`; `react` >= 19 is a peer. `@coldmind/vcl.app` (Form, Application)
and `@coldmind/vcl.common` (Button, TextEdit, ...) are built on it. This is the class-based VCL: live component objects,
as in Delphi. It is a separate stack from `@lunarstorm/vcl` (`framework/vcl/react`), the stateless React components that
render the spec's view resources (spec 2.4, 6); `doc/plans/main.md` has no section for this one.

## Scripts

- `pnpm --filter @coldmind/vcl.core typecheck` runs `tsc --noEmit -p tsconfig.json`.
- No `test` or `build` script; no tests in the package.

## Decisions and hazards

- `Control.render()` must use no React state and no hooks; all state is on the instance. `ControlHost` is `memo`ed and
  subscribes to its control only, so a property change re-renders that control, not the tree.
- `@prop` notifies only when `Object.is` says the value changed; arrays and objects need a new identity (`ListBox.items`).
- `destroy` flips `destroyed` first, so a dying subtree no longer notifies; owned components are destroyed last-created
  first, then `own` cleanups in reverse; a cleanup that throws goes to `handleException`, not out.
- Exceptions route up the owner chain to the `Application`, else `kernel.handleException` (`console.error`).
  `invoke` also catches rejected promises from event handlers.
- `endUpdate` without `beginUpdate` throws; `parent = self or descendant` throws.
- `assign` and TSX attributes accept data properties and `on*` slots, not methods (`Assignable`).
- The `build` TSX runtime is opt-in per file by pragma; `render()` keeps React's JSX. The `build` doc example uses a
  `Panel` with `direction="row"`, but no Panel class or `direction` prop exists in `vcl.core`, `vcl.app` or `vcl.common`
  (the stylesheet does define `.vcl-panel` and `.vcl-dir-row`).
- `Kernel.instance` is a module singleton: one kernel per JS realm.
