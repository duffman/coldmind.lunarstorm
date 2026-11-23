# @coldmind/model

Fields, models, projections, validation, JSON Schema, introspection and records. Fields are immutable and chainable: every modifier returns a new field. A model is a shape (a record of fields) that carries one typed handle per field. You project a model by passing handles, never strings. `check` and `parse` validate a model or a plain shape. `toJsonSchema` and `introspect` walk the same field tree the validator uses. `record` puts self-validating instances on top of a model. The package knows nothing about storage.

## Entry points

From `package.json`:

| Specifier | File |
|---|---|
| `@coldmind/model` (`main`, `types`, `exports["."]`) | `./src/model.ts` |
| `@coldmind/model/internal` (`exports["./internal"]`) | `./src/model.internal.ts`: sibling-package internals including `clearDataTypes`, `buildModel`, `metaOf`, `cloneValue`, `ConstrainedField`, `Constraint`, `ParseOut`, `MODEL`, `MODEL_OWNER`, `canonicalFor`, `hasAsyncValidation`. Documented in source as "Not exported from the root." |
| Compatibility entrypoints | `@coldmind/model/define-model` and `@coldmind/model/model.key` remain explicit export-map compatibility aliases (no wrapper files). Arbitrary source deep-imports are not exported. |

Source layout and how to extend the module: [`doc/README.md`](doc/README.md).

Dependency: `@coldmind/core` (`workspace:*`). Test script: `vitest run`.

Everything below is exported from `@coldmind/model` (`model.ts`).

---

## Exports

### Field base (`src/fields/model.field.pipeline.ts`)

```ts
type SyncValidator<T> = (v: T, ctx: { path: Path }) => ValidationError | null
type AsyncValidator<T> = (
  v: T,
  ctx: { path: Path },
) => ValidationError | null | Promise<ValidationError | null>

/** Type-only tag set by .default(); same object at runtime. */
type Defaulted<F> = Tagged<F, 'defaulted'>

interface FieldOpts<T> {
  req: boolean                        // initial: true
  null: boolean                       // initial: false
  def: T | (() => T) | undefined      // initial: undefined
  doc: string                         // initial: ''
  hidden?: boolean                    // set by .hidden()
}
```

`FieldOpts` is a named interface so other packages can declaration-merge their own annotations into it.

```ts
abstract class Field<T, O = T> {
  _opts: FieldOpts<T>   // @internal, but read by projections / record / introspect

  required(): Field<T, Exclude<O, undefined>>
  optional(): Field<T, O | undefined>
  nullable(): Field<T, O | null>
  default(v: T | (() => T)): Defaulted<Field<T, Exclude<O, undefined>>>
  doc(s: string): this
  hidden(): this & { _opts: FieldOpts<T> & { hidden: true } }
  coerce(fn: (v: unknown) => unknown): this
  transform(fn: (v: T) => T): this
  use(fn: SyncValidator<T>): this
  useAsync(fn: AsyncValidator<T>): this
  hasAsync(): boolean
  validateSync(path: Path, raw: unknown): ValidationResult<O>
  validate(path: Path, raw: unknown): Promise<ValidationResult<O>>
  abstract clone(): this
}
```

| Method | Behaviour | Code |
|---|---|---|
| `required()` | `undefined` is an error. This is the default state. | `REQ` |
| `optional()` | `undefined` is accepted, and the property becomes optional in `InferShape`. | none |
| `nullable()` / `nonNullable()` | Accept/reject `null` and update the output type explicitly. | `NULL` |
| `default(v)` | Used when input is `undefined`. A function is called per use. Plain cloneable object values are cloned per use; custom class values require a factory default or a `defineDataType({ clone })` hook. | none |
| `doc(s)` | Description, surfaced by `introspect` and in JSON Schema as `description`. | none |
| `hidden()` | Left out of `publicView`. The field stays in the shape and is still validated. Hidden state is preserved across presence modifiers. | none |
| `coerce(fn)` | Runs before the type check. Chained calls compose (`fn(prev(v))`). A throw becomes an error with msg `Coercion failed: ${String(e)}`. | `COERCE_ERROR` |
| `transform(fn)` | Runs after the type check, `T → T`. Chained calls compose. A throw becomes an error with msg `Transform failed: ${String(e)}`. | `TRANSFORM_ERROR` |
| `use(fn)` | Sync custom validator. Return `null` to pass. A returned error with `level: 'warning'` goes to `warnings`. | caller's |
| `useAsync(fn)` | Async custom validator. Makes this field and every ancestor async-only. | caller's |
| `hasAsync()` | True if this field or anything nested under it has an async validator. | none |
| `validateSync(path, raw)` | Validates one value. `path` prefixes error paths (`[]` at the root). **Throws `Error`** if `hasAsync()`: `validateSync() called on a field with async validators at path <json> — use validate()`. | none |
| `validate(path, raw)` | Async validation. Works on sync fields too. | none |

