# runtime/typescript-declarations: `lunarstorm-typescript-declarations` and `lunartypes`

One folder, two packages (plan D6):

- the Cargo crate **`lunarstorm-typescript-declarations`** (`src/lib.rs`): turns the Runtime's
  declarations (`ModuleDecl`, `Item`, `Ty` in `runtime/engine-contract/src/native_declarations.rs`) into `.d.ts`;
- the npm package **`lunartypes`** (`package.json`): those files, generated into
  `dist/` and committed.

Every native a script or host module can reach is declared once in Rust; the same
declaration checks the call's arguments at run time and becomes the TypeScript type
here, so the two cannot drift.

## Surface

| item | what |
|---|---|
| `emit(&Surface) -> Vec<(PathBuf, String)>` | renders the files |
| `Surface { core, globals, extensions, hosts }` | the declarations, split into layers |
| `Host { name, modules, extensions }` | what one host adds, and the extensions it installs |
| `common(a, b)`, `only(all, base)` | split hosts' merged declarations into layers |
| `INTERNAL` | `lunarstorm:internal`, rendered as `Ops` + `ops` |

Collecting the declarations is `type_files()` in `platform/cli/host/src/lunar_runtime_commands.rs`,
behind the command `lunarstorm types [--out <dir>]` (`doc/cli.md`).

| file | holds | import as |
|---|---|---|
| `dist/index.d.ts` | what every host offers | `lunartypes` |
| `dist/cli.d.ts` | the CLI host's additions; references index and `extensions/hostapi.d.ts` | `lunartypes/cli` |
| `dist/desktop.d.ts` | the desktop host's additions; references index | `lunartypes/desktop` |
| `dist/extensions/hostapi.d.ts` | HostApi as the CLI host installs it (area `ui`) | via `lunartypes/cli` |
| `dist/globals.d.ts` | `declare global { … }`; only written once a global is declared | via `lunartypes` |

Today every declared native sits on `ops` (`lunarstorm:internal`); the `node:*`
modules join as they are ported to Rust, with no change here.

## Regenerate and check

```sh
cargo run --bin lunar -- types               # at the repo root; writes runtime/typescript-declarations/dist
cargo test -p lunarstorm-typescript-declarations               # emitter unit test + tests/generated.rs (dist is current)
pnpm --filter lunartypes typecheck           # tsc over dist/ and check/*.ts
```

`tests/generated.rs` regenerates the files in memory and fails, naming
`lunarstorm types`, when any differs from `dist/` or `dist/` holds a file no longer
generated. `check/sample.ts` uses the declarations; `check/negative.ts` holds one
`@ts-expect-error` per call the types must reject.

## Decisions

- **Layers by subtraction.** Each host's set comes from a Runtime built as that host
  builds it, never started. The core is what the CLI and desktop hosts declare
  identically; a host or extension file holds only what it adds. A module a layer
  adds to is declared again there; TypeScript merges ambient modules and interfaces.
- **`lunarstorm:internal` is emitted, under its own specifier**, as `interface Ops` and
  `export const ops: Ops`: the module's declared export is the `ops` object, and its
  items are that object's members. Host modules and extension JS import it; the
  Runtime refuses the import in user programs.
- **Globals in their own file.** `declare global` needs a module file, and a module
  file turns `declare module "x"` into an augmentation, so they cannot share one.
- **Mapping.** `Bytes` → `Uint8Array`, `Unknown` → `unknown`, an empty `Object` →
  `object`, `Named` → the name, a return of `Undefined` → `void`. `Optional` is `?` on
  a parameter or field, unless a required parameter follows: then `T | undefined`,
  because TypeScript forbids `?` there. Class accessors are plain properties: a
  declaration does not say whether one is read-only.
- **Deterministic text.** Declaration order, LF, 2-space indent, JSDoc from `about`.
- **The typecheck reads the `.d.ts` itself.** `tsconfig.json` sets
  `skipLibCheck: false` (the shared base skips declaration files) and `types: []`.
- **dist/ is committed.** The root `.gitignore` ignores every `dist/*`; this folder's
  `.gitignore` re-includes it.
- **The desktop host's shell is optional.** `lunarstorm-host-desktop-backend` gates tao behind
  its default feature `shell`, so the CLI reads its declarations without tao.
