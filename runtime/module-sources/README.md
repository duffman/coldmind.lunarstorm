# lunarstorm-module-sources

What the Runtime's module loader can import, beyond host modules and builtins:
every file becomes an ES module the VM compiles, and every specifier resolves to
a file. No VM, no JS engine, no event loop; `runtime/event-loop-runtime` calls it from its VM
callbacks, and anything else (a bundler, a CLI check) can resolve the same way.

## Public surface

- `Modules::new(read)`: one per Runtime. `read` reads a file under the host's
  policy; module files and `package.json` files both go through it.
- `Modules::resolve(base, specifier)`: a canonical absolute path. A
  `lunarstorm-require:` prefix asks for `require()` rules; an `import` of a
  CommonJS file answers `<file>#esm` (its facade).
- `Modules::load(name, builtin)`: the file as an ES module. `builtin` maps a
  `require()` specifier to a builtin or host module name.
- `Modules::entry(name, code, file, builtin)`: the same for the entry module a
  host hands over; a CommonJS entry runs its body at once.
- `transform::{strip_types, json_module, imports, scan_commonjs, commonjs_body, commonjs_facade}`
  (`imports` lists what an ES module imports; `.jbn` packaging walks the graph with it)
  and `Manifest` (`type`, `main`, `exports`, `imports`).

| file                          | becomes                                             |
|-------------------------------|-----------------------------------------------------|
| `.mjs`                        | itself                                              |
| `.js`, no `type` / no manifest | itself with module syntax, else CommonJS           |
| `.js` under `"type":"module"` | itself                                              |
| `.cjs`, `.js` under CommonJS  | its body; an `import` reaches a facade              |
| `.json`                       | `export default` of its value                       |
| `.ts`, `.mts`, `.cts`         | types stripped, then as `.js`, `.mjs`, `.cjs`        |

## Decisions

- Ported from jetbunny's source packages (`src/platform/package/`), adapted to
  real paths under the host policy and Node's `node_modules` lookup (works with
  pnpm's symlinks: resolved files are canonicalized).
- Conditions: `lunarstorm`, `node`, then `import` or `require`, then `default`,
  chosen in the mapping's key order as Node does; an unmatched condition falls
  through, `null` excludes.
- TypeScript is stripped, never type-checked. Printed lines are padded or joined
  so every line sits on its original line number (stack traces name `.ts`
  lines); comments are dropped so joining is safe, and lines inside template
  literals are never joined.
- The CommonJS body carries the file's own name, so stack frames of CommonJS code
  name the real file; the one-line facade carries `#esm`.
- Builtins and host modules win over packages of the same name (the caller's
  `builtin` function decides).

## Hazards

- Kinds and manifests are cached per `Modules`: a file whose `type` changes on
  disk keeps its first kind until a new Runtime.
- `.tsx`/`.jsx` are not transformed.

## Build and test

```sh
cargo test -p lunarstorm-module-sources
cargo test -p lunarstorm-event-loop-runtime --test modules   # real files on QuickJS
```