Pipeline per value (in `_prologue` + `_validateSyncUnchecked` / `validate`):
1. `undefined` → resolve the default. Still `undefined` → `REQ` if required, otherwise valid with `value: undefined`.
2. `null` → valid with `null` if nullable, otherwise `NULL`.
3. coerce → `COERCE_ERROR` on throw.
4. type check → `TYPE` (msg `Type mismatch`).
5. transform → `TRANSFORM_ERROR` on throw.
6. structural parse: constraints for `str`/`num`, items for `list`, keys for `obj`, variants for unions.
7. `use()` validators, then (async path only) `useAsync()` validators. These run **only if step 6 produced no errors**.

```ts
const nick = str().trim().min(2).optional().doc('Display name')
nick.validateSync([], '  a ').errors // [{ path: [], msg: 'Min length 2', code: 'MIN', level: 'error' }]

const even = int().use((v, ctx) =>
  v % 2 === 0 ? null : { path: ctx.path, msg: 'Must be even', code: 'EVEN', level: 'error' },
)
```

---

### Field factories and classes

Each subclass overrides `optional()`, `nullable()`, `required()` and `default()` so that they return the same subclass (e.g. `StrField<O | undefined>`).

#### `str` / `StrField` (`src/fields/model.field.string.ts`)

```ts
const str: () => StrField<string>

class StrField<O = string> extends ConstrainedField<string, O> {
  coerceString(): this
  trim(): this
  lowercase(): this
  uppercase(): this
  min(n: number): this
  max(n: number): this
  length(n: number): this
  nonempty(): this
  pattern(re: RegExp, msg?: string /* = 'Invalid format' */): this
  email(): this
  url(): this
  uuid(): this
  oneOf<const V extends readonly string[]>(values: V): StrField<V[number] | Exclude<O, string>>
}
```

Type check: `typeof v === 'string'`.

| Method | Kind | Check | msg | Code | JSON Schema params |
|---|---|---|---|---|---|
| `coerceString()` | coerce | `String(v)` | n/a | n/a | n/a |
| `trim()` | transform | `v.trim()` | n/a | n/a | n/a |
| `lowercase()` | transform | `v.toLowerCase()` | n/a | n/a | n/a |
| `uppercase()` | transform | `v.toUpperCase()` | n/a | n/a | n/a |
| `min(n)` | constraint | `v.length >= n` | `Min length ${n}` | `MIN` | `{ minLength: n }` |
| `max(n)` | constraint | `v.length <= n` | `Max length ${n}` | `MAX` | `{ maxLength: n }` |
| `length(n)` | constraint | `v.length === n` | `Length must be ${n}` | `LENGTH` | `{ minLength: n, maxLength: n }` |
| `nonempty()` | constraint | `v.length > 0` | `Cannot be empty` | `EMPTY` | `{ minLength: 1 }` |
| `pattern(re, msg)` | constraint | `re.test(v)` after `re.lastIndex = 0` | `msg` (default `Invalid format`) | `PATTERN` | `{ pattern: re.source, flags: re.flags without g/y }` |
| `email()` | constraint | `/^[^\s@]+@[^\s@]+\.[^\s@]+$/` | `Invalid email` | `EMAIL` | `{ format: 'email' }` |
| `url()` | constraint | `new URL(v)` does not throw | `Invalid URL` | `URL` | `{ format: 'uri' }` |
| `uuid()` | constraint | `/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i` | `Invalid UUID` | `UUID` | `{ format: 'uuid' }` |
| `oneOf(values)` | constraint | `values.includes(v)` | `Must be one of: ${values.join(', ')}` | `ONE_OF` | `{ enum: [...values] }` |

```ts
const slug = str().trim().lowercase().min(1).max(64).pattern(/^[a-z0-9-]+$/)
const role = str().oneOf(['admin', 'user'] as const) // StrField<'admin' | 'user'>
```

#### `num` / `int` / `NumField` (`src/fields/model.field.number.ts`)

```ts
const num: () => NumField<number>
const int: () => NumField<number>   // = new NumField().integer()

class NumField<O = number> extends ConstrainedField<number, O> {
  coerceNumber(): this
  min(n: number): this
  max(n: number): this
  positive(): this
  negative(): this
  nonzero(): this
  integer(): this
  finite(): this
  port(): this
}
```

Type check: `typeof v === 'number' && !Number.isNaN(v)`, so `NaN` is `TYPE`.

| Method | Kind | Check | msg | Code | JSON Schema params |
|---|---|---|---|---|---|
| `coerceNumber()` | coerce | non-blank string → `Number(v)`; anything else unchanged | n/a | n/a | n/a |
| `min(n)` | constraint | `v >= n` | `Min ${n}` | `MIN` | `{ minimum: n }` |
| `max(n)` | constraint | `v <= n` | `Max ${n}` | `MAX` | `{ maximum: n }` |
| `positive()` | constraint | `v > 0` | `Must be positive` | `POSITIVE` | `{ exclusiveMinimum: 0 }` |
| `negative()` | constraint | `v < 0` | `Must be negative` | `NEGATIVE` | `{ exclusiveMaximum: 0 }` |
| `nonzero()` | constraint | `v !== 0` | `Cannot be zero` | `NONZERO` | none |
| `integer()` | constraint | `Number.isInteger(v)` | `Must be an integer` | `INTEGER` | `{ type: 'integer' }` |
| `finite()` | constraint | `Number.isFinite(v)` | `Must be finite` | `FINITE` | none |
| `port()` | constraint | integer and `1 <= v <= 65535` | `Invalid port` | `PORT` | none |

