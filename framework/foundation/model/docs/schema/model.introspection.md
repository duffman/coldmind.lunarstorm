# Introspection

Source: [`src/schema/model.schema.introspection.ts`](../../schema/model.introspect.ts).

`introspect(modelOrShape)` returns `{ name, fields, order }`. `introspectField(field)` returns one
`FieldInfo`. This is the supported way to read a field from outside the module; `_opts` and
`_constraints` are internals and may change.

## Where each value comes from

| `FieldInfo` | Source |
|---|---|
| `kind` | the field's class, through `kindOf` |
| `type` | `_opts.typeName`, present only for a data type |
| `required`, `nullable`, `hidden`, `doc` | `_opts` |
| `hasDefault` | `_opts.def !== undefined` |
| `default` | `_opts.def` when it is not a function; a factory is never called |
| `async` | `hasAsync()`, which includes nested fields |
| `constraints` | `_constraints` of a `StrField` / `NumField` |
| `item` | `ListField.item` |
| `fields`, `strict` | `ObjField.shape`, `ObjField._strict` |
| `variants` | union or discriminated variants, in declaration order |
| `discriminant` | `DiscriminatedField.discKey` |

## Details

- `kindOf` is an `instanceof` chain ending in `'any'`. A field class it does not list is reported
  as `'any'`.
- A data type reports the kind of its base and its own name in `type`.
- A list gets two synthetic constraints, `MIN_LENGTH` and `MAX_LENGTH`, with
  `params: { minItems }` / `{ maxItems }`. They are validators at run time, not constraints.
- A discriminated variant's `doc` is replaced by its tag.
- `order` is declaration order, the order a generator should emit.
- `use()` and `useAsync()` validators are not listed. `async` is the only trace of them.

## Consumers in this module

`toJsonSchema` calls `introspectField` for every field, so the schema and the description cannot
disagree.
