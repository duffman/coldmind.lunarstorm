/**
 * @module model.field.custom.types
 * `defineDataType`: extension data types — one spec, in one file, carrying everything the type needs.
 *
 * ---
 * A data type is a base field (its validation and output type), a name, and
 * whatever each layer needs to handle it. The whole definition is one object
 * passed to one call, so an extension is a single file:
 *
 *   name     the key every layer looks the type up by
 *   base     validation and the TypeScript value type
 *   schema   JSON Schema keywords merged over the base field's schema
 *   sql      column type per dialect          ┐ declared by @coldmind/db via
 *   codec    value ↔ driver value per dialect ┘ declaration merging
 *
 * The name rides on the field as `_opts.typeName`, so it survives cloning,
 * `fieldsOf`, projections and every modifier. This module knows nothing about
 * storage: `sql` and `codec` are merged into {@link DataTypeSpec} by the db
 * layer and read back by it from {@link dataTypeOf}.
 */
import type { Path } from '@coldmind/core'
import type { AnyField } from '@coldmind/model/src/fields/model.field.any.ts'
import type { JsonSchema } from '@coldmind/model/src/schema/model.schema.json.ts'
import type { SomeField } from '@coldmind/model/src/models/model.shape.inference.ts'
import type { InferField } from '@coldmind/model/src/models/model.shape.inference.ts'

/**
 * The complete definition of an extension data type. Open for declaration merging:
 * `@coldmind/data/schema` adds `sql` and `codec`.
 *
 * @typeParam F - the base field; its output type is the data type's value type
 */
export interface DataTypeSpec<F extends SomeField = SomeField> {
  /** Unique across the process. Reported by `introspect` and carried on every field the factory makes. */
  readonly name: string
  /** A fresh base field per call: type check, constraints, validators. */
  readonly base: () => F
  /** Merged over the schema the base field produces in `toJsonSchema`. */
  readonly schema?: JsonSchema
  /** Clone a value while preserving extension-specific class/prototype semantics. */
  readonly clone?: (value: InferField<F>) => InferField<F>
}

/** The process-wide registry, keyed by name, in registration order. */
const DATA_TYPES = new Map<string, DataTypeSpec>()

/** A clone of `f` carrying the data type's name. */
function named<F extends SomeField>(f: F, spec: DataTypeSpec<F>): F {
  const c = f.clone()
  c._opts = {
    ...c._opts,
    typeName: spec.name,
    ...(spec.clone ? { cloneValue: spec.clone as (value: any) => any } : {}),
  }
  return c
}

/**
 * Register an extension data type and return its field factory. Call once per name, at module load.
 *
 * @example
 * // types/decimal.ts — the whole extension
 * import { defineDataType, guard, any } from '@coldmind/model'
 * import { Decimal } from 'decimal.js'
 *
 * export const decimal = defineDataType({
 *   name: 'decimal',
 *   base: () => guard(any<Decimal>(), (v) => v instanceof Decimal, 'Not a Decimal'),
 *   schema: { type: 'string', format: 'decimal' },
 *   sql: { postgres: 'NUMERIC(19,4)', mysql: 'DECIMAL(19,4)', default: 'TEXT' },
 *   codec: { toDb: (d: Decimal) => d.toString(), fromDb: (s: unknown) => new Decimal(String(s)) },
 * })
 *
 * // anywhere
 * const Invoice = model('invoice', { id: int().primaryKey(), total: decimal() }, () => ({}))
 *
 * @returns a factory; each call yields a fresh field tagged with `spec.name`
 * @throws {Error} when `name` is already registered
 */
export function defineDataType<F extends SomeField>(spec: DataTypeSpec<F>): () => F {
  if (DATA_TYPES.has(spec.name)) {
    throw new Error(`Data type "${spec.name}" is already defined`)
  }
  DATA_TYPES.set(spec.name, spec as unknown as DataTypeSpec)
  return () => named(spec.base(), spec)
}

/**
 * The spec behind a data type, by name or by a field made from it. `undefined` for a plain field.
 *
 * @example
 * dataTypeOf(decimal())?.name // 'decimal'
 * dataTypeOf('decimal')
 */
export function dataTypeOf(x: SomeField | string): DataTypeSpec | undefined {
  const name = typeof x === 'string' ? x : x._opts.typeName
  return name === undefined ? undefined : DATA_TYPES.get(name)
}

/** Every registered data type, in registration order. */
export function dataTypes(): readonly DataTypeSpec[] {
  return [...DATA_TYPES.values()]
}

/** @internal for tests and re-registration in watch mode. */
export function clearDataTypes(): void {
  DATA_TYPES.clear()
}

/**
 * Attach a runtime type check to an `any<T>()`, which accepts everything by default.
 * The usual `base` for a data type whose values are class instances.
 *
 * @example
 * guard(any<Date>(), (v) => v instanceof Date, 'Not a Date')
 *
 * @param code - the error code reported on failure; defaults to `'TYPE'`
 */
export function guard<T, O>(
  field: AnyField<T, O>,
  check: (v: T) => boolean,
  msg: string,
  code = 'TYPE',
): AnyField<T, O> {
  return field.use((v: T, ctx: { path: Path }) =>
    check(v) ? null : { path: ctx.path, msg, code, level: 'error' as const },
  )
}
