/**
 * @module model.field.union
 * Union and discriminated union fields: `union()`, `discriminated()` factories, {@link UnionField}, {@link DiscriminatedField}.
 */

import type { ValidationError, Path } from '@coldmind/core'
import {
  type Defaulted,
  type DefaultedState,
  Field,
  type FieldState,
  type FieldStateOf,
  type HiddenState,
  type InitialFieldState,
  type NonNullableState,
  type NullableState,
  type OptionalOutput,
  type OptionalState,
  type ParseOut,
  type RequiredState,
  type WithoutDefaultOutput,
  type WithoutDefaultState,
} from '@coldmind/model/src/fields/model.field.pipeline.ts'
import type { InferField, InferShape, Shape, SomeField } from '@coldmind/model/src/models/model.shape.inference.ts'
import { obj, type ObjField } from '@coldmind/model/src/fields/model.field.object.ts'
import { str } from '@coldmind/model/src/fields/model.field.string.ts'

/**
 * A value that must match at least one of `variants`. First match wins.
 * Presence comes from the variants: an optional variant makes the union
 * optional, a nullable one makes it nullable, and the first variant default
 * is the union's default.
 *
 * @example
 * union(str(), int())            // string | number
 * union(str().optional(), int()) // string | number | undefined
 */
type AnyTrue<B extends boolean> = true extends B ? true : false
type AllTrue<B extends boolean> = false extends B ? false : true
type UnionDefaulted<Fs extends readonly SomeField[]> = AnyTrue<
  FieldStateOf<Fs[number]>['defaulted']
>
type UnionRequired<Fs extends readonly SomeField[]> = UnionDefaulted<Fs> extends true
  ? false
  : AllTrue<FieldStateOf<Fs[number]>['required']>

/** Presence state derived from the variants exactly as the runtime constructor does. */
export type UnionState<Fs extends readonly SomeField[]> = {
  readonly required: UnionRequired<Fs>
  readonly nullable: AnyTrue<FieldStateOf<Fs[number]>['nullable']>
  readonly defaulted: UnionDefaulted<Fs>
  readonly hidden: false
}

export class UnionField<
  Fs extends readonly SomeField[] = readonly SomeField[],
  O = InferField<Fs[number]>,
  ST extends FieldState = UnionState<Fs>,
