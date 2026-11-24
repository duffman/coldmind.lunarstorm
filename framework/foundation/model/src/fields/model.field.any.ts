/**
 * @module model.field.any
 * Unchecked passthrough field: `any()` factory and {@link AnyField}.
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
 * Accepts any value of declared type `T` without checking it. Add a check with `guard()` or `.use()`.
 *
 * @example
 * const payload = any<{ kind: string }>()
 * const when = guard(any<Date>(), (v) => v instanceof Date, 'Not a Date')
 *
 * ---
 * Accepts anything non-undefined. Escape hatch for opaque payloads.
 */
export class AnyField<
  T = unknown,
  O = T,
  S extends FieldState = InitialFieldState,
> extends Field<T, O, S> {
  clone(): this {
    const c = new AnyField<T, O, S>() as unknown as this
    this._copyBase(c)
    return c
  }
  protected _checkType(_v: unknown): boolean {
    return true
  }
  protected _parseSync(_path: Path, v: T): ParseOut<T> {
    return { value: v, errors: [], warnings: [] }
  }

  override optional(): AnyField<T, OptionalOutput<O, S>, OptionalState<S>> {
    return super.optional() as AnyField<T, OptionalOutput<O, S>, OptionalState<S>>
  }
  override nullable(): AnyField<T, O | null, NullableState<S>> {
    return super.nullable() as AnyField<T, O | null, NullableState<S>>
  }
  override nonNullable(): AnyField<T, Exclude<O, null>, NonNullableState<S>> {
    return super.nonNullable() as AnyField<T, Exclude<O, null>, NonNullableState<S>>
  }
  override required(): AnyField<T, Exclude<O, undefined>, RequiredState<S>> {
    return super.required() as AnyField<T, Exclude<O, undefined>, RequiredState<S>>
  }
  override default(
    v: T | (() => T),
  ): Defaulted<AnyField<T, Exclude<O, undefined>, DefaultedState<S>>> {
    return super.default(v) as unknown as Defaulted<
      AnyField<T, Exclude<O, undefined>, DefaultedState<S>>
    >
  }
  override withoutDefault(): AnyField<
    T,
    WithoutDefaultOutput<O, S>,
    WithoutDefaultState<S>
  > {
    return super.withoutDefault() as AnyField<
      T,
      WithoutDefaultOutput<O, S>,
      WithoutDefaultState<S>
    >
  }
  override hidden(): AnyField<T, O, HiddenState<S>> {
    return super.hidden() as AnyField<T, O, HiddenState<S>>
  }
}

/**
 * A required field of type `T` with no runtime type check.
 *
 * @example
 * any<Record<string, string>>()
 */
export const any = <T = unknown>() => new AnyField<T>()
