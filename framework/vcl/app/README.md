# @coldmind/vcl.app

The application layer of the Delphi-style VCL: `Form` (a top-level control with `show`, `showModal`, `close`, an error
boundary) and `Application` (owns the forms, mounts the React root, draws main form, windows, modals and unhandled errors),
plus `provideClient`, which puts the network connection and the server's remote controllers into the injector.

## Public API

`src/index.ts` exports `vcl.form.tsx`, `vcl.application.tsx`, `vcl.client.ts`:

- `Form`: `caption`, `onShow`, `onCloseQuery(sender) => boolean`, `onClose(sender, action) => CloseAction | void`,
  `show()`, `showModal(): Promise<ModalResult>`, `close()`, `modalResult`, `renderClient()`. Types `ModalResult`
  (`ok cancel yes no abort retry close none`), `CloseAction` (`hide free none`), abstract `FormHost`, `messageOf`.
- `Application(name?)`: `title`, `mainForm`, `forms`, `createForm(FormClass)`, `initialize()`, `run(MainForm?, container?)`,
  `terminate()`, `onException`, `dismissError(id)`, `mainFormClass`.
- `provideClient(client, controllers, container?)`, `remoteToken(Controller)`, `ConnectionState` (token of `Action<ClientState>`).

```ts
class Main extends Form { @Inject(remoteToken(NotesController)) notes!: Remote<NotesController>; }
provideClient(new Client(wsLink(url)), [NotesController]);
await new Application("notes").run(Main);   // resolves when the main form closes
```

## How it fits

Depends on `@coldmind/vcl.core`, `inject`, `rpc`, `net.client`, `wire`; `react` and `react-dom` >= 19 are peers.
`@coldmind/vcl.common` imports `Form` and `ModalResult` from it (a `Button` with a `modalResult` closes its form). The
Delphi concepts it carries: `Application.Run`, `Form.ShowModal`, `ModalResult`, `OnCloseQuery`, `caFree`. No section of
`doc/plans/main.md` covers it; the desktop forms of spec 2.5 and 2.6 (`<application>`, `mainForm`) are the separate
`@desktop/forms` + `@lunarstorm/vcl` pair.

## Scripts

- `pnpm --filter @coldmind/vcl.app typecheck` runs `tsc --noEmit -p tsconfig.json`.
- No `test` or `build` script; no tests in the package.

## Decisions and hazards

- `FormHost` is declared in `vcl.form.tsx` so the form module does not import `Application` (no cycle).
- The first `createForm` becomes the main form. Closing the main form (`hideForm`) calls `terminate()`; `showForm` on the
  main form is a no-op. `terminate` unmounts the root, destroys all forms and leaves the kernel.
- `close()` order: `onCloseQuery` (false keeps it open) -> `onClose` (may return `"none"` to cancel or `"free"` to destroy)
  -> hide. A modal closed without a result settles as `"cancel"`. Escape and the caption `x` set `modalResult = "cancel"`.
- `showModal` on a visible form rejects; `show` on a form without an `Application` owner throws.
- Unhandled exceptions: with no `onException`, the application logs, calls `kernel.onTrace` and shows a dismissible banner.
  Note it does not call `kernel.onException`, so a kernel-level handler is bypassed for anything an application owns.
- `run` installs the stylesheet via `installStyles(document)` and mounts into `#app`, else appends a `div` to `body`;
  it needs a DOM.
- `remoteToken` caches one token per controller class; `provideClient` must run before any form is created.
- `index.ts` and the files import each other by package name through `"./*": "./*"`.