```ts
const priority = int().min(1).max(5)
const ratio = num().finite().positive()
const fromQuery = int().coerceNumber()
```

#### `bool` / `BoolField` (`fields/boolean.field.ts`)

```ts
const bool: () => BoolField<boolean>
class BoolField<O = boolean> extends Field<boolean, O> {}
```

Strict: `typeof v === 'boolean'`. `'true'` and `1` are `TYPE` unless you add `.coerce(fn)`. There are no extra methods.

```ts
const done = bool().default(false)
```

#### `any` / `AnyField` (`fields/any.field.ts`)

```ts
const any: <T = unknown>() => AnyField<T>
class AnyField<T = unknown, O = T> extends Field<T, O> {}
```

The type check always passes. Presence (`REQ`/`NULL`), `coerce`, `transform`, `use` and `useAsync` still apply.

```ts
const payload = any<{ kind: string }>()
const when = any<Date>().use((v, ctx) =>
  v instanceof Date ? null : { path: ctx.path, msg: 'Not a Date', code: 'DATE', level: 'error' },
)
```

#### `defineDataType` / `DataTypeSpec` / `guard` — extension data types (`src/fields/model.field.custom.types.ts`)

```ts
interface DataTypeSpec<F extends SomeField = SomeField> {
  readonly name: string            // unique; carried on every field as `_opts.typeName`
  readonly base: () => F           // validation and value type
  readonly schema?: JsonSchema     // merged over the base field's schema in `toJsonSchema`
  // + `sql?`, `codec?` when @coldmind/data/schema is loaded (declaration merging)
}
function defineDataType<F extends SomeField>(spec: DataTypeSpec<F>): () => F
function dataTypeOf(x: SomeField | string): DataTypeSpec | undefined
function dataTypes(): readonly DataTypeSpec[]
function guard<T, O>(field: AnyField<T, O>, check: (v: T) => boolean, msg: string, code = 'TYPE'): AnyField<T, O>
```

An extension data type is one spec in one file. The name survives cloning, modifiers, `fieldsOf` and
projections. `introspect` reports it as `type`; `kind` stays the base's. `defineDataType` throws when the
name is already registered. `clearDataTypes` (`@coldmind/model/internal`) empties the registry.

```ts
// types/money.ts
export const money = defineDataType({
  name: 'money',
  base: () => guard(any<Money>(), (v) => v instanceof Money, 'Not Money'),
  schema: { type: 'string', pattern: '^-?\\d+\\.\\d{2}$' },
  sql: { postgres: 'NUMERIC(19,2)', mysql: 'DECIMAL(19,2)', default: 'TEXT' },
  codec: { toDb: (m: Money) => m.toString(), fromDb: (v: unknown) => Money.parse(String(v)) },
})
```

#### `list` / `listOf` / `ListField` (`src/fields/model.field.list.ts`)

```ts
function list<F extends SomeField>(item: F): ListField<F>
function list<M extends AnyModel>(item: M): ListField<ObjField<ShapeOf<M>>>
function list<S extends Shape>(item: S): ListField<ObjField<S>>

function listOf<M extends AnyModel>(item: M): Model<'', { items: ListField<ObjField<ShapeOf<M>>> }>
function listOf<F extends SomeField>(item: F): Model<'', { items: ListField<F> }>

class ListField<F extends SomeField, O = InferField<F>[]> extends Field<InferField<F>[], O> {
  constructor(_item: F)
  get item(): F
  minLength(n: number): this
  maxLength(n: number): this
}
```

Type check: `Array.isArray(v)`. Each item is validated at path `[...path, i]`. The async path validates items concurrently (`Promise.all`). A model or shape passed to `list` is wrapped with `obj(...)`. `listOf(x)` is `model({ items: list(x) })`, an unnamed model.

| Method | msg | Code | JSON Schema |
|---|---|---|---|
| `minLength(n)` | `Min ${n} items` | `MIN_LENGTH` | `minItems: n` |
| `maxLength(n)` | `Max ${n} items` | `MAX_LENGTH` | `maxItems: n` |

`minLength` / `maxLength` are registered as sync validators, not constraints. They therefore run only when every item is valid.

```ts
const tags = list(str().nonempty()).maxLength(10)
tags.validateSync([], ['a', '']).errors[0]?.path // [1]
const Page = listOf(Account) // Model<'', { items: ListField<ObjField<…>> }>
```

#### `obj` / `ObjField` (`src/fields/model.field.object.ts`)

```ts
function obj<S extends Shape>(shape: S): ObjField<S>
function obj<M extends AnyModel>(model: M): ObjField<ShapeOf<M>>

class ObjField<S extends Shape, O = InferShape<S>> extends Field<InferShape<S>, O> {
  constructor(readonly shape: S)
  strict(): this
}
```

Type check: non-null, non-array `object`. Each declared key is validated at `[...path, k]`, reading the source value only if `Object.hasOwn(src, k)`. A key whose result value is `undefined` is omitted from the output. Undeclared keys are never copied to the output. They are reported as `UNKNOWN_KEY` (msg `Unknown key`, path `[...path, k]`): as a **warning** by default, as an **error** after `.strict()`.

