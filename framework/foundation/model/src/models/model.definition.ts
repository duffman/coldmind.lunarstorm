/**
 * @module model.definition
 * `model`: a shape with one handle per field, named or not.
 */
import { MODEL, MODEL_OWNER } from '@coldmind/model/model.key'
import type { AsyncValidator, SyncValidator } from '@coldmind/model/src/fields/model.field.pipeline.ts'
import type { Shape, SomeField } from '@coldmind/model/src/models/model.shape.inference.ts'

// ─────────────────────────────────────────────────────────────
// MODELS
//
// A shape you intend to *reference* needs handles, not names: `Account.email`
// instead of `'email'`. A model is a shape with one handle per field, so a
// projection is checked against the model it came from. Names/keys are checked
// statically and exact owner identity is checked at runtime.
//
// The metadata hangs off a symbol, so no field name can collide with it and
// every string key on a model is a field.
// ─────────────────────────────────────────────────────────────

/** A model's name, its underlying shape, and whether unknown keys are errors. */
export type ModelMeta<N extends string, S extends Shape> = {
  readonly name: N
  readonly fields: S
  /** @internal Stable runtime identity for this exact model instance. */
  readonly owner: object
  /**
   * Unknown keys are errors rather than dropped warnings — for a model at a
   * trust boundary, where an unknown key means sender and contract disagree.
   */
  readonly strict?: boolean
  /** @internal Root validators attached with `refine`. */
  readonly refinements?: readonly SyncValidator<any>[]
  /** @internal Root async validators attached with `refineAsync`. */
  readonly asyncRefinements?: readonly AsyncValidator<any>[]
}

/** A handle on one field. Carries static name/key/type plus exact runtime model ownership. */
export type FieldRef<
  N extends string = string,
  K extends string = string,
  F extends SomeField = SomeField,
> = {
  readonly model: N
  readonly key: K
  readonly field: F
  /** @internal Exact model ownership; prevents forged/foreign handles at runtime. */
  readonly [MODEL_OWNER]: object
}

/**
 * What {@link model} returns: one {@link FieldRef} per field, plus hidden name and shape.
 *
 * @example
 * const Account = model('account', { id: int(), email: str().email() })
 * Account.email       // FieldRef<'account', 'email', StrField>
 * type A = Infer<typeof Account> // { id: number; email: string }
 */
export type Model<N extends string, S extends Shape> = {
  readonly [K in keyof S & string]: FieldRef<N, K, S[K]>
} & { readonly [MODEL]: ModelMeta<N, S> }

/** Any model, whatever its name and shape. Use in constraints. */
// biome-ignore lint/suspicious/noExplicitAny: matches a model of any name and shape
export type AnyModel = { readonly [MODEL]: ModelMeta<any, any> }

/** The shape behind a model type. */
// biome-ignore lint/suspicious/noExplicitAny: matches any name; only the shape is inferred
export type ShapeOf<M> = M extends { readonly [MODEL]: ModelMeta<any, infer S extends Shape> }
  ? S
  : never

/** A model's name type; `''` for an unnamed one. */
// biome-ignore lint/suspicious/noExplicitAny: matches any shape; only the name is inferred
export type NameOf<M> = M extends { readonly [MODEL]: ModelMeta<infer N, any> } ? N : ''

/** Anywhere a shape is wanted, a model does too. */
export type ShapeInput = Shape | AnyModel

/** The shape behind a {@link ShapeInput} type: a model's shape, or the shape itself. */
// biome-ignore lint/suspicious/noExplicitAny: matches any name; only the shape is inferred
export type AsShape<X> = X extends { readonly [MODEL]: ModelMeta<any, infer S extends Shape> }
  ? S
  : X extends Shape
    ? X
    : never

/** A field as `optional()` leaves it — same subclass, output widened by `undefined`. */
export type Optionalized<F extends SomeField> = ReturnType<F['optional']>

/** A field as `partial()` leaves it: optional and with defaults suppressed. */
export type PatchOptionalized<F extends SomeField> = ReturnType<ReturnType<F['optional']>['withoutDefault']>

