/**
 * @module model.schema.json
 * `toJsonSchema`: a JSON Schema (draft 2020-12) for a model, shape or field.
 *
 * Built from the same field tree `introspect` walks. Declarative constraints are
 * combined conservatively so schema emission does not weaken chained runtime rules;
 * arbitrary `use()` validators remain runtime-only.
 */
import { dataTypeOf } from '@coldmind/model/src/fields/model.field.custom.types.ts'
import { type ShapeInput, metaOf } from '@coldmind/model/define-model'
import { DiscriminatedField, Field, ObjField, UnionField } from '@coldmind/model/src/fields/model.fields.ts'
import { type FieldInfo, introspectField } from '@coldmind/model/src/schema/model.schema.introspection.ts'
import type { Shape, SomeField } from '@coldmind/model/src/models/model.shape.inference.ts'

/** A JSON Schema object. Additional keywords are allowed. */
export type JsonSchema = {
  readonly $schema?: string
  readonly $id?: string
  readonly title?: string
  readonly type?: string | readonly string[]
  readonly properties?: Readonly<Record<string, JsonSchema>>
  readonly required?: readonly string[]
  readonly additionalProperties?: boolean | JsonSchema
  readonly items?: JsonSchema
  readonly prefixItems?: readonly JsonSchema[]
  readonly allOf?: readonly JsonSchema[]
  readonly anyOf?: readonly JsonSchema[]
  readonly oneOf?: readonly JsonSchema[]
  readonly enum?: readonly unknown[]
  readonly const?: unknown
  readonly format?: string
  readonly pattern?: string
  readonly minLength?: number
  readonly maxLength?: number
  readonly minimum?: number
  readonly maximum?: number
  readonly exclusiveMinimum?: number
  readonly exclusiveMaximum?: number
  readonly minItems?: number
  readonly maxItems?: number
  readonly description?: string
  readonly default?: unknown
  readonly nullable?: boolean
}

const DRAFT = 'https://json-schema.org/draft/2020-12/schema'

/** Bound keywords by direction, so a repeated constraint keeps the tightest value. */
const LOWER = new Set(['minLength', 'minimum', 'exclusiveMinimum', 'minItems'])
const UPPER = new Set(['maxLength', 'maximum', 'exclusiveMaximum', 'maxItems'])

/**
 * Constraint params onto the schema. Repeated constraints all apply, so they
 * combine: the tightest bound wins, enums intersect, extra patterns go to `allOf`.
 * JSON Schema has no regex flags, so a pattern that needs `i`/`m`/`s`/`u` is
 * left out rather than stated wrong.
 */
const applyParams = (schema: Record<string, unknown>, info: FieldInfo): void => {
  const patterns: string[] = []
  const formats: string[] = []
  for (const c of info.constraints) {
    const p = c.params
    if (!p) continue
    if (typeof p.pattern === 'string' && !p.flags) patterns.push(p.pattern)
    for (const [k, v] of Object.entries(p)) {
      if (k === 'type' && v === 'integer') schema.type = 'integer'
      else if (k === 'format' && typeof v === 'string') formats.push(v)
      else if (k === 'enum') {
        const prev = schema.enum as unknown[] | undefined
        schema.enum = prev ? prev.filter((x) => (v as unknown[]).includes(x)) : v
      } else if (typeof v === 'number' && (LOWER.has(k) || UPPER.has(k))) {
        const prev = schema[k] as number | undefined
        schema[k] = prev === undefined ? v : LOWER.has(k) ? Math.max(prev, v) : Math.min(prev, v)
      }
    }
  }
  const [first, ...rest] = patterns
  if (first !== undefined) schema.pattern = first
  const clauses: JsonSchema[] = rest.map((pattern) => ({ pattern }))

  const [firstFormat, ...restFormats] = formats
  if (firstFormat !== undefined) schema.format = firstFormat
  clauses.push(...restFormats.map((format) => ({ format })))

  if (clauses.length > 0) {
    schema.allOf = [...((schema.allOf as readonly JsonSchema[] | undefined) ?? []), ...clauses]
  }
}