```ts
const address = obj({ street: str(), zip: str().length(5) }).strict()
const owner = obj(Account)
```

#### `union` / `UnionField` (`src/fields/model.field.union.ts`)

```ts
const union: <const Fs extends readonly SomeField[]>(...variants: Fs) => UnionField<Fs>

class UnionField<Fs extends readonly SomeField[] = readonly SomeField[], O = InferField<Fs[number]>>
  extends Field<InferField<Fs[number]>, O> {
  constructor(readonly variants: Fs)
}
```

The type check always passes. Variants are tried in order and the first valid one wins (its warnings are kept). If none matches, the errors are `{ path, msg: 'No union variant matched', code: 'UNION' }` followed by every variant's errors. The constructor takes presence from the variants:
- `default` = the first variant default found
- `required` = no default and every variant is required
- `nullable` = some variant is nullable

```ts
union(str(), int())            // string | number
union(str().optional(), int()) // string | number | undefined
union(str().email(), int().positive())
```

#### `discriminated` / `DiscriminatedField` / `DiscValue` (`src/fields/model.field.union.ts`)

```ts
type DiscValue<K extends string, V extends Record<string, Shape>> = {
  [P in keyof V & string]: InferShape<V[P]> & { readonly [D in K]: P }
}[keyof V & string]

function discriminated<K extends string, V extends Record<string, Shape>>(
  key: K,
  variants: V,
): DiscriminatedField<K, V>

class DiscriminatedField<K extends string = string, V extends Record<string, Shape> = Record<string, Shape>, O = DiscValue<K, V>>
  extends Field<DiscValue<K, V>, O> {
  constructor(readonly discKey: K, readonly variants: { readonly [P in keyof V]: ObjField<Shape> })
}
```

Type check: non-null, non-array `object`. `v[key]` has to be a string that is an own key of `variants`. Otherwise the error is `{ path: [...path, key], msg: 'Unknown ${key}: ${String(tag)}', code: 'DISCRIMINANT' }`. The selected variant is validated as an `obj(...)` at `path`. When a variant shape does not declare `key`, `discriminated` injects `key: str().oneOf([tag])` into it.

```ts
const Animal = discriminated('kind', {
  cat: { lives: int() },
  dog: { bark: str() },
})
```

---

### Shapes and inference (`src/models/model.shape.inference.ts`)

```ts
type SomeField = Field<any, any>
type Shape = Record<string, SomeField>
type InferField<F> = F extends Field<any, infer O> ? O : never
type InferShape<S extends Shape>  // keys whose output includes undefined become optional properties
type Infer<X>                     // InferShape<ShapeOf<X>> for a model, InferShape<X> for a shape
```

```ts
const input = { title: str().nonempty(), priority: int().min(1).optional() } satisfies Shape
type Input = InferShape<typeof input> // { title: string; priority?: number }
```

---

### Models (`src/models/model.definition.ts`)

```ts
type ModelMeta<N extends string, S extends Shape> = {
  readonly name: N
  readonly fields: S
  readonly owner: object
  readonly strict?: boolean
  readonly refinements?: readonly SyncValidator<any>[]
  readonly asyncRefinements?: readonly AsyncValidator<any>[]
}
type FieldRef<N extends string = string, K extends string = string, F extends SomeField = SomeField> = {
  readonly model: N
  readonly key: K
  readonly field: F
  readonly [MODEL_OWNER]: object
}
type Model<N extends string, S extends Shape> =
  { readonly [K in keyof S & string]: FieldRef<N, K, S[K]> } & { readonly [MODEL]: ModelMeta<N, S> }
type AnyModel = { readonly [MODEL]: ModelMeta<any, any> }
type ShapeOf<M>      // shape behind a model type
type NameOf<M>       // name type; '' for unnamed / non-model
type ShapeInput = Shape | AnyModel
type AsShape<X>      // a model's shape, or X itself if X is a Shape
type Optionalized<F extends SomeField> = ReturnType<F['optional']>
type PatchOptionalized<F extends SomeField> = ReturnType<ReturnType<F['optional']>['withoutDefault']>
type Requiredized<F extends SomeField> = ReturnType<F['required']>
```

```ts
function model<N extends string, S extends Shape>(name: N, fields: S, options?: ModelOptions): Model<N, S>
function model<S extends Shape>(fields: S, options?: ModelOptions): Model<'', S>
const defineShape: typeof model   // alias

const isModel: (x: unknown) => x is AnyModel
const toShape: (x: ShapeInput) => Shape
const fieldsOf: <M extends AnyModel>(m: M) => ShapeOf<M>
const nameOf: <M extends AnyModel>(m: M) => NameOf<M>
function refine<M extends AnyModel>(model: M, fn: SyncValidator<Infer<M>>): Model<NameOf<M>, ShapeOf<M>>
function refineAsync<M extends AnyModel>(model: M, fn: AsyncValidator<Infer<M>>): Model<NameOf<M>, ShapeOf<M>>
```

