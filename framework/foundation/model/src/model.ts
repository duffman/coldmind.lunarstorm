/**
 * @module @coldmind/model
 * Fields, models, projections, validation and records. Knows nothing about storage.
 *
 *   fields       `str`, `int`, `num`, `bool`, `list`, `obj`, `any` — immutable, chainable
 *   data types   `defineDataType({ name, base, schema, sql?, codec? })` — one spec per extension type
 *   models       `model(name?, fields)` → handles · `toShape` · `Infer<typeof M>`
 *   projections  `pick`, `omit`, `partial`, `makeOptional`, `makeRequired`, `merge` — all take handles; `publicView` drops `.hidden()` fields
 *   validation   `check` / `checkAsync` → result · `parse` / `parseAsync` → value or `ValidationFailed`
 *   records      `record(model)` → instances that validate themselves
 *   introspect   `introspect` → a JSON description of a model or shape
 *
 * Source is grouped by category: `fields/`, `models/`, `validation/`, `schema/`. See `docs/README.md`.
 *
 * @example
 * const Account = model('account', { id: int(), email: str().trim().email() })
 * const Signup = makeOptional(Account, Account.id)
 * const signup = parse(Signup, body) // { id?: number; email: string }
 */

// ── fields ──────────────────────────────────────────────────
export {
  Field,
  type AsyncValidator,
  type Defaulted,
  type FieldState,
  type FieldStateOf,
  type FieldOpts,
  type InitialFieldState,
  type NonNullableState,
  type SyncValidator,
} from '@coldmind/model/src/fields/model.field.pipeline.ts'
export {
  any,
  bool,
  discriminated,
  int,
  list,
  listOf,
  num,
  obj,
  str,
  union,
  AnyField,
  BoolField,
  DiscriminatedField,
  ListField,
  NumField,
  ObjField,
  StrField,
  UnionField,
  type DiscValue,
} from '@coldmind/model/src/fields/model.fields.ts'

// ── extension data types ────────────────────────────────────
export { dataTypeOf, dataTypes, defineDataType, guard, type DataTypeSpec } from '@coldmind/model/src/fields/model.field.custom.types.ts'

// ── shapes and validation ───────────────────────────────────
export {
  type Infer,
  type InferField,
  type InferInputShape,
  type InferShape,
  type Shape,
  type SomeField,
} from '@coldmind/model/src/models/model.shape.inference.ts'
export {
  check,
  checkAsync,
  parse,
  parseAsync,
  refine,
  refineAsync,
  unwrapResult,
  ValidationFailed,
  type ShapeOptions,
} from '@coldmind/model/src/validation/model.validation.ts'

// ── models ──────────────────────────────────────────────────
export {
  defineShape,
  fieldsOf,
  isModel,
  model,
  nameOf,
  toShape,
  type AnyModel,
  type AsShape,
  type FieldRef,
  type Model,
  type ModelMeta,
  type ModelOptions,
  type NameOf,
  type Optionalized,
  type PatchOptionalized,
  type Requiredized,
  type ShapeInput,
  type ShapeOf,
} from '@coldmind/model/define-model'
export { toJsonSchema, type JsonSchema } from '@coldmind/model/src/schema/model.schema.json.ts'
export {
  makeOptional,
  makeRequired,
  merge,
  omit,
  partial,
  pick,
  publicView,
  type RefTuple,
  type Visible,
} from '@coldmind/model/src/models/model.projections.ts'

// ── records (DTO instances) ─────────────────────────────────
export { record, RecordBase, type RecordOf, type RecordType } from '@coldmind/model/src/validation/model.record.ts'

// ── introspection ───────────────────────────────────────────
export {
  introspect,
  introspectField,
  type ConstraintInfo,
  type FieldInfo,
  type FieldKind,
  type ShapeInfo,
} from '@coldmind/model/src/schema/model.schema.introspection.ts'