/** `nullable()` as `anyOf: [schema, { type: 'null' }]`. The `nullable` keyword is never emitted. */
const wrapNullable = (schema: JsonSchema, info: FieldInfo): JsonSchema => {
  if (!info.nullable) return schema
  return { anyOf: [schema, { type: 'null' }] }
}

/** The schema of one field, without `$schema`. A data type's own `schema` is merged last and wins. */
function fieldToSchema(field: SomeField): JsonSchema {
  const info = introspectField(field)
  if (field instanceof UnionField) {
    const schema: Record<string, unknown> = {
      anyOf: field.variants.map((v: SomeField) => fieldToSchema(v)),
    }
    if (info.doc) schema.description = info.doc
    if (info.hasDefault && info.default !== undefined) schema.default = info.default
    applyParams(schema, info)
    const extra = dataTypeOf(field)?.schema
    if (extra) Object.assign(schema, extra)
    return wrapNullable(schema as JsonSchema, info)
  }
  if (field instanceof DiscriminatedField) {
    const oneOf = Object.entries(field.variants).map(([tag, variant]) => {
      const s = fieldToSchema(variant) as JsonSchema & { properties?: Record<string, JsonSchema> }
      const existing = s.properties?.[field.discKey]
      const properties = {
        ...(s.properties ?? {}),
        [field.discKey]: existing
          ? { allOf: [existing, { const: tag, type: 'string' }] }
          : { const: tag, type: 'string' },
      }
      const required = [...new Set([...(s.required ?? []), field.discKey])]
      return { ...s, type: 'object', properties, required }
    })
    const schema: Record<string, unknown> = { oneOf }
    if (info.doc) schema.description = info.doc
    if (info.hasDefault && info.default !== undefined) schema.default = info.default
    applyParams(schema, info)
    const extra = dataTypeOf(field)?.schema
    if (extra) Object.assign(schema, extra)
    return wrapNullable(schema as JsonSchema, info)
  }

  const extra = dataTypeOf(field)?.schema
  const schema: Record<string, unknown> = {}
  if (info.doc) schema.description = info.doc
  if (info.hasDefault && info.default !== undefined) schema.default = info.default

  switch (info.kind) {
    case 'string':
      schema.type = 'string'
      break
    case 'number':
      schema.type = 'number'
      break
    case 'boolean':
      schema.type = 'boolean'
      break
    case 'list':
      schema.type = 'array'
      if (info.item) schema.items = fieldToSchema((field as unknown as { item: SomeField }).item)
      break
    case 'object':
      schema.type = 'object'
      if (field instanceof ObjField) {
        const shape = objectSchema(field.shape)
        schema.properties = shape.properties
        if (shape.required.length > 0) schema.required = shape.required
        schema.additionalProperties = !field._strict
      }
      break
    default:
      // A data type's own schema says what an otherwise-unchecked value is.
      if (!extra) schema.additionalProperties = true
      break
  }

  applyParams(schema, info)
  if (extra) Object.assign(schema, extra)
  return wrapNullable(schema as JsonSchema, info)
}

/** `properties` and `required` of a shape. A key is required when its field is required and has no default. */
function objectSchema(shape: Shape): {
  properties: Record<string, JsonSchema>
  required: string[]
} {
  const properties: Record<string, JsonSchema> = {}
  const required: string[] = []
  for (const [k, f] of Object.entries(shape)) {
    properties[k] = fieldToSchema(f)
    if (f._opts.req && f._opts.def === undefined) required.push(k)
  }
  return { properties, required }
}

/**
 * JSON Schema for a model, a shape, or a single field. This is the input contract and
 * includes `.hidden()` fields; `toJsonSchema(publicView(M))` is the output contract.
 *
 * @example
 * toJsonSchema(Account)
 * // { $schema, type: 'object', properties: { id: { type: 'integer' }, … }, required: ['id', 'email'] }
 */
export function toJsonSchema(target: ShapeInput | SomeField): JsonSchema {
  if (target instanceof Field) return { $schema: DRAFT, ...fieldToSchema(target) }
  const { name, fields, strict } = metaOf(target)
  const { properties, required } = objectSchema(fields)
  return {
    $schema: DRAFT,
    ...(name ? { title: name } : {}),
    type: 'object',
    properties,
    ...(required.length > 0 ? { required } : {}),
    additionalProperties: !strict,
  }
}