- `model(name, fields, options)` / `model(fields, options)`: snapshots the field map, creates one owner token and one `FieldRef` per field, then freezes the model. The meta sits under the non-enumerable `MODEL` symbol. `{ strict: true }` is supported directly.
- `defineShape`: the same function as `model`, kept as the RPC layer's original name.
- `isModel(x)`: true for objects that carry the `MODEL` key (anything returned by `model` or a projection).
- `toShape(x)`: the plain shape of a model, or the shape itself.
- `fieldsOf(m)` / `nameOf(m)`: the typed shape and name. The name is `''` for an unnamed model.

```ts
const Account = model('account', { id: int(), email: str().trim().email() })
const Point = defineShape({ x: num(), y: num() })
Account.email.key            // 'email'
nameOf(Account)              // 'account'
Object.keys(toShape(Account)) // ['id', 'email']
type A = Infer<typeof Account> // { id: number; email: string }
```

---

### Validation (`src/validation/model.validation.ts`)

```ts
type ShapeOptions = { readonly strict?: boolean }

function check<X extends ShapeInput>(
  target: X,
  data: unknown,
  options?: ShapeOptions,           // strict: default = the model's meta.strict, else false
): ValidationResult<Infer<X>>

function checkAsync<X extends ShapeInput>(
  target: X,
  data: unknown,
  options?: ShapeOptions,
): Promise<ValidationResult<Infer<X>>>

function parse<X extends ShapeInput>(
  target: X,
  data: unknown,
  options?: ShapeOptions,
): Infer<X>

function parseAsync<X extends ShapeInput>(
  target: X,
  data: unknown,
  options?: ShapeOptions,
): Promise<Infer<X>>

function unwrapResult<T>(
  result: ValidationResult<T>,
  context?: string,
): T

class ValidationFailed extends Error {
  readonly name: 'ValidationFailed'
  readonly errors: readonly ValidationError[]
  readonly warnings: readonly ValidationError[]
  constructor(
    errors: readonly ValidationError[],
    warnings?: readonly ValidationError[],   // default []
    context?: string,
  )
}
```

| Function | Returns | Throws |
|---|---|---|
| `check` | result | `Error('check() called on a shape with async validators — use checkAsync()')` if any field is async |
| `checkAsync` | promise of result | none |
| `parse` | value | `ValidationFailed` if invalid; the same `Error` as `check` if async |
| `parseAsync` | promise of value | `ValidationFailed` if invalid |
| `unwrapResult` | `result.value` | `ValidationFailed(result.errors, result.warnings, context)` if `!isValid` |

`ValidationFailed.message` is `formatErrors(errors)`, prefixed by `${context}: ` when `context` is given.

Top-level input goes through the same `ObjField` path as a nested `obj()`: non-object input gives `TYPE`, `undefined` gives `REQ`, `null` gives `NULL`, and undeclared keys give `UNKNOWN_KEY`.

```ts
import { formatErrors } from '@coldmind/core'

const r = check(Account, body, { strict: true })
if (!r.isValid) console.error(formatErrors(r.errors))

try {
  const account = parse(Account, body) // { id: number; email: string }
} catch (e) {
  if (e instanceof ValidationFailed) console.error(e.message, e.errors)
}

const value = unwrapResult(check(Account, body), 'account')
```

---

### Projections (`src/models/model.projections.ts`)

Every projection returns a new model built with the source model's **name and strictness**. TypeScript checks model name/key/field compatibility; runtime projection code additionally checks the exact model owner token and field identity.

```ts
type RefTuple<N extends string, S extends Shape, Ks extends readonly (keyof S & string)[]> = {
  readonly [I in keyof Ks]: FieldRef<N, Ks[I] & string, S[Ks[I] & string]>
}
type Visible<S extends Shape> = {
  [K in keyof S as FieldStateOf<S[K]>['hidden'] extends true ? never : K]: S[K]
}

function pick<M extends AnyModel, Ks extends readonly (keyof ShapeOf<M> & string)[]>(
  model: M,
  ...refs: RefTuple<NameOf<M>, ShapeOf<M>, Ks>
): Model<NameOf<M>, Simplify<Pick<ShapeOf<M>, Ks[number]>>>

function omit<M extends AnyModel, Ks extends readonly (keyof ShapeOf<M> & string)[]>(
  model: M,
  ...refs: RefTuple<NameOf<M>, ShapeOf<M>, Ks>
): Model<NameOf<M>, Simplify<Omit<ShapeOf<M>, Ks[number]>>>

function partial<X extends ShapeInput>(
  shape: X,
): Model<NameOf<X>, Simplify<{ [K in keyof AsShape<X>]: PatchOptionalized<AsShape<X>[K]> }>>

function makeOptional<M extends AnyModel, Ks extends readonly (keyof ShapeOf<M> & string)[]>(
  model: M,
  ...refs: RefTuple<NameOf<M>, ShapeOf<M>, Ks>
): Model<NameOf<M>, Simplify<Omit<ShapeOf<M>, Ks[number]> & { [K in Ks[number]]: Optionalized<ShapeOf<M>[K]> }>>

function makeRequired<M extends AnyModel, Ks extends readonly (keyof ShapeOf<M> & string)[]>(
  model: M,
  ...refs: RefTuple<NameOf<M>, ShapeOf<M>, Ks>
): Model<NameOf<M>, Simplify<Omit<ShapeOf<M>, Ks[number]> & { [K in Ks[number]]: Requiredized<ShapeOf<M>[K]> }>>

function merge<A extends ShapeInput, B extends ShapeInput>(
  a: A,
  b: B,
): Model<NameOf<A>, Merge<AsShape<A>, AsShape<B>>>

function publicView<X extends ShapeInput>(
  shape: X,
): Model<NameOf<X>, Simplify<Visible<AsShape<X>>>>
```

