# @coldmind/model source layout

Source is under `packages/model/src/`, tests under `packages/model/tests/` and examples under the workspace's `examples/model/`. No root-level `src` or undifferentiated `index.ts` files.

| Area | Responsibilities | Representative filename |
| --- | --- | --- |
| `src/fields/` | Value field classes, validation pipelines, data-type extension registry | `model.field.pipeline.ts`, `model.field.string.ts`, `model.field.custom.types.ts` |
| `src/models/` | Model definition, identity, shape inference, handle-based projections | `model.definition.ts`, `model.identity.ts`, `model.shape.inference.ts` |
| `src/validation/` | Sync/async check/parse, record facade | `model.validation.ts`, `model.record.ts` |
| `src/schema/` | Introspection and JSON Schema generation | `model.schema.introspection.ts`, `model.schema.json.ts` |
| `src/model.ts` | Explicit stable package root exports | `@coldmind/model` |
| `src/model.internal.ts` | Small, explicit cross-package implementation surface | `@coldmind/model/internal` |

## Import direction

The `model` package imports only the `core` package. `data` may import published `model` APIs and explicitly permitted internals. Internal source imports use relative paths; consumers import `@coldmind/model` rather than a physical `.ts` filename.

## Naming

File names are globally descriptive in an IDE: `model.field.string.ts`, not `string.ts`; `model.shape.inference.ts`, not `types.ts`. Dots separate domain, subject and role. One public barrel for the package and one dedicated fields barrel; no generic `index.ts` file.

## Entrypoints

`@coldmind/model` is the stable public surface. `@coldmind/model/internal` is a deliberate sibling-package seam. `@coldmind/model/define-model` and `@coldmind/model/model.key` remain compatibility aliases in the export map, without redundant source-file facades. Arbitrary deep source imports are intentionally not exported.

## Source dependencies

`model.identity.ts` carries model symbols. `model.definition.ts` creates handles. `model.shape.inference.ts` derives types and has only type-level references to definitions. `model.field.pipeline.ts` is the field base, extended by other field classes. `model.validation.ts` coordinates parsed shapes. `model.schema.introspection.ts` and `model.schema.json.ts` are outputs over the same field definitions. The existing type-only circular references are preserved without adding new runtime cycles.