> extends Field<InferField<Fs[number]>, O, ST> {
  constructor(readonly variants: Fs) {
    super()
    const def = variants.find((v) => v._opts.def !== undefined)?._opts.def
    this._opts.def = def as InferField<Fs[number]> | undefined
    this._opts.req = def === undefined && variants.every((v) => v._opts.req)
    this._opts.null = variants.some((v) => v._opts.null)
  }

  /** Variants are shared, not cloned: fields are immutable. */
  clone(): this {
    const c = new UnionField<Fs, O, ST>(this.variants) as unknown as this
    this._copyBase(c)
    return c
  }

  override hasAsync(): boolean {
    if (super.hasAsync()) return true
    return this.variants.some((v) => v.hasAsync())
  }

  protected _checkType(_v: unknown): boolean {
    return true
  }

  override optional(): UnionField<Fs, OptionalOutput<O, ST>, OptionalState<ST>> {
    return super.optional() as UnionField<Fs, OptionalOutput<O, ST>, OptionalState<ST>>
  }
  override nullable(): UnionField<Fs, O | null, NullableState<ST>> {
    return super.nullable() as UnionField<Fs, O | null, NullableState<ST>>
  }
  override nonNullable(): UnionField<Fs, Exclude<O, null>, NonNullableState<ST>> {
    return super.nonNullable() as UnionField<Fs, Exclude<O, null>, NonNullableState<ST>>
  }
  override required(): UnionField<Fs, Exclude<O, undefined>, RequiredState<ST>> {
    return super.required() as UnionField<Fs, Exclude<O, undefined>, RequiredState<ST>>
  }
  override default(
    v: InferField<Fs[number]> | (() => InferField<Fs[number]>),
  ): Defaulted<UnionField<Fs, Exclude<O, undefined>, DefaultedState<ST>>> {
    return super.default(v) as unknown as Defaulted<
      UnionField<Fs, Exclude<O, undefined>, DefaultedState<ST>>
    >
  }
  override withoutDefault(): UnionField<
    Fs,
    WithoutDefaultOutput<O, ST>,
    WithoutDefaultState<ST>
  > {
    return super.withoutDefault() as UnionField<
      Fs,
      WithoutDefaultOutput<O, ST>,
      WithoutDefaultState<ST>
    >
  }
  override hidden(): UnionField<Fs, O, HiddenState<ST>> {
    return super.hidden() as UnionField<Fs, O, HiddenState<ST>>
  }

  protected _parseSync(path: Path, v: InferField<Fs[number]>): ParseOut<InferField<Fs[number]>> {
    const collected: ValidationError[] = []
    for (const variant of this.variants) {
      const r = variant._validateSyncUnchecked(path, v)
      if (r.isValid) {
        return {
          value: r.value as InferField<Fs[number]>,
          errors: [],
          warnings: r.warnings,
        }
      }
      collected.push(...r.errors)
    }
    return {
      value: v,
      errors: [
        { path, msg: 'No union variant matched', code: 'UNION', level: 'error' },
        ...collected,
      ],
      warnings: [],
    }
  }

  protected override async _parseAsync(
    path: Path,
    v: InferField<Fs[number]>,
  ): Promise<ParseOut<InferField<Fs[number]>>> {
    const collected: ValidationError[] = []
    for (const variant of this.variants) {
      const r = await variant.validate(path, v)
      if (r.isValid) {
        return {
          value: r.value as InferField<Fs[number]>,
          errors: [],
          warnings: r.warnings,
        }
      }
      collected.push(...r.errors)
    }
    return {
      value: v,
      errors: [
        { path, msg: 'No union variant matched', code: 'UNION', level: 'error' },
        ...collected,
      ],
      warnings: [],
    }
  }
}

/** Value type of {@link discriminated}: each variant tagged with the discriminant. */
export type DiscValue<K extends string, V extends Record<string, Shape>> = {
  [P in keyof V & string]: InferShape<V[P]> & { readonly [D in K]: P }
}[keyof V & string]

/**
 * An object whose `key` field selects which variant shape applies.
 *
 * @example
 * const Animal = discriminated('kind', {
 *   cat: { lives: int() },
 *   dog: { bark: str() },
 * })
 */
export class DiscriminatedField<
  K extends string = string,
  V extends Record<string, Shape> = Record<string, Shape>,
  O = DiscValue<K, V>,
  ST extends FieldState = InitialFieldState,