| Function | Result | Throws |
|---|---|---|
| `pick(m, ...refs)` | only the named fields | `Error('pick(): model "<name or (anonymous)>" has no field "<k>"')` for a foreign key (only possible through a cast) |
| `omit(m, ...refs)` | all but the named fields | same, with `omit()` |
| `partial(x)` | every field `.optional().withoutDefault()`; takes a model or a plain shape | none |
| `makeOptional(m, ...refs)` | the named fields `.optional()` | same, with `makeOptional()` |
| `makeRequired(m, ...refs)` | the named fields `.required()` | same, with `makeRequired()` |
| `merge(a, b)` | `{ ...fields(a), ...fields(b) }`: `b` wins a collision and `a`'s name/strict are kept | none |
| `publicView(x)` | every field without `_opts.hidden` | none |

```ts
const Public = pick(Account, Account.id, Account.email)
const NewAccount = omit(Account, Account.id)
const AccountPatch = partial(Account)
const Signup = makeOptional(Account, Account.id)
const EmailPatch = makeRequired(AccountPatch, AccountPatch.email)
const WithAudit = merge(Account, { createdAt: num() })

const User = model('user', { name: str(), password: str().hidden() })
const PublicUser = publicView(User) // fields: { name }
```

---

### JSON Schema (`src/schema/model.schema.json.ts`)

```ts
type JsonSchema = {
  readonly $schema?: string
  readonly $id?: string
  readonly title?: string
  readonly type?: string | readonly string[]
  readonly properties?: Readonly<Record<string, JsonSchema>>
  readonly required?: readonly string[]
  readonly additionalProperties?: boolean | JsonSchema
  readonly items?: JsonSchema
  readonly prefixItems?: readonly JsonSchema[]
  readonly allOf?: readonly JsonSchema[]
  readonly anyOf?: readonly JsonSchema[]
  readonly oneOf?: readonly JsonSchema[]
  readonly enum?: readonly unknown[]
  readonly const?: unknown
  readonly format?: string
  readonly pattern?: string
  readonly minLength?: number
  readonly maxLength?: number
  readonly minimum?: number
  readonly maximum?: number
  readonly exclusiveMinimum?: number
  readonly exclusiveMaximum?: number
  readonly minItems?: number
  readonly maxItems?: number
  readonly description?: string
  readonly default?: unknown
  readonly nullable?: boolean
}

function toJsonSchema(target: ShapeInput | SomeField): JsonSchema
```

It emits draft 2020-12 (`$schema: 'https://json-schema.org/draft/2020-12/schema'`). For a model or shape it emits `{ $schema, title?: name (omitted when ''), type: 'object', properties, required? (omitted when empty), additionalProperties: !strict }`. For a single field it emits `{ $schema, ...fieldSchema }`. It never throws.

Per field:
- `string` / `number` / `boolean` map to `type`. `integer()` sets `type: 'integer'`.
- `list` maps to `type: 'array'` + `items`, and `minItems`/`maxItems` come from `minLength`/`maxLength`.
- `obj` maps to `type: 'object'`, `properties`, `required` (if non-empty) and `additionalProperties: !strict`.
- `any` maps to `additionalProperties: true` with no `type`.
- `union` maps to `{ anyOf: [...variants] }`.
- `discriminated` maps to `{ oneOf: [...] }`. Each variant is `type: 'object'`, gets `properties[key] = { const: tag, type: 'string' }`, and has `key` in `required`.
- `nullable()` wraps the result in `{ anyOf: [schema, { type: 'null' }] }`. The `nullable` keyword itself is never emitted.
- `doc` maps to `description`, and a literal default maps to `default`. A factory default is not emitted.
- A property is `required` iff the field is required **and** has no default.

```ts
toJsonSchema(Account)
// { $schema, title: 'account', type: 'object',
//   properties: { id: { type: 'integer' }, email: { type: 'string', format: 'email' } },
//   required: ['id', 'email'], additionalProperties: true }
toJsonSchema(publicView(User)) // output contract without hidden fields
```

---

### Introspection (`src/schema/model.schema.introspection.ts`)

