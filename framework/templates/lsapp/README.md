# LS desktop application template

This starter explores a Delphi-inspired, event-driven desktop application model. A view is a persistent object that owns controls; JSX describes its initial component tree as `LSNode` values. The included browser host renders that model as a local desktop mockup. It is not a native Tao integration.

## Run

Requires Bun and TypeScript (`tsc`) on the PATH. No packages need to be installed in this template.

```sh
bun run dev
```

Open the printed loopback URL. The example runs entirely in memory. Restart the command after editing to rebuild.

Use `bun run typecheck`, `bun run build`, and `bun test` for verification. The build uses the TypeScript compiler API, locating an installed TypeScript package or the package containing the `tsc` executable.

## Project structure

- `application.main.ts` creates the application and starts the generated view pair.
- `lsapp.project.json` contains the starter's entry, view directory, output directory, and preview metadata.
- `views/MyDash/MyDash.ts` declares the view, typed outlets, injected service, and action.
- `views/MyDash/MyDash.tsx` declares its `LSNode` layout and connects controls using references to those members.
- `services/` contains application services separate from the view.
- `common/` provides components, lifecycle, event dispatch, service injection, JSX and the browser host.
- `tooling/` checks the pairing and supported connections using TypeScript symbols, generates the view registry, typechecks and bundles the application.

Each view has its own directory, containing a same-named `.ts` class and `.tsx` layout. The generator writes `tooling/generated-views.ts` from these pairs. Outlets and injections take constructor tokens (`@ViewOutlet(Edit)`, `@Inject(CounterService)`), so no reflection package or string identifiers are needed. Visible captions and project paths are ordinary strings.

## Current boundaries

The validator checks view pairing, declared outlets, action connections and injection tokens. It is not a general whole-program dependency analyser or a native bridge generator. The common library and host are a small prototype, not a full Delphi VCL. Application dispatch serializes synchronous events; an async action resumes through the JavaScript promise scheduler, while control notifications enter the application event loop. Async actions should guard against completing after their view is disposed. A native desktop host, native window/event integration, durable storage and a cross-runtime bridge are not implemented.
