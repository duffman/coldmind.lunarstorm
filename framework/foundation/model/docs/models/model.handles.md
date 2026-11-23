# Shapes, models and handles

Source: [`src/models/model.shape.inference.ts`](../../models/shape.types.ts),
[`src/models/model.identity.ts`](../../models/model.key.ts),
[`src/models/model.definition.ts`](../../models/model.define.ts).

## The three things

| Thing | What it is |
|---|---|
| `Shape` | `Record<string, SomeField>`, a plain object of fields |
| `Model<N, S>` | an object with one `FieldRef` per field, plus metadata under the `MODEL` symbol |
| `ShapeInput` | `Shape \| AnyModel`, what every consumer accepts |

A handle is `FieldRef<N, K, F>`: `{ model: N, key: K, field: F, [MODEL_OWNER]: owner }`. `Account.email` is the handle for the `email` field of the model named `account`; the owner token identifies the exact model instance at runtime.

## What `model()` builds

`model(name, fields)` and `model(fields)` call `buildModel`, which:

1. snapshots and freezes `fields`;
2. creates one stable owner token and one frozen `FieldRef` per field;
3. defines `[MODEL] = { name, fields, owner, strict?, refinements? }` as a non-enumerable property;
4. freezes the model object.

Every string key on a model is therefore a field, and `Object.keys(model)` is the field list. An
unnamed model has the name `''`. `defineShape` is the same function as `model`.

`strict` can be set directly with `model(name, fields, { strict: true })` or `model(fields, { strict: true })`. `buildModel` remains available to sibling packages through `@coldmind/model/internal`.

`MODEL` is `Symbol('modellera.model')`. It lives in its own file with no imports.

## Reading a model

| Function | Returns |
|---|---|
| `isModel(x)` | whether `x` carries `MODEL` |
| `metaOf(x)` (internal) | `{ name, fields, owner, strict?, refinements? }`; for a plain shape the shape object itself is the owner |
| `toShape(x)` | the shape of a model, or the shape itself |
| `fieldsOf(m)` | the shape, typed as `ShapeOf<M>` |
| `nameOf(m)` | the name, typed as `NameOf<M>` |

Code that accepts a `ShapeInput` calls `metaOf` first and works on the result.

## Types

| Type | Gives |
|---|---|
| `InferField<F>` | the output type of a field |
| `InferShape<S>` | the parsed/output value type; optional properties come from explicit field state, not from `undefined` in the value union |
| `Infer<X>` | `InferShape` of a model's shape, or of a shape |
| `ShapeOf<M>`, `NameOf<M>` | a model's shape and name |
| `AsShape<X>` | the shape behind a `ShapeInput` type |
| `InferInputShape<S>` | the accepted input shape; optional/defaulted fields may be omitted |
| `Optionalized<F>`, `PatchOptionalized<F>`, `Requiredized<F>` | optional; optional with defaults suppressed; required field forms |

## Why handles

- Renaming a field is a compile error at every place the handle is used.
- A handle carries its model's name type, so `pick(Account, Order.id)` does not type-check.

The type check compares name, key and field type, so different named models are rejected statically. TypeScript is structural, so two separate models with the same name/key/field type can still be assignable. Runtime projection checks therefore also compare the private owner token and exact field instance, rejecting those foreign handles deterministically.
