# @coldmind/model — maintainer guide

Read [the model API](model.api.md) for public symbols, [the package organization](architecture/module.layout.md) for new paths, and the [workspace overview](../../../ARCHITECTURE.md) for naming rules and package boundaries.

## Deep dives

- [Extension points](architecture/extension.points.md)
- [Field pipeline](fields/field.pipeline.md)
- [Custom data types](fields/data.types.md)
- [Model handles](models/model.handles.md)
- [Handle-based projections](models/model.projections.md)
- [Validation methods](validation/validation.verbs.md)
- [Records](validation/model.records.md)
- [Introspection](schema/model.introspection.md)
- [JSON Schema](schema/json.schema.md)

## Commands (workspace root)

```bash
bun install
bun run typecheck
bun run test --project @coldmind/model
bun run example:model
bun run example:model records
```

Source: `packages/model/src/`, tests: `packages/model/tests/`, separate examples: `examples/model/`. The executable end-to-end application is `apps/demo/`.
