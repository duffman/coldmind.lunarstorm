/**
 * @module model.field.object
 * Nested object field: `obj()` factory and {@link ObjField}.
 */

import type { ValidationError, Path } from '@coldmind/core'
import type { AnyModel, ShapeOf } from '@coldmind/model/define-model'
import { isModel, metaOf } from '@coldmind/model/define-model'
import {
  type Defaulted,
  type DefaultedState,
  Field,
  type FieldState,
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
import type { InferShape, Shape, SomeField } from '@coldmind/model/src/models/model.shape.inference.ts'

/**
 * A nested object validated by `shape`. Unknown keys are warnings (dropped) unless `.strict()`.
 *
 * @example
 * const address = obj({ street: str(), zip: str().length(5) }).strict()
 *
 * @typeParam S - the nested shape
 */
export class ObjField<
  S extends Shape,
  O = InferShape<S>,
  ST extends FieldState = InitialFieldState,
> extends Field<InferShape<S>, O, ST> {
  /** @internal unknown keys are errors rather than warnings */
  _strict = false

  readonly shape: S

  constructor(shape: S) {
    super()
    this.shape = Object.freeze({ ...shape }) as S
  }

  /** Children are shared, not cloned: fields are immutable, every modifier returns a copy. */
  clone(): this {
    const c = new ObjField<S, O, ST>(this.shape) as unknown as this
    this._copyBase(c)
    ;(c as unknown as ObjField<S, O, ST>)._strict = this._strict
    return c
  }

  override hasAsync(): boolean {
    if (super.hasAsync()) return true
    for (const field of Object.values(this.shape)) if (field.hasAsync()) return true
    return false
  }

  protected _checkType(v: unknown): boolean {
    return typeof v === 'object' && v !== null && !Array.isArray(v)
  }

  override optional(): ObjField<S, OptionalOutput<O, ST>, OptionalState<ST>> {
    return super.optional() as ObjField<S, OptionalOutput<O, ST>, OptionalState<ST>>
  }
  override nullable(): ObjField<S, O | null, NullableState<ST>> {
    return super.nullable() as ObjField<S, O | null, NullableState<ST>>
  }
  override nonNullable(): ObjField<S, Exclude<O, null>, NonNullableState<ST>> {
    return super.nonNullable() as ObjField<S, Exclude<O, null>, NonNullableState<ST>>
  }
  override required(): ObjField<S, Exclude<O, undefined>, RequiredState<ST>> {
    return super.required() as ObjField<S, Exclude<O, undefined>, RequiredState<ST>>
  }
  override default(
    v: InferShape<S> | (() => InferShape<S>),
  ): Defaulted<ObjField<S, Exclude<O, undefined>, DefaultedState<ST>>> {
    return super.default(v) as unknown as Defaulted<
      ObjField<S, Exclude<O, undefined>, DefaultedState<ST>>
    >
  }
  override withoutDefault(): ObjField<
    S,
    WithoutDefaultOutput<O, ST>,
    WithoutDefaultState<ST>
  > {
    return super.withoutDefault() as ObjField<
      S,
      WithoutDefaultOutput<O, ST>,
      WithoutDefaultState<ST>
    >
  }
  override hidden(): ObjField<S, O, HiddenState<ST>> {
    return super.hidden() as ObjField<S, O, HiddenState<ST>>
  }

  /** Report unknown keys as errors instead of warnings. */
  strict(): this {
    return this._mod((c) => {
      ;(c as unknown as ObjField<S, O, ST>)._strict = true
    })
  }

  protected _parseSync(path: Path, v: InferShape<S>): ParseOut<InferShape<S>> {
    const src = v as Record<string, unknown>
    const value: Record<string, unknown> = {}
    const errors: ValidationError[] = []
    const warnings: ValidationError[] = []

    for (const [k, field] of Object.entries(this.shape)) {
      const r = field._validateSyncUnchecked([...path, k], Object.hasOwn(src, k) ? src[k] : undefined)
      if (r.value !== undefined) value[k] = r.value
      errors.push(...r.errors)
      warnings.push(...r.warnings)
    }

    this._checkUnknownKeys(path, src, errors, warnings)
    return { value: value as InferShape<S>, errors, warnings }
  }

  protected override async _parseAsync(
    path: Path,
    v: InferShape<S>,
  ): Promise<ParseOut<InferShape<S>>> {
    const src = v as Record<string, unknown>
    const entries = await Promise.all(
      Object.entries(this.shape).map(async ([k, field]) => ({
        k,
        r: await field.validate([...path, k], Object.hasOwn(src, k) ? src[k] : undefined),
      })),
    )

    const value: Record<string, unknown> = {}
    const errors: ValidationError[] = []
    const warnings: ValidationError[] = []

    for (const { k, r } of entries) {
      if (r.value !== undefined) value[k] = r.value
      errors.push(...r.errors)
      warnings.push(...r.warnings)
    }

    this._checkUnknownKeys(path, src, errors, warnings)
    return { value: value as InferShape<S>, errors, warnings }
  }

  private _checkUnknownKeys(
    path: Path,
    src: Record<string, unknown>,
    errors: ValidationError[],
    warnings: ValidationError[],
  ): void {
    for (const k of Object.keys(src)) {
      if (!Object.hasOwn(this.shape, k)) {
        const e: ValidationError = {
          path: [...path, k],
          msg: 'Unknown key',
          code: 'UNKNOWN_KEY',
          level: this._strict ? 'error' : 'warning',
        }
        ;(this._strict ? errors : warnings).push(e)
      }
    }
  }
}

/**
 * A required nested object of `shape`, or of a model's fields.
 *
 * @example
 * obj({ lat: num(), lng: num() })
 * obj(Account)
 */
export function obj<S extends Shape>(shape: S): ObjField<S>
export function obj<M extends AnyModel>(model: M): ObjField<ShapeOf<M>>
export function obj(x: Shape | AnyModel): ObjField<Shape> {
  if (!isModel(x)) return new ObjField(x)
  // A model's strictness and refinements belong to the model, so a nested use keeps them.
  const meta = metaOf(x)
  let field = new ObjField<Shape>(meta.fields)
  if (meta.strict) field = field.strict()
  for (const fn of meta.refinements ?? []) field = field.use(fn) as ObjField<Shape>
  for (const fn of meta.asyncRefinements ?? []) field = field.useAsync(fn) as ObjField<Shape>
  return field
}
