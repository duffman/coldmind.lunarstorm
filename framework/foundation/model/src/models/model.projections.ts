/**
 * @module model.projections
 * Handle-checked projections over a model: `pick`, `omit`, `partial`, `makeOptional`, `makeRequired`, `merge`.
 */
import type { Merge, Simplify } from '@coldmind/core'
import type { FieldStateOf } from '@coldmind/model/src/fields/model.field.pipeline.ts'
import { MODEL_OWNER } from '@coldmind/model/model.key'
import {
  type AnyModel,
  type AsShape,
  type FieldRef,
  type Model,
  type NameOf,
  type Optionalized,
  type PatchOptionalized,
  type Requiredized,
  type ShapeInput,
  type ShapeOf,
  buildModel,
  metaOf,
  toShape,
} from '@coldmind/model/define-model'
import type { Shape } from '@coldmind/model/src/models/model.shape.inference.ts'

// ── Projections ─────────────────────────────────────────────
//
// The key tuple is inferred from the handles, so the result narrows to exactly
// the fields that were named. Different model names/keys fail statically; exact
// model-instance ownership is also checked at runtime for structurally identical handles.

/** The handles a projection takes: one {@link FieldRef} per key in `Ks`, all from model `N`. */
export type RefTuple<
  N extends string,
  S extends Shape,
  Ks extends readonly (keyof S & string)[],
> = {
  readonly [I in keyof Ks]: FieldRef<N, Ks[I] & string, S[Ks[I] & string]>
}

/** The keys the handles name. Throws when one is not a field of the model (only possible through a cast). */
function refKeys(
  fn: string,
  name: string,
  fields: Shape,
  owner: object,
  refs: readonly unknown[],
): Set<string> {
  const keys = new Set<string>()
  for (const r of refs) {
    if (typeof r !== 'object' || r === null) {
      throw new Error(`${fn}(): expected a field handle from model "${name || '(anonymous)'}"`)
    }
    const ref = r as FieldRef
    const k = ref.key
    if (!Object.hasOwn(fields, k)) throw new Error(`${fn}(): model "${name || '(anonymous)'}" has no field "${k}"`)
    if (ref[MODEL_OWNER] !== owner || ref.field !== fields[k]) {
      throw new Error(`${fn}(): field "${k}" does not belong to model "${name || '(anonymous)'}"`)
    }
    keys.add(k)
  }
  return keys
}

/**
 * A model with only the named fields. Handles must come from `model`.
 *
 * @example
 * const Public = pick(Account, Account.id, Account.email)
 *
 * @throws {Error} when a handle names a field the model lacks (only possible through a cast)
 *
 * ---
 * A subset — `pick(Account, Account.id, Account.email)`.
 */
export function pick<M extends AnyModel, Ks extends readonly (keyof ShapeOf<M> & string)[]>(
  model: M,
  ...refs: RefTuple<NameOf<M>, ShapeOf<M>, Ks>
): Model<NameOf<M>, Simplify<Pick<ShapeOf<M>, Ks[number]>>> {
  const { name, fields, strict, owner } = metaOf(model)
  const out: Shape = {}
  for (const k of refKeys('pick', name, fields, owner, refs)) out[k] = fields[k] as Shape[string]
  return buildModel(name, out, strict) as Model<NameOf<M>, Simplify<Pick<ShapeOf<M>, Ks[number]>>>
}

/**
 * A model without the named fields.
 *
 * @example
 * const NewAccount = omit(Account, Account.id)
 *
 * ---
 * The complement of `pick` — `omit(Account, Account.id)`.
 */
export function omit<M extends AnyModel, Ks extends readonly (keyof ShapeOf<M> & string)[]>(
  model: M,
  ...refs: RefTuple<NameOf<M>, ShapeOf<M>, Ks>
): Model<NameOf<M>, Simplify<Omit<ShapeOf<M>, Ks[number]>>> {
  const { name, fields, strict, owner } = metaOf(model)
  const drop = refKeys('omit', name, fields, owner, refs)
  const out: Shape = {}
  for (const [k, f] of Object.entries(fields)) if (!drop.has(k)) out[k] = f
  return buildModel(name, out, strict) as Model<NameOf<M>, Simplify<Omit<ShapeOf<M>, Ks[number]>>>
}

/**
 * A model with every field optional. Accepts a model or a plain shape.
 *
 * @example
 * const AccountPatch = partial(Account)
 *
 * ---
 * Every field optional — the usual shape of a patch payload.
 */
