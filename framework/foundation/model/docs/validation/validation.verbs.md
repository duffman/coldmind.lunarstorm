# Validation verbs

Source: [`src/validation/model.validation.ts`](../../validation/model.validate.ts).

| Verb | Returns | Throws |
|---|---|---|
| `check(target, data, options?)` | `ValidationResult` | `Error` when the target has async validators |
| `checkAsync(...)` | `Promise<ValidationResult>` | — |
| `parse(...)` | the value | `ValidationFailed`; `Error` as for `check` |
| `parseAsync(...)` | `Promise` of the value | `ValidationFailed` |
| `unwrapResult(result, context?)` | `result.value` | `ValidationFailed` |

`target` is a model or a plain shape. `parse` is `unwrapResult(check(...))`.

## One code path

`rootOf(target, options)` wraps the target's fields in an `ObjField` and validates the data with
it at path `[]`. A top-level check and a nested `obj()` are the same code, with the same codes:
`TYPE` for non-object input, `REQ` for `undefined`, `NULL` for `null`, `UNKNOWN_KEY` for an
undeclared key.

The root field is cached in a `WeakMap` keyed by the shape object, one entry per strictness.

## Strictness

`options.strict ?? model.strict`. An explicit `{ strict: false }` makes a strict model lenient. A
plain shape has no strictness of its own.

Strict turns `UNKNOWN_KEY` from a warning into an error. Either way the key is not in the value.

## The value

Defaults applied, transforms applied, unknown keys dropped, keys that validated to `undefined`
absent. It is a new object; the input is not modified.

## `ValidationFailed`

Carries `errors` and `warnings`. `message` is `formatErrors(errors)` from `@coldmind/core`,
prefixed with `<context>: ` when a context was given.

## Storage handoff (internal)

Two functions exported through `@coldmind/model/internal` and imported by `@coldmind/data/query`.

**`hasAsyncValidation(target)`**: whether the target needs the async verbs.

**`canonicalFor(target, value)`**: returns `value` when it is known to be an unchanged result of a
valid check against the same fields, else `undefined`.

- After every valid `check` / `checkAsync`, `remember` records the result object in a `WeakMap`
  together with the fields it was checked against and a copy of its values. It records only a
  result whose values are all scalars; one nested object or array and nothing is recorded.
- `canonicalFor` returns the value only when all of these hold: it was recorded; `target` has the
  same keys with the same field instances; the value has the same keys; every value is
  `Object.is`-equal to the recorded one.

`check` and `checkAsync` never consult this record. They always validate.

## Error codes

The full table is in [`API_SPEC.md`](../../API_SPEC.md#validation-error-codes).
