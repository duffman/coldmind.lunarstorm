/**
 * @module model.field.boolean
 * Boolean field: `bool()` factory and {@link BoolField}.
 */

import type { Path } from '@coldmind/core'
import {
  Field,
  type Defaulted,
  type DefaultedState,
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

/**
 * A boolean field. Strict: `'true'` and `1` are type errors unless you add `.coerce()`.
 *
 * @example
 * const done = bool().default(false)
 */
export class BoolField<
  O = boolean,
  S extends FieldState = InitialFieldState,
> extends Field<boolean, O, S> {
  clone(): this {
    const c = new BoolField<O, S>() as unknown as this
    this._copyBase(c)
    return c
  }
  protected _checkType(v: unknown): boolean {
    return typeof v === 'boolean'
  }
  protected _parseSync(_path: Path, v: boolean): ParseOut<boolean> {
    return { value: v, errors: [], warnings: [] }
  }

  override optional(): BoolField<OptionalOutput<O, S>, OptionalState<S>> {
    return super.optional() as BoolField<OptionalOutput<O, S>, OptionalState<S>>
  }
  override nullable(): BoolField<O | null, NullableState<S>> {
    return super.nullable() as BoolField<O | null, NullableState<S>>
  }
  override nonNullable(): BoolField<Exclude<O, null>, NonNullableState<S>> {
    return super.nonNullable() as BoolField<Exclude<O, null>, NonNullableState<S>>
  }
  override required(): BoolField<Exclude<O, undefined>, RequiredState<S>> {
    return super.required() as BoolField<Exclude<O, undefined>, RequiredState<S>>
  }
  override default(
    v: boolean | (() => boolean),
  ): Defaulted<BoolField<Exclude<O, undefined>, DefaultedState<S>>> {
    return super.default(v) as unknown as Defaulted<
      BoolField<Exclude<O, undefined>, DefaultedState<S>>
    >
  }
  override withoutDefault(): BoolField<WithoutDefaultOutput<O, S>, WithoutDefaultState<S>> {
    return super.withoutDefault() as BoolField<WithoutDefaultOutput<O, S>, WithoutDefaultState<S>>
  }
  override hidden(): BoolField<O, HiddenState<S>> {
    return super.hidden() as BoolField<O, HiddenState<S>>
  }
}

/**
 * A required boolean field.
 *
 * @example
 * done: bool().default(false)
 */
export const bool = () => new BoolField()
