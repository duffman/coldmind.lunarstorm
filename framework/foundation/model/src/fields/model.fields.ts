/**
 * @module model.fields
 * Concrete field types and their factories.
 */

export { ConstrainedField, Field, type Defaulted, type ParseOut } from '@coldmind/model/src/fields/model.field.pipeline.ts'
export type { SomeField } from '@coldmind/model/src/models/model.shape.inference.ts'

export { StrField, str } from '@coldmind/model/src/fields/model.field.string.ts'
export { NumField, num, int } from '@coldmind/model/src/fields/model.field.number.ts'
export { BoolField, bool } from '@coldmind/model/src/fields/model.field.boolean.ts'
export { AnyField, any } from '@coldmind/model/src/fields/model.field.any.ts'
export { ListField, list, listOf } from '@coldmind/model/src/fields/model.field.list.ts'
export { ObjField, obj } from '@coldmind/model/src/fields/model.field.object.ts'
export { UnionField, DiscriminatedField, union, discriminated, type DiscValue } from '@coldmind/model/src/fields/model.field.union.ts'
