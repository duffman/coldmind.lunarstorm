# @coldmind/core

The vocabulary shared by every layer: the validation result and error types (`ValidationError`, `ValidationResult`, `Path`, `ValidationLevel`), two formatters that render errors as text (`formatPath`, `formatErrors`), and type-only re-exports from `type-fest`. Nothing in this package knows about fields, tables or SQL.

## Entry points

From `package.json`:

| Specifier | File |
|---|---|
| `@coldmind/core` (`main`, `types`, `exports["."]`) | `./src/core.ts` |
| `@coldmind/core/*` (explicit subpath exports) | `./src/core.validation.ts` or `./src/core.types.ts` (explicit exports only) |

Dependencies: `type-fest ^5.10.0`, used for types only.

## Exports

### Validation types (`validation.ts`)

```ts
/** Location of a value: object keys and array indexes from the root. */
type Path = (string | number)[]
```

```ts
/** 'error' fails validation; 'warning' is reported but does not. */
type ValidationLevel = 'error' | 'warning'
```

```ts
/** One failed check. */
type ValidationError = {
  path: Path
  msg: string
  code: string
  level: ValidationLevel
}
```

Example (from source JSDoc):

```ts
const e: ValidationError = { path: ['items', 0, 'qty'], msg: 'Min 1', code: 'MIN', level: 'error' }
```

```ts
/** What every validate* and parse* returns. Read `.value` after checking `.isValid`. */
type ValidationResult<T> = {
  isValid: boolean
  value: T | undefined
  errors: ValidationError[]
  warnings: ValidationError[]
}
```

### Formatting (`validation.ts`)

```ts
function formatPath(
  path: Path,          // required; no default
): string
```

Numbers render as `[n]`. Strings are joined with `.`, and the first segment has no leading dot. An empty path gives `''`. Never throws.

```ts
formatPath(['a', 0, 'b'])       // 'a[0].b'
formatPath(['items', 0, 'qty']) // 'items[0].qty'
formatPath([])                  // ''
```

```ts
function formatErrors(
  errors: readonly ValidationError[],  // required; no default
): string
```

Renders each error as `${formatPath(e.path) || '(root)'}: ${e.msg}` and joins them with `'; '`. An empty array gives `''`. The function ignores `level` and `code`. Never throws.

```ts
import { formatErrors, type ValidationError } from '@coldmind/core'

const errors: ValidationError[] = [
  { path: ['title'], msg: 'Cannot be empty', code: 'EMPTY', level: 'error' },
  { path: [], msg: 'Type mismatch', code: 'TYPE', level: 'error' },
]
formatErrors(errors) // 'title: Cannot be empty; (root): Type mismatch'
```

### Type helpers (`type-utils.ts`)

These are type-only re-exports from `type-fest`. They have no runtime presence.

```ts
export type {
  Exact,
  JsonValue,
  Merge,
  Promisable,
  ReadonlyDeep,
  Simplify,
  Tagged,
  UnknownRecord,
} from 'type-fest'
```

In this repo, `@coldmind/model` uses `Simplify`, `Merge` and `Tagged`. `Tagged` backs the model package's `Defaulted<F>`.

## Validation error codes

This package emits no codes. `ValidationError.code` is a free `string`. The codes themselves come from `@coldmind/model` (see `../model/docs/model.api.md`).

## Notes

- `formatErrors` renders warnings the same way as errors if you pass them in. Separating the two is the caller's job (`ValidationResult` keeps them in `errors` and `warnings`).
- The source barrel is `src/core.ts`; the package name is `@coldmind/core`.
