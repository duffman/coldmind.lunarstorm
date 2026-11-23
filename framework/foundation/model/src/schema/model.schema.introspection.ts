/**
 * @module model.schema.introspection
 * `introspect` / `introspectField`: fields and shapes as plain data for docs, schemas and forms.
 */
import type { ValidationLevel } from '@coldmind/core'
import { type ShapeInput, metaOf } from '@coldmind/model/define-model'
import type { Constraint } from '@coldmind/model/src/fields/model.field.pipeline.ts'
import {
  BoolField,
  DiscriminatedField,
  ListField,
  NumField,
  ObjField,
  StrField,
  UnionField,
} from '@coldmind/model/src/fields/model.fields.ts'
import type { Shape, SomeField } from '@coldmind/model/src/models/model.shape.inference.ts'

// ─────────────────────────────────────────────────────────────
// INTROSPECTION
//
// Everything a doc generator, a schema emitter or a form builder needs, as
// data. Reading `_opts` and `_constraints` from the outside is what this
// replaces: the shape of the answer is stable even if the field internals
// are not.
// ─────────────────────────────────────────────────────────────

/** A field's base kind, by class. A data type reports the kind of its base. */
export type FieldKind =
  | 'string'
  | 'number'
  | 'boolean'
  | 'list'
  | 'object'
  | 'any'
  | 'union'
  | 'discriminated'

/** One declarative constraint: its error code, message, level and bound values. */
export type ConstraintInfo = {
  readonly code: string
  readonly msg: string
  readonly level: ValidationLevel
  readonly params?: Readonly<Record<string, unknown>>
}

/** One field as plain data. `use()` / `useAsync()` validators are not listed; `async` says whether any are async. */
export type FieldInfo = {
  readonly kind: FieldKind
  /** The `defineDataType` name, for a field made by a data type factory. `kind` is its base's. */
  readonly type?: string
  /** False once `optional()` has been applied. */
  readonly required: boolean
  readonly nullable: boolean
  /** A default means the caller may omit the field and still get a value. */
  readonly hasDefault: boolean
  /** Present only for a literal default; a factory is not evaluated here. */
  readonly default?: unknown
  /** Set by `.hidden()`: left out of the model's public view. */
  readonly hidden: boolean
  readonly doc: string
  readonly async: boolean
  readonly constraints: readonly ConstraintInfo[]
  /** For a list: the item. For an object: nothing — see `fields`. */
  readonly item?: FieldInfo
  /** For an object field, its nested shape. */
  readonly fields?: Readonly<Record<string, FieldInfo>>
  /** Object fields only: whether unknown keys are rejected. */
  readonly strict?: boolean
  /** Union / discriminated: the variants, in declaration order. */
  readonly variants?: readonly FieldInfo[]
  /** Discriminated: the field that selects the variant. */
  readonly discriminant?: string
  /** Discriminated: tag for each entry in `variants`, in the same order. */
  readonly variantTags?: readonly string[]
}

/** A model or shape as plain data. `name` is `''` for a plain shape. */
export type ShapeInfo = {
  readonly name: string
  readonly fields: Readonly<Record<string, FieldInfo>>
  /** Declaration order, which is the order a generator should emit. */
  readonly order: readonly string[]
}

/** Kind by `instanceof`. Anything unrecognised, `AnyField` included, is `'any'`. */
const kindOf = (f: SomeField): FieldKind =>
  f instanceof StrField
    ? 'string'
    : f instanceof NumField
      ? 'number'
      : f instanceof BoolField
        ? 'boolean'
        : f instanceof ListField
          ? 'list'
          : f instanceof ObjField
            ? 'object'
            : f instanceof UnionField
              ? 'union'
              : f instanceof DiscriminatedField
                ? 'discriminated'
                : 'any'

/**
 * One field, as data.
 *
 * @example
 * introspectField(str().max(5).optional()) // { kind: 'string', required: false, constraints: [{ code: 'MAX', … }], … }
 */
export function introspectField(f: SomeField): FieldInfo {
  const o = f._opts
  const cons = (f as unknown as { _constraints?: Constraint<unknown>[] })._constraints ?? []
  const base = {
    kind: kindOf(f),
    ...(o.typeName === undefined ? {} : { type: o.typeName }),
    required: o.req,
    nullable: o.null,
    hasDefault: o.def !== undefined,
    hidden: o.hidden === true,
    doc: o.doc,
    async: f.hasAsync(),
    constraints: cons.map((c) => ({
      code: c.code,
      msg: c.msg,
      level: c.level,
      ...(c.params ? { params: c.params } : {}),
    })),
  }
  const withDefault =
    o.def !== undefined && typeof o.def !== 'function' ? { ...base, default: o.def } : base

  if (f instanceof ListField) {
    const constraints = [...withDefault.constraints]
    if (f._minItems !== undefined) {
      constraints.push({
        code: 'MIN_LENGTH',
        msg: `Min ${f._minItems} items`,
        level: 'error',
        params: { minItems: f._minItems },
      })
    }
    if (f._maxItems !== undefined) {
      constraints.push({
        code: 'MAX_LENGTH',
        msg: `Max ${f._maxItems} items`,
        level: 'error',
        params: { maxItems: f._maxItems },
      })
    }
    return { ...withDefault, constraints, item: introspectField(f.item) }
  }
  if (f instanceof ObjField) {
    return { ...withDefault, strict: f._strict, fields: introspectShape(f.shape).fields }
  }
  if (f instanceof UnionField) {
    return { ...withDefault, variants: f.variants.map((v: SomeField) => introspectField(v)) }
  }
  if (f instanceof DiscriminatedField) {
    const entries = Object.entries(f.variants)
    return {
      ...withDefault,
      discriminant: f.discKey,
      variantTags: entries.map(([tag]) => tag),
      variants: entries.map(([, field]) => introspectField(field)),
    }
  }
  return withDefault
}

/**
 * Every field of a shape or model, as data, in declaration order.
 *
 * @example
 * const info = introspect(Account)
 * info.name  // 'account'
 * info.order // ['id', 'email']
 */
export function introspect(x: ShapeInput): ShapeInfo {
  const { name, fields } = metaOf(x)
  return { ...introspectShape(fields), name }
}

/** The fields of a plain shape. {@link introspect} fills in the name. */
function introspectShape(shape: Shape): ShapeInfo {
  const out: Record<string, FieldInfo> = {}
  const order: string[] = []
  for (const [k, f] of Object.entries(shape)) {
    out[k] = introspectField(f)
    order.push(k)
  }
  return { name: '', fields: out, order }
}