```ts
type FieldKind = 'string' | 'number' | 'boolean' | 'list' | 'object' | 'any' | 'union' | 'discriminated'

type ConstraintInfo = {
  readonly code: string
  readonly msg: string
  readonly level: ValidationLevel
  readonly params?: Readonly<Record<string, unknown>>
}

type FieldInfo = {
  readonly kind: FieldKind
  readonly type?: string                              // `defineDataType` name
  readonly required: boolean
  readonly nullable: boolean
  readonly hasDefault: boolean
  readonly default?: unknown                          // literal defaults only
  readonly hidden: boolean
  readonly doc: string
  readonly async: boolean
  readonly constraints: readonly ConstraintInfo[]
  readonly item?: FieldInfo                           // list
  readonly fields?: Readonly<Record<string, FieldInfo>> // object
  readonly strict?: boolean                           // object
  readonly variants?: readonly FieldInfo[]            // union / discriminated
  readonly discriminant?: string                      // discriminated
}

type ShapeInfo = {
  readonly name: string
  readonly fields: Readonly<Record<string, FieldInfo>>
  readonly order: readonly string[]                   // declaration order
}

function introspectField(f: SomeField): FieldInfo
function introspect(x: ShapeInput): ShapeInfo
```

- `introspectField` reads `_opts` and `_constraints`. For a `ListField` it appends synthetic `MIN_LENGTH` / `MAX_LENGTH` entries (`params: { minItems }` / `{ maxItems }`). For discriminated variants it sets each variant's `doc` to its tag.
- `introspect` returns `{ name, fields, order }`. `name` is `''` for a plain shape.

```ts
introspectField(str().max(5).optional())
// { kind: 'string', required: false, constraints: [{ code: 'MAX', msg: 'Max length 5', level: 'error', params: { maxLength: 5 } }], … }
const info = introspect(Account)
info.name  // 'account'
info.order // ['id', 'email']
```

---

### Records (`src/validation/model.record.ts`)

```ts
type RecordOf<S extends Shape> = RecordBase<S> & InferShape<S>

class RecordBase<S extends Shape> {
  constructor(
    readonly $modelName: string,
    readonly shape: S,
    initial?: Partial<InferShape<S>>,
  )
  check(): ValidationResult<InferShape<S>>
  checkAsync(): Promise<ValidationResult<InferShape<S>>>
  set(patch: Partial<InferShape<S>>): ValidationResult<InferShape<S>>
  setAsync(patch: Partial<InferShape<S>>): Promise<ValidationResult<InferShape<S>>>
  assign(data: Partial<InferShape<S>>): this
  clone(): RecordOf<S>
  toJSON(): InferShape<S>
}

interface RecordType<N extends string, S extends Shape> {
  readonly model: AnyModel
  readonly name: N
  readonly shape: S
  create(initial?: Partial<InferShape<S>>): RecordOf<S>
  check(data: unknown): ValidationResult<InferShape<S>>
  checkAsync(data: unknown): Promise<ValidationResult<InferShape<S>>>
  parse(data: unknown): RecordOf<S>
  parseAsync(data: unknown): Promise<RecordOf<S>>
}

function record<M extends AnyModel>(m: M): RecordType<NameOf<M>, ShapeOf<M>>
```

| Member | Behaviour | Throws |
|---|---|---|
| `record(m)` | builds the factory | `Error('record(): model "<name or (anonymous)>" has fields that shadow record members: <keys>')` if a field is named `constructor`, `check`, `checkAsync`, `set`, `setAsync`, `assign`, `clone`, `toJSON`, `_data`, `$modelName` or `shape` |
| `create(initial?)` | live instance. Defaults are applied; a nullable field with no default becomes `null`. Nothing is validated. | none |
| `check(data)` / `checkAsync(data)` | `check(m, data)` / `checkAsync(m, data)` | `check` throws `Error` on async shapes |
| `parse(data)` / `parseAsync(data)` | validates, then `create(value)` | `ValidationFailed(errors, warnings, name \|\| undefined)` |
| instance `check()` / `checkAsync()` | validates the current values without changing them | `check()` throws `Error` on async shapes |
| instance `set(patch)` / `setAsync(patch)` | validates `{ ...current, ...patch }` and commits only if valid | none (returns the result) |
| instance `assign(data)` | `Object.assign` with no validation | none |
| instance `clone()` | deep copy (`structuredClone` of the values) | none |
| instance `toJSON()` | shallow copy of the set values | none |

```ts
const UserModel = model('user', { name: str().nonempty(), age: int().optional() })
const User = record(UserModel)
const u = User.parse({ name: 'Ada' }) // or throws ValidationFailed
u.age = 36
u.set({ name: '' }).isValid // false; u.name is still 'Ada'
const PublicUser = record(pick(UserModel, UserModel.name))
```

---

## Validation error codes

Every code emitted by the library, grepped from `fields/*.ts` (`code: '…'` literals and `_addC(…, 'CODE', …)`). All are emitted at `level: 'error'`, except `UNKNOWN_KEY` in non-strict mode.