> extends Field<DiscValue<K, V>, O, ST> {
  constructor(
    readonly discKey: K,
    readonly variants: { readonly [P in keyof V]: ObjField<Shape> },
  ) {
    super()
  }

  /** Variants are shared, not cloned: fields are immutable. */
  clone(): this {
    const c = new DiscriminatedField<K, V, O, ST>(this.discKey, this.variants) as unknown as this
    this._copyBase(c)
    return c
  }

  override hasAsync(): boolean {
    if (super.hasAsync()) return true
    return Object.values(this.variants).some((v) => v.hasAsync())
  }

  protected _checkType(v: unknown): boolean {
    return typeof v === 'object' && v !== null && !Array.isArray(v)
  }

  override optional(): DiscriminatedField<K, V, OptionalOutput<O, ST>, OptionalState<ST>> {
    return super.optional() as DiscriminatedField<K, V, OptionalOutput<O, ST>, OptionalState<ST>>
  }
  override nullable(): DiscriminatedField<K, V, O | null, NullableState<ST>> {
    return super.nullable() as DiscriminatedField<K, V, O | null, NullableState<ST>>
  }
  override nonNullable(): DiscriminatedField<K, V, Exclude<O, null>, NonNullableState<ST>> {
    return super.nonNullable() as DiscriminatedField<K, V, Exclude<O, null>, NonNullableState<ST>>
  }
  override required(): DiscriminatedField<K, V, Exclude<O, undefined>, RequiredState<ST>> {
    return super.required() as DiscriminatedField<K, V, Exclude<O, undefined>, RequiredState<ST>>
  }
  override default(
    v: DiscValue<K, V> | (() => DiscValue<K, V>),
  ): Defaulted<DiscriminatedField<K, V, Exclude<O, undefined>, DefaultedState<ST>>> {
    return super.default(v) as unknown as Defaulted<
      DiscriminatedField<K, V, Exclude<O, undefined>, DefaultedState<ST>>
    >
  }
  override withoutDefault(): DiscriminatedField<
    K,
    V,
    WithoutDefaultOutput<O, ST>,
    WithoutDefaultState<ST>
  > {
    return super.withoutDefault() as DiscriminatedField<
      K,
      V,
      WithoutDefaultOutput<O, ST>,
      WithoutDefaultState<ST>
    >
  }
  override hidden(): DiscriminatedField<K, V, O, HiddenState<ST>> {
    return super.hidden() as DiscriminatedField<K, V, O, HiddenState<ST>>
  }

  protected _parseSync(path: Path, v: DiscValue<K, V>): ParseOut<DiscValue<K, V>> {
    const variant = this.#variant(path, v)
    if (!('shape' in variant)) return variant
    const r = variant._validateSyncUnchecked(path, v)
    return { value: (r.value as DiscValue<K, V>) ?? v, errors: r.errors, warnings: r.warnings }
  }

  protected override async _parseAsync(
    path: Path,
    v: DiscValue<K, V>,
  ): Promise<ParseOut<DiscValue<K, V>>> {
    const variant = this.#variant(path, v)
    if (!('shape' in variant)) return variant
    const r = await variant.validate(path, v)
    return { value: (r.value as DiscValue<K, V>) ?? v, errors: r.errors, warnings: r.warnings }
  }

  #miss(path: Path, v: DiscValue<K, V>, tag: unknown): ParseOut<DiscValue<K, V>> {
    return {
      value: v,
      errors: [
        {
          path: [...path, this.discKey],
          msg: `Unknown ${this.discKey}: ${String(tag)}`,
          code: 'DISCRIMINANT',
          level: 'error',
        },
      ],
      warnings: [],
    }
  }

  #variant(path: Path, v: DiscValue<K, V>): ObjField<Shape> | ParseOut<DiscValue<K, V>> {
    const src = v as Record<string, unknown>
    const tag = src[this.discKey]
    if (typeof tag !== 'string' || !Object.hasOwn(this.variants, tag)) return this.#miss(path, v, tag)
    return this.variants[tag as keyof V] ?? this.#miss(path, v, tag)
  }
}

/**
 * A value matching at least one of the variants. First match wins.
 *
 * @example
 * union(str().email(), int().positive())
 */
export const union = <const Fs extends readonly SomeField[]>(...variants: Fs) =>
  new UnionField(variants)

/**
 * An object whose `key` field selects a variant. The discriminant is injected
 * when a variant does not already declare it.
 *
 * @example
 * discriminated('kind', { cat: { lives: int() }, dog: { bark: str() } })
 */
export function discriminated<K extends string, V extends Record<string, Shape>>(
  key: K,
  variants: V,
): DiscriminatedField<K, V> {
  const built = {} as { [P in keyof V]: ObjField<Shape> }
  for (const [tag, shape] of Object.entries(variants) as [
    keyof V & string,
    V[keyof V & string],
  ][]) {
    const fields: Shape = Object.hasOwn(shape, key)
      ? { ...shape }
      : { [key]: str().oneOf([tag]), ...shape }
    built[tag] = obj(fields)
  }
  return new DiscriminatedField(key, built)
}
