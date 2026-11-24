# @desktop/forms

Backend runtime of a model project (spec 2.5, 4c). A developer writes `class Main extends Form` with no constructor and
fields like `save = this.find('Save')`; this package supplies `Form`, the control proxies those fields return, and the
dispatch that routes host messages to the connected handlers. Plain JS with JSDoc, no build step.

## Public API

`exports` is `"."` -> `forms.js`:

- `Form`: base class. `find(name)` returns the control's proxy or throws `<Form> has no control named '<name>'`; `name`
  is the class name. Constructing a class not in the project throws `'X' is not a form of this application`.
- `Control`: proxy base, `name`, `kind`, `get(prop)`, `set(prop, value)`, `receive(props)`.
- `start({ application, forms })`: the generated backend entry calls it once. Creates every form and calls `serve`.
- `APPLICATION` (`'@application'`): the `windowId` of application events.

```js
class Main extends Form {
  save = this.find('Save');
  name = this.find('Name');
  clicked = () => log.push(`clicked:${this.name.text}`);
}
start({
  application: { mainForm: 'Main', connections: {} },
  forms: { Main: { create: Main, controls: { Save: ['button', { caption: 'Save' }] }, connections: { 'Save.onClick': ['clicked'] } } },
});
```

## How it fits

Imports `restored`, `serve`, `setProperty` from `@desktop/bridge`, which is not in this workspace (no `package.json`
declares it); the tests replace it with `mock.module`. Spec 4c emits the `FormDefinition` tables, 4d is the bridge: a
write on a proxy goes to the form's WebView, where `@lunarstorm/vcl` (`framework/vcl/react`) mirrors it; a user event
comes back, updates the proxy, then runs the handlers. No workspace package depends on it. Not the same VCL as
`@coldmind/vcl.*` (owner tree of live component objects); here the controls are proxies of a WebView's state.

## Scripts

- `pnpm --filter @desktop/forms test` runs `bun test` (`forms.test.js`, 10 tests, bridge mocked).
- No `typecheck` or `build` script.

## Decisions and hazards

- Form lookup is by class name, so minifying form class names breaks `new.target.name`.
- `Control` keeps the owning form's name in a private field; a form class that declares a field called `name` would
  otherwise shadow `Form.name` and send writes to the wrong window. `Form.linked` and `Form.control` are static and read
  private tables for the same reason.
- Writing a value equal to the current one (`Object.is`) sends nothing. Values from a WebView event (`receive`) are not
  sent back.
- `onCloseQuery` is a request: handlers get `{ cancel }` and the host receives `{ cancel: Boolean(...) }`, after awaiting
  async handlers.
- A control linked to an action (`caption`, `enabled`, `checked`) follows the action's writes; its `onExecute` runs after
  the control's own click handlers.
- Module-level maps (`definitions`, `forms`, `proxies`) mean one project per process; `start` clears the first two.
- Proxy classes are built once per kind from the first form's prop list: a later form with extra props for the same kind
  has no accessor for them (use `get`/`set`).
- Unknown forms, targets and unconnected events are ignored silently.
