# Records

Source: [`src/validation/model.record.ts`](../../validation/model.record.ts).

`record(model)` returns a `RecordType`: stateless validation plus `create` for live instances.
A record is built from a model, so project first and record the projection:

```ts
const Public = record(pick(UserModel, UserModel.name))
```

`record` throws when a field name is a record member: `constructor`, `check`, `checkAsync`, `set`,
`setAsync`, `assign`, `clone`, `toJSON`, `_data`, `$modelName`, `shape`. The list is computed from
`RecordBase.prototype`, so a new method becomes reserved automatically.

## The instance is a Proxy

The `RecordBase` constructor returns a Proxy over itself.

| Access | Behaviour |
|---|---|
| read a shape key | the stored value from `_data` |
| read anything else | the member on the instance; a function is bound to the instance, which is what lets a method reach the private `#commit` |
| write a shape key | stored; `undefined` deletes the key |
| write anything else | rejected, which throws `TypeError` in strict mode |

## Initial values

Per field: the value from `initial` when it is an own property; else the default; else `null` when
the field is nullable. An `undefined` result is not stored, so `toJSON()` has no `key: undefined`.

## What validates

| Operation | Validates | Strictness |
|---|---|---|
| `Type.create(initial)` | no | — |
| `Type.parse(data)` / `parseAsync` | yes, then `create(value)`; throws `ValidationFailed` | the model's |
| `Type.check(data)` / `checkAsync` | yes | the model's |
| `instance.key = value` | no | — |
| `instance.assign(data)` | no | — |
| `instance.check()` / `checkAsync()` | yes, current values, changes nothing | lenient |
| `instance.set(patch)` / `setAsync` | yes, `{ ...current, ...patch }`; applies only when valid | lenient |

The instance methods validate against the plain shape, which carries no strictness. The type
methods validate against the model.

`set` commits by replacing every shape key from the validated value, so a key that validated to
`undefined` is cleared. On failure nothing changes.

The sync methods throw `Error` when the shape has async validators.

## Copies

`clone()` is deep (`structuredClone` of the values). `toJSON()` is a shallow copy.
