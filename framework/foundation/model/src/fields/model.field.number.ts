/**
 * @module model.field.number
 * Number field: `num()`, `int()` factories and {@link NumField}.
 */

import type { Path } from '@coldmind/core'
import {
  ConstrainedField,
  type Defaulted,
  type DefaultedState,
  type FieldState,
  type HiddenState,
  type InitialFieldState,
  type NonNullableState,
  type NullableState,
  type OptionalOutput,
  type OptionalState,
  type RequiredState,
  type WithoutDefaultOutput,
  type WithoutDefaultState,
} from '@coldmind/model/src/fields/model.field.pipeline.ts'

/**
 * A number field. Rejects `NaN`. Use {@link int} for integers — it is `num().integer()`.
 *
 * @example
 * const priority = int().min(1).max(5)
 * const ratio = num().finite().positive()
 *
 * @typeParam O - output type; widened by `optional()`/`nullable()`
 */
export class NumField<
  O = number,
  S extends FieldState = InitialFieldState,
> extends ConstrainedField<number, O, S> {
  clone(): this {
    const c = new NumField<O, S>() as unknown as this
    this._copyBase(c)
    this._copyConstraints(c as unknown as ConstrainedField<number, O, S>)
    return c
  }

  protected _checkType(v: unknown): boolean {
    return typeof v === 'number' && !Number.isNaN(v)
  }

  override optional(): NumField<OptionalOutput<O, S>, OptionalState<S>> {
    return super.optional() as NumField<OptionalOutput<O, S>, OptionalState<S>>
  }
  override nullable(): NumField<O | null, NullableState<S>> {
    return super.nullable() as NumField<O | null, NullableState<S>>
  }
  override nonNullable(): NumField<Exclude<O, null>, NonNullableState<S>> {
    return super.nonNullable() as NumField<Exclude<O, null>, NonNullableState<S>>
  }
  override required(): NumField<Exclude<O, undefined>, RequiredState<S>> {
    return super.required() as NumField<Exclude<O, undefined>, RequiredState<S>>
  }
  override default(
    v: number | (() => number),
  ): Defaulted<NumField<Exclude<O, undefined>, DefaultedState<S>>> {
    return super.default(v) as unknown as Defaulted<
      NumField<Exclude<O, undefined>, DefaultedState<S>>
    >
  }
  override withoutDefault(): NumField<WithoutDefaultOutput<O, S>, WithoutDefaultState<S>> {
    return super.withoutDefault() as NumField<WithoutDefaultOutput<O, S>, WithoutDefaultState<S>>
  }
  override hidden(): NumField<O, HiddenState<S>> {
    return super.hidden() as NumField<O, HiddenState<S>>
  }

  /** Accept numeric strings. Explicit opt-in. A blank string stays a string and fails the type check. */
  coerceNumber(): this {
    return this.coerce((v) => (typeof v === 'string' && v.trim() !== '' ? Number(v) : v))
  }

  /** `>= n`. Error code `MIN`. */
  min(n: number): this {
    return this._addC((v) => v >= n, `Min ${n}`, 'MIN', 'error', { minimum: n })
  }
  /** `<= n`. Error code `MAX`. */
  max(n: number): this {
    return this._addC((v) => v <= n, `Max ${n}`, 'MAX', 'error', { maximum: n })
  }
  /** `> 0`. Error code `POSITIVE`. */
  positive(): this {
    return this._addC((v) => v > 0, 'Must be positive', 'POSITIVE', 'error', {
      exclusiveMinimum: 0,
    })
  }
  /** `< 0`. Error code `NEGATIVE`. */
  negative(): this {
    return this._addC((v) => v < 0, 'Must be negative', 'NEGATIVE', 'error', {
      exclusiveMaximum: 0,
    })
  }
  /** `!== 0`. Error code `NONZERO`. */
  nonzero(): this {
    return this._addC((v) => v !== 0, 'Cannot be zero', 'NONZERO')
  }
  /** Whole number. Error code `INTEGER`; `introspect` and JSON Schema report it as an integer. */
  integer(): this {
    return this._addC((v) => Number.isInteger(v), 'Must be an integer', 'INTEGER', 'error', {
      type: 'integer',
    })
  }
  /** Not `±Infinity`. Error code `FINITE`. */
  finite(): this {
    return this._addC((v) => Number.isFinite(v), 'Must be finite', 'FINITE')
  }
  /** Integer in 1–65535. Error code `PORT`. */
  port(): this {
    return this._addC((v) => Number.isInteger(v) && v >= 1 && v <= 65535, 'Invalid port', 'PORT')
  }
}

/**
 * A required number field; rejects `NaN`.
 *
 * @example
 * num().min(0).max(1)
 */
export const num = () => new NumField()

/**
 * A required integer field: `num().integer()`.
 *
 * @example
 * priority: int().min(1).max(5)
 */
export const int = () => new NumField().integer()
