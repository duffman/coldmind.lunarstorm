# Data types

Source: [`src/fields/model.field.custom.types.ts`](../../fields/data.type.ts).

A data type is a named base field. The name is the key other layers use to look up what they
need to handle the type.

```ts
// types/decimal.ts
export const decimal = defineDataType({
  name: 'decimal',
  base: () => guard(any<Decimal>(), (v) => v instanceof Decimal, 'Not a Decimal'),
  schema: { type: 'string', format: 'decimal' },
})

const Invoice = model('invoice', { id: int(), total: decimal() })
```

## How it works

- `defineDataType(spec)` stores the spec in a module-level `Map` keyed by `spec.name` and returns
  a factory. It throws when the name is already registered.
- Each factory call runs `spec.base()` and clones the result with `_opts.typeName = spec.name`.
- `_copyBase` spreads `_opts`, so the name survives every modifier, clone and projection.
- `dataTypeOf(field | name)` returns the spec, or `undefined` for a plain field.
- `dataTypes()` returns every spec in registration order.
- `clearDataTypes()` (from `@coldmind/model/internal`) empties the registry, for tests and watch mode.

The registry is process-wide. Define a data type once, at module load.

## Who reads the name

| Reader | Effect |
|---|---|
| `introspectField` | reports `type: <name>`; `kind` stays the base field's kind |
| `toJsonSchema` | merges `spec.schema` over the schema derived from the base field; `spec.schema` wins |
| other packages | `DataTypeSpec` is an interface, so a package can declaration-merge its own keys into it and read them back through `dataTypeOf`. The source comment names `sql` and `codec`, added by `@coldmind/data/schema`. This module does not read them. |

For a base of `any()`, `toJsonSchema` emits `additionalProperties: true` only when the spec has no
`schema`.

## `guard`

`guard(any<T>(), check, msg, code = 'TYPE')` attaches a `use()` validator to an `AnyField`. It is
the usual `base` for a type whose values are class instances, because `any()` accepts everything
on its own.
