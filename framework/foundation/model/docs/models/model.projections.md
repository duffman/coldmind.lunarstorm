# Projections

Source: [`src/models/model.projections.ts`](../../models/model.projections.ts).

A projection takes a model and returns a new model. All seven follow one pattern: `metaOf(input)`,
build a new shape, `buildModel(name, shape, strict)`.

| Function | Input | Fields of the result | Field instances |
|---|---|---|---|
| `pick(m, ...refs)` | model + handles | the named ones, in the order the handles were given | shared |
| `omit(m, ...refs)` | model + handles | all but the named ones | shared |
| `partial(x)` | model or shape | all, each `.optional().withoutDefault()` | new |
| `makeOptional(m, ...refs)` | model + handles | all; the named ones `.optional()` | new for the named ones |
| `makeRequired(m, ...refs)` | model + handles | all; the named ones `.required()` | new for the named ones |
| `merge(a, b)` | two models or shapes | `{ ...a, ...b }`; `b` wins a collision | shared |
| `publicView(x)` | model or shape | all without `_opts.hidden` | shared |

## What carries over

- **Name and strictness** of the source. For `merge`, those of `a`.
- **Key order**: declaration order. `pick` uses the handle order instead, and `merge` appends
  `b`'s new keys after `a`'s.
- **Data type names, docs, defaults, validators**: they are on the field, and the field is either
  shared or cloned.

`makeOptional` preserves defaults. `partial` is PATCH semantics: it makes every field optional and suppresses JS defaults, so an omitted field stays omitted instead of being manufactured by validation.

## What does not

- **Handles.** The result has its own. After a projection that changes a field's type, the
  source's handle no longer fits, so use the result's:

  ```ts
  const Patch = partial(Account)
  const EmailPatch = makeRequired(Patch, Patch.email)
  ```

- **The cached validator.** `check` caches by shape object, and a projection builds a new shape.

## Handle parameters

`RefTuple<N, S, Ks>` types the rest parameter: one `FieldRef<N, K, S[K]>` per key. The key tuple
`Ks` is inferred from the handles, which is how the result type narrows to exactly those fields.

`refKeys` verifies the key, exact field instance, and a private owner token shared by the model and its handles. It rejects a handle from another model even when both models have the same name, key and field kind. Different model names/keys are rejected by TypeScript; exact instance ownership is enforced at runtime.

## `publicView` and `hidden()`

`publicView` filters at run time on `_opts.hidden` and at the type level from the field's explicit `hidden` state. Presence modifiers preserve that state, so call order does not matter. `toJsonSchema(M)` is the input contract;
`toJsonSchema(publicView(M))` is the output contract.
