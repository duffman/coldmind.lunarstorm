# @coldmind/decorate

The decorator kernel: one view of legacy (`experimentalDecorators`) and standard (TC39) decorators, and per-class records for both. Browser-safe, no runtime dependencies. A library keeps its own meaning for decorators and writes it into records the kernel keys correctly.

## API

```ts
import { createRecords, siteOf, classChain, metadataKey } from '@coldmind/decorate';
```

| Export | What it does |
| --- | --- |
| `siteOf(value, arg?, third?)` | Reads the arguments a decorator function received, whichever dialect called it, into a `DecoratorSite`: `dialect`, `placement` (`class`, `field`, `accessor`, `setter`, `getter`, `method`, `parameter`), `name`, `index`, `static`, `private`, `ctor`, `metadata`, `access`. |
| `createRecords(make)` | A record store. `at(site)` returns the record a decorator writes to; `of(ctor)` returns the record collected for that class alone. |
| `classChain(ctor)` | A class and its bases, base first, for merging records. |
| `placementPhrase` | Placement names for error messages. |
| `metadataKey` | `Symbol.metadata`, polyfilled once for runtimes that lack it (Bun). |

## Records

- **Standard:** keyed on `context.metadata`. A member never attaches to another class, and no class decorator is needed. A subclass sees only its own record; merging bases is the caller's job, with `classChain`.
- **Legacy:** keyed on the constructor.
- **No metadata object:** a standard class decorator on a runtime that provides none gets one object, used for its record and the class lookup. Standard member decorators need the object and the caller decides what to do without it.
- Each store has its own slot on the metadata object, so libraries do not see each other's records.

## Writing a decorator

```ts
import { createRecords, placementPhrase, siteOf } from '@coldmind/decorate';

const store = createRecords<Notes>(() => ({ fields: [] }));

export function Field(value: unknown, arg?: unknown, third?: unknown): void {
  const site = siteOf(value, arg, third);
  if (site.placement !== 'field') throw new TypeError(`@Field goes on a field, not ${placementPhrase[site.placement]}`);
  store.at(site).fields.push(String(site.name));
}
// later
store.of(SomeClass)?.fields;
```

## Tests

```bash
cd packages/foundation/decorate && bun run test
```

The legacy suite runs with `tests/legacy/tsconfig.json`, which enables `experimentalDecorators`.

## Distribution

`bun run build` emits JavaScript and declarations to `dist` with NodeNext resolution.
Node imports compiled JavaScript; Bun imports source TypeScript. TypeScript consumers
resolve declarations first. The build uses package self-name remapping (`rootDir` /
`outDir`), not path aliases or previously built declarations.

Explicit implementation entries are `@coldmind/decorate/internal/core`,
`/internal/metadata`, `/internal/records`, and `/internal/site`. Existing
`@coldmind/decorate/src/decorate.{core,metadata,records,site}.ts` imports remain
compatibility aliases to the same condition-selected modules. The former arbitrary
wildcard is intentionally replaced by these inventoried entries and `./package.json`.

Run `bun run typecheck` and `bun run test` for source/dialect checks. After building,
`node --test tests/node/*.test.mjs` checks compiled imports, compatibility entry
identity, metadata side effects, and distribution contracts without a transpiler.