| Code | msg | Emitting field | Method / trigger |
|---|---|---|---|
| `REQ` | `Required` | every `Field` | input `undefined`, no default, field required (default state / `required()`) |
| `NULL` | `Cannot be null` | every `Field` | input `null` without `nullable()` |
| `COERCE_ERROR` | `Coercion failed: <e>` | every `Field` | `coerce(fn)` threw (also `coerceString` / `coerceNumber`) |
| `TYPE` | `Type mismatch` | `StrField`, `NumField` (incl. `NaN`), `BoolField`, `ListField`, `ObjField`, `DiscriminatedField` | type check failed (never emitted by `AnyField` / `UnionField`) |
| `TRANSFORM_ERROR` | `Transform failed: <e>` | every `Field` | `transform(fn)` threw (also `trim` / `lowercase` / `uppercase`) |
| `MIN` | `Min length ${n}` / `Min ${n}` | `StrField` / `NumField` | `min(n)` |
| `MAX` | `Max length ${n}` / `Max ${n}` | `StrField` / `NumField` | `max(n)` |
| `LENGTH` | `Length must be ${n}` | `StrField` | `length(n)` |
| `EMPTY` | `Cannot be empty` | `StrField` | `nonempty()` |
| `PATTERN` | `Invalid format` (or custom `msg`) | `StrField` | `pattern(re, msg)` |
| `EMAIL` | `Invalid email` | `StrField` | `email()` |
| `URL` | `Invalid URL` | `StrField` | `url()` |
| `UUID` | `Invalid UUID` | `StrField` | `uuid()` |
| `ONE_OF` | `Must be one of: …` | `StrField` | `oneOf(values)` |
| `POSITIVE` | `Must be positive` | `NumField` | `positive()` |
| `NEGATIVE` | `Must be negative` | `NumField` | `negative()` |
| `NONZERO` | `Cannot be zero` | `NumField` | `nonzero()` |
| `INTEGER` | `Must be an integer` | `NumField` | `integer()`, `int()` |
| `FINITE` | `Must be finite` | `NumField` | `finite()` |
| `PORT` | `Invalid port` | `NumField` | `port()` |
| `MIN_LENGTH` | `Min ${n} items` | `ListField` | `minLength(n)` |
| `MAX_LENGTH` | `Max ${n} items` | `ListField` | `maxLength(n)` |
| `UNKNOWN_KEY` | `Unknown key` | `ObjField` (and top-level `check`/`parse`) | undeclared key: warning by default, error with `.strict()` / `{ strict: true }` / a strict model |
| `UNION` | `No union variant matched` | `UnionField` | no variant valid; followed by all variant errors |
| `DISCRIMINANT` | `Unknown ${key}: ${tag}` | `DiscriminatedField` | `v[key]` is missing, not a string, or not a variant tag; path `[...path, key]` |

Custom codes come from `use()` / `useAsync()` validators. `EVEN` in the source JSDoc is only an example.

---

## Notes

- **Immutability.** Every modifier clones. Containers (`ListField.item`, `ObjField.shape`, union/discriminated variants) share their children rather than cloning them.
- **Custom validators are skipped on structural failure.** `use()` / `useAsync()` validators, and therefore `list().minLength/maxLength`, run only if the type check, transform, constraints and nested items/keys all passed.
- **Constraints run after transforms.** `str().trim().min(2)` measures the trimmed string.
- **`nonempty()` is separate from presence.** A required `str()` accepts `''`.
- **`pattern()` resets `lastIndex`** before every test, so `/g` and `/y` regexes behave the same on every call. In JSON Schema, `g`/`y` are stripped from `flags`. A pattern that still has flags (`i`, `m`, `s`, `u`) is left out of the schema rather than stated wrong. Repeated patterns: the first goes to `pattern`, the rest to `allOf`.
- **Repeated constraints combine in JSON Schema.** The tightest bound wins and `enum`s intersect.
- **`coerceNumber()` leaves blank strings as strings**, so `''` / `'  '` fail with `TYPE`.
- **Async gating.** `validateSync`, `check`, `parse`, `RecordBase.check/set` and `RecordType.check/parse` throw a plain `Error` (not `ValidationFailed`) when any field in the tree has `useAsync()`. The async variants work on sync trees too.
- **Strictness resolution in `check`:** `options.strict ?? model.strict`. An explicit `{ strict: false }` makes a strict model lenient. `model(name, fields, { strict: true })` and `model(fields, { strict: true })` set it directly; projections carry it forward.
- **`obj(model)` and `list(model)` take only the model's fields.** The resulting `ObjField` is lenient until `.strict()` is called, whatever the model's strictness.
- **`check` caches** one root `ObjField` per model owner/plain-shape object and strictness in a `WeakMap`, preventing same-shape models with different root refinements from sharing a validator.
- **Output of object validation:** unknown keys are dropped, and keys that validate to `undefined` are absent (not present-with-`undefined`). Defaults are applied.
- **`publicView` and `toJsonSchema`.** `toJsonSchema(M)` is the input contract and includes `.hidden()` fields. `toJsonSchema(publicView(M))` is the output contract. Hidden state is explicit and survives presence-modifier call order.
- **Records are Proxies.** Reading a shape key returns the stored value. Methods are bound to the target. Assigning `undefined` to a shape key deletes it. Assigning to a non-shape property returns `false` from the trap, which throws `TypeError` in ESM strict mode. `set()` commits by replacing each shape key, so a key that validated to `undefined` is cleared. An absent optional value is never stored, so `toJSON()` does not print `key: undefined`.
- **`defaultValue` cloning:** a function default is called per use. Plain cloneable object values are cloned. Custom class instances are never silently flattened or shared: use a factory default or `defineDataType({ clone })`.
