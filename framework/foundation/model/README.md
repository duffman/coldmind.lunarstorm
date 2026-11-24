# @coldmind/model

Runtime data contracts: fields, models, validation, projections, records, introspection and JSON Schema.

`@coldmind/model` is persistence-independent. Define a contract once and use it at API, configuration, command or storage boundaries as appropriate.

## Example

```ts
import { model, parse, partial, str, int } from '@coldmind/model'

const Account = model('account', {
  id: int(),
  email: str().trim().email(),
}, { strict: true })

const AccountPatch = partial(Account)
const account = parse(Account, { id: 1, email: ' a@example.com ' })
```

## Main API areas

- fields: `str`, `int`, `num`, `bool`, `list`, `obj`, `union`, `discriminated`, `any`
- custom types: `defineDataType`, `dataTypeOf`, `dataTypes`
- models: `model`, `defineShape`, `fieldsOf`, `toShape`, `Infer`
- validation: `check`, `checkAsync`, `parse`, `parseAsync`, `refine`, `refineAsync`
- projections: `pick`, `omit`, `partial`, `makeOptional`, `makeRequired`, `merge`, `publicView`
- records: `record`, `RecordBase`
- schema: `introspect`, `toJsonSchema`

## Dependency position

Depends on `@coldmind/core`. It does not depend on `@coldmind/data`, `@coldmind/rpc`, ActionWire or SignalBus.

## Internal integration surface

`@coldmind/model/internal`, `@coldmind/model/define-model` and `@coldmind/model/model.key` exist for controlled package integration, especially `@coldmind/data`. Applications should prefer the public `@coldmind/model` API.

## More documentation

See [`docs/README.md`](docs/README.md), [`docs/model.api.md`](docs/model.api.md), and the examples under [`../../examples/model`](../../examples/model).

## Verify

```bash
bun --filter @coldmind/model typecheck
bun --filter @coldmind/model test
bun run example:model
```