export function partial<X extends ShapeInput>(
  shape: X,
): Model<NameOf<X>, Simplify<{ [K in keyof AsShape<X>]: PatchOptionalized<AsShape<X>[K]> }>> {
  const { name, fields, strict } = metaOf(shape)
  const out: Shape = {}
  for (const [k, f] of Object.entries(fields)) out[k] = f.optional().withoutDefault()
  return buildModel(name, out, strict) as Model<
    NameOf<X>,
    Simplify<{ [K in keyof AsShape<X>]: PatchOptionalized<AsShape<X>[K]> }>
  >
}

/**
 * A model with only the named fields made optional.
 *
 * Not `Field#optional()`, which acts on one field.
 *
 * @example
 * const Signup = makeOptional(Account, Account.id)
 *
 * ---
 * Only the named fields made optional — everything else keeps its rules.
 */
export function makeOptional<M extends AnyModel, Ks extends readonly (keyof ShapeOf<M> & string)[]>(
  model: M,
  ...refs: RefTuple<NameOf<M>, ShapeOf<M>, Ks>
): Model<
  NameOf<M>,
  Simplify<Omit<ShapeOf<M>, Ks[number]> & { [K in Ks[number]]: Optionalized<ShapeOf<M>[K]> }>
> {
  const { name, fields, strict, owner } = metaOf(model)
  const soften = refKeys('makeOptional', name, fields, owner, refs)
  const out: Shape = {}
  for (const [k, f] of Object.entries(fields)) out[k] = soften.has(k) ? f.optional() : f
  return buildModel(name, out, strict) as Model<
    NameOf<M>,
    Simplify<Omit<ShapeOf<M>, Ks[number]> & { [K in Ks[number]]: Optionalized<ShapeOf<M>[K]> }>
  >
}

/**
 * A model with only the named fields made required.
 *
 * Not `Field#required()`, which acts on one field.
 *
 * @example
 * const Patch = partial(Account)
 * const EmailPatch = makeRequired(Patch, Patch.email)
 *
 * ---
 * The named fields made required again — the inverse of `makeOptional`.
 */
export function makeRequired<M extends AnyModel, Ks extends readonly (keyof ShapeOf<M> & string)[]>(
  model: M,
  ...refs: RefTuple<NameOf<M>, ShapeOf<M>, Ks>
): Model<
  NameOf<M>,
  Simplify<Omit<ShapeOf<M>, Ks[number]> & { [K in Ks[number]]: Requiredized<ShapeOf<M>[K]> }>
> {
  const { name, fields, strict, owner } = metaOf(model)
  const harden = refKeys('makeRequired', name, fields, owner, refs)
  const out: Shape = {}
  for (const [k, f] of Object.entries(fields)) out[k] = harden.has(k) ? f.required() : f
  return buildModel(name, out, strict) as Model<
    NameOf<M>,
    Simplify<Omit<ShapeOf<M>, Ks[number]> & { [K in Ks[number]]: Requiredized<ShapeOf<M>[K]> }>
  >
}

/** The keys of `S` whose field is not marked `.hidden()`. */
export type Visible<S extends Shape> = {
  [K in keyof S as FieldStateOf<S[K]>['hidden'] extends true ? never : K]: S[K]
}

/**
 * The model's public view: every field except those marked `.hidden()`. Keeps name and strictness.
 *
 * @example
 * const User = model('user', { name: str(), password: str().hidden() })
 * const PublicUser = publicView(User) // { name }
 * toJsonSchema(PublicUser)            // the output contract
 */
export function publicView<X extends ShapeInput>(shape: X): Model<NameOf<X>, Simplify<Visible<AsShape<X>>>> {
  const { name, fields, strict } = metaOf(shape)
  const out: Shape = {}
  for (const [k, f] of Object.entries(fields)) if (!f._opts.hidden) out[k] = f
  return buildModel(name, out, strict) as Model<NameOf<X>, Simplify<Visible<AsShape<X>>>>
}

/**
 * A model with the fields of both; `b` wins a collision. Keeps `a`'s name and strictness.
 *
 * @example
 * const WithAudit = merge(Account, { createdAt: num() })
 *
 * ---
 * Two shapes side by side, the right one winning a collision.
 */
export function merge<A extends ShapeInput, B extends ShapeInput>(
  a: A,
  b: B,
): Model<NameOf<A>, Merge<AsShape<A>, AsShape<B>>> {
  const { name, fields, strict } = metaOf(a)
  return buildModel(name, { ...fields, ...toShape(b) }, strict) as Model<
    NameOf<A>,
    Merge<AsShape<A>, AsShape<B>>
  >
}