/** A field as `required()` leaves it — `undefined` removed from the output. */
export type Requiredized<F extends SomeField> = ReturnType<F['required']>

/** True for a value returned by {@link model} or a projection. */
export const isModel = (x: unknown): x is AnyModel =>
  typeof x === 'object' && x !== null && Object.hasOwn(x, MODEL)

/** {@link ModelMeta} with its type parameters erased, for runtime code. */
export type RuntimeModelMeta = {
  readonly name: string
  readonly fields: Shape
  readonly owner: object
  readonly strict?: boolean
  readonly refinements?: readonly SyncValidator<any>[]
  readonly asyncRefinements?: readonly AsyncValidator<any>[]
}

/** @internal crosses a file boundary; not part of the public surface. */
export const metaOf = (x: ShapeInput): RuntimeModelMeta =>
  isModel(x)
    ? (x[MODEL] as RuntimeModelMeta)
    : { name: '', fields: x, owner: x }

/**
 * The plain shape behind a model, or the shape itself.
 *
 * @example
 * Object.keys(toShape(Account)) // ['id', 'email']
 */
export const toShape = (x: ShapeInput): Shape => metaOf(x).fields

/**
 * A model's shape, typed.
 *
 * @example
 * fieldsOf(Account).email // StrField
 */
export const fieldsOf = <M extends AnyModel>(m: M): ShapeOf<M> => m[MODEL].fields as ShapeOf<M>

/**
 * A model's name, typed; `''` for an unnamed model.
 *
 * @example
 * nameOf(Account) // 'account'
 */
export const nameOf = <M extends AnyModel>(m: M): NameOf<M> => m[MODEL].name as NameOf<M>

/** @internal builds the handle object; shared with projections and packages that derive models. */
export type BuildModelExtras = {
  readonly refinements?: readonly SyncValidator<any>[]
  readonly asyncRefinements?: readonly AsyncValidator<any>[]
}

export function buildModel(
  name: string,
  fields: Shape,
  strict?: boolean,
  extras: BuildModelExtras = {},
): AnyModel {
  const snapshot = Object.freeze({ ...fields }) as Shape
  const owner = Object.freeze({})
  const m = Object.create(null) as Record<string, FieldRef>
  for (const [key, field] of Object.entries(snapshot)) {
    m[key] = Object.freeze({ model: name, key, field, [MODEL_OWNER]: owner }) as FieldRef
  }
  const meta: RuntimeModelMeta = Object.freeze({
    name,
    fields: snapshot,
    owner,
    ...(strict ? { strict: true } : {}),
    ...(extras.refinements?.length ? { refinements: Object.freeze([...extras.refinements]) } : {}),
    ...(extras.asyncRefinements?.length
      ? { asyncRefinements: Object.freeze([...extras.asyncRefinements]) }
      : {}),
  })
  Object.defineProperty(m, MODEL, { value: meta, enumerable: false })
  return Object.freeze(m) as unknown as AnyModel
}

/** Options accepted by {@link model}. */
export type ModelOptions = { readonly strict?: boolean }

/**
 * A shape with a handle for every field. Name it, or leave it unnamed for a shape written inline.
 *
 * @example
 * const Account = model('account', { id: int(), email: str().email() })
 * const Point = model({ x: num(), y: num() })
 * pick(Account, Account.id)   // handles, no strings
 * parse(Account, body)        // { id: number; email: string } or ValidationFailed
 *
 * @returns a {@link Model}; `check` / `parse` / `record` accept it directly
 */
export function model<N extends string, S extends Shape>(
  name: N,
  fields: S,
  options?: ModelOptions,
): Model<N, S>
export function model<S extends Shape>(fields: S, options?: ModelOptions): Model<'', S>
export function model(
  a: string | Shape,
  b?: Shape | ModelOptions,
  c?: ModelOptions,
): AnyModel {
  if (typeof a === 'string') return buildModel(a, (b as Shape | undefined) ?? {}, c?.strict)
  return buildModel('', a, (b as ModelOptions | undefined)?.strict)
}

/**
 * Alias of {@link model}. The RPC layer's original name; kept so a contract
 * written as a "shape" is still a model.
 */
export const defineShape = model
