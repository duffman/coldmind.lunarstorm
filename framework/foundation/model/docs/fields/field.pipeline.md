# The field pipeline

Source: [`src/fields/model.field.pipeline.ts`](../../fields/base.field.ts) and the kinds beside it.

## `Field<T, O>`

- `T` is the base type the field parses and constrains.
- `O` is the output type. `optional()` adds `undefined`, `nullable()` adds `null`, `required()` and
  `default()` remove `undefined`, `StrField.oneOf()` narrows it.

State on every field:

| Member | Set by | Meaning |
|---|---|---|
| `_opts.req` | `required()` / `optional()` | `undefined` is an error |
| `_opts.null` | `nullable()` | `null` is accepted |
| `_opts.def` | `default()` | value or factory used for `undefined` input |
| `_opts.doc` | `doc()` | description |
| `_opts.hidden` | `hidden()` | left out of `publicView` |
| `_opts.typeName` | a `defineDataType` factory | the data type's name |
| `_coerceFn` | `coerce()` | runs before the type check |
| `_transformFn` | `transform()` | runs after the type check, `T → T` |
| `_sync`, `_async` | `use()`, `useAsync()` | custom validators |

Subclasses add `_constraints` (`ConstrainedField`), `_minItems` / `_maxItems` (`ListField`) and
`_strict` (`ObjField`).

## Immutability

Every modifier calls `_mod`, which calls `clone()`, mutates the clone and returns it. Arrays are
replaced, never pushed to (`c._sync = [...c._sync, fn]`), so a clone never shares a list with its
source. Container fields share their children: `ListField.item`, `ObjField.shape` and the union
variants are the same instances in the clone.

A field instance can therefore be used in any number of shapes.

## Steps per value

| # | Step | Where | Failure code |
|---|---|---|---|
| 1 | `undefined` → default. Still `undefined`: error if required, else done with `undefined` | `_prologue` | `REQ` |
| 2 | `null`: done with `null` if nullable, else error | `_prologue` | `NULL` |
| 3 | coerce | `_prologue` | `COERCE_ERROR` |
| 4 | type check | `_checkType` | `TYPE` |
| 5 | transform | `_prologue` | `TRANSFORM_ERROR` |
| 6 | structural parse: constraints, items, keys, variants | `_parseSync` / `_parseAsync` | per kind |
| 7 | `use()` validators, then `useAsync()` validators on the async path | `_runSyncTests`, `validate` | caller's |

- A default goes through steps 2–7 like any input.
- Step 7 is skipped when step 6 produced an error.
- An error with `level: 'warning'` goes to `warnings` and does not make the result invalid.
- `result.value` is set only when there are no errors.

## Sync and async

- `validateSync` throws when `hasAsync()` is true, then calls `_validateSyncUnchecked`.
- Containers override `hasAsync()` to include their children, and call `_validateSyncUnchecked`
  on them. The guard runs once, at the entry point.
- `validate` works for any field. `ObjField` and `ListField` validate their children concurrently
  (`Promise.all`). `UnionField` tries variants one at a time, in order. Async validators on one
  field run one at a time, in registration order.

## Notes per kind

- **`StrField`, `NumField`**: constraints all run; none short-circuits.
- **`ListField`**: `minLength` / `maxLength` are sync validators (step 7), so they run only when
  every item is valid. The bound is also stored in `_minItems` / `_maxItems` for `introspect`.
- **`ObjField`**: reads a key only when it is an own property of the input. A key that validates
  to `undefined` is absent from the output. Unknown keys are dropped and reported as
  `UNKNOWN_KEY`: a warning, or an error after `.strict()`.
- **`UnionField`**: the constructor derives presence from the variants. The first variant default
  is the default; the union is required when there is no default and every variant is required;
  it is nullable when any variant is. Each variant runs its own full pipeline on the value.
- **`DiscriminatedField`**: `discriminated()` adds `key: str().oneOf([tag])` to a variant that
  does not declare the key. An unknown tag fails with `DISCRIMINANT` at `[...path, key]`.

## Type-level details

- Each kind preserves explicit field state across `optional`, `nullable`, `nonNullable`, `required`, `default`, `withoutDefault` and `hidden`.
- Presence is not inferred from the output union: a required `any<undefined>()` is still a required property.
- `default()` returns `Defaulted<F>` for sibling-package compatibility and also records `defaulted: true` in the field state.
- `hidden()` is call-order independent; `hidden().optional()` and `optional().hidden()` are equivalent for `publicView`.
