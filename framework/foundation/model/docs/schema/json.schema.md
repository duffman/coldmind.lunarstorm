# JSON Schema

Source: [`src/schema/model.schema.json.ts`](../../schema/json.schema.ts).

`toJsonSchema(target)` emits draft 2020-12 for a model, a shape or a single field.

| Target | Output |
|---|---|
| model or shape | `{ $schema, title?, type: 'object', properties, required?, additionalProperties: !strict }` |
| field | `{ $schema, ...fieldSchema }` |

`title` is the model name, left out when it is `''`. `strict` is the model's.

## Per kind

| Kind | Schema |
|---|---|
| `string`, `number`, `boolean` | `type` |
| `list` | `type: 'array'`, `items` |
| `object` | `type: 'object'`, `properties`, `required` when non-empty, `additionalProperties: !_strict` |
| `any` | `additionalProperties: true`, no `type`; nothing when the field is a data type with its own `schema` |
| `union` | `{ anyOf: [...] }` |
| `discriminated` | `{ oneOf: [...] }`; each variant gets `properties[key] = { const: tag, type: 'string' }` and `key` in `required` |

Then, for every kind except union and discriminated:

1. `doc` → `description`; a literal default → `default`. A factory default is not emitted.
2. Constraint params are applied (below).
3. A data type's `schema` is merged last and wins.

Finally `nullable()` wraps the result in `{ anyOf: [schema, { type: 'null' }] }`.

Union and discriminated fields return before steps 1–3: they get no `description`, no `default`
and no data type overlay.

A property is in `required` when its field is required and has no default.

## Constraint params

`applyParams` reads `params` from every constraint `introspectField` reports, the synthetic list
bounds included.

| Param | Rule when repeated |
|---|---|
| `minLength`, `minimum`, `exclusiveMinimum`, `minItems` | the larger |
| `maxLength`, `maximum`, `exclusiveMaximum`, `maxItems` | the smaller |
| `enum` | intersection |
| `pattern` | first → `pattern`, the rest → `allOf: [{ pattern }, …]` |
| `format` | the last |
| `type: 'integer'` | sets `type` |

A pattern whose `flags` is non-empty is left out, because JSON Schema has no regex flags.
`StrField.pattern` already strips `g` and `y`, which do not change what matches.

Any other param key is ignored here and still reported by `introspect`.

## Input and output contracts

`toJsonSchema(M)` includes `.hidden()` fields: it is what the model accepts.
`toJsonSchema(publicView(M))` is what it exposes.
