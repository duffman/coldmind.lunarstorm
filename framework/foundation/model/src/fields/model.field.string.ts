/**
 * @module model.field.string
 * String field: `str()` factory and {@link StrField}.
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
 * A string field. Constraints (`min`, `email`, `oneOf`, …) run after transforms (`trim`, `lowercase`).
 *
 * @example
 * const slug = str().trim().lowercase().min(1).max(64).pattern(/^[a-z0-9-]+$/)
 * const role = str().oneOf(['admin', 'user'] as const) // StrField<'admin' | 'user'>
 *
 * @typeParam O - output type; widened by `optional()`/`nullable()`, narrowed by `oneOf()`
 */
export class StrField<
  O = string,
  S extends FieldState = InitialFieldState,
> extends ConstrainedField<string, O, S> {
  clone(): this {
    const c = new StrField<O, S>() as unknown as this
    this._copyBase(c)
    this._copyConstraints(c as unknown as ConstrainedField<string, O, S>)
    return c
  }

  protected _checkType(v: unknown): boolean {
    return typeof v === 'string'
  }

  override optional(): StrField<OptionalOutput<O, S>, OptionalState<S>> {
    return super.optional() as StrField<OptionalOutput<O, S>, OptionalState<S>>
  }
  override nullable(): StrField<O | null, NullableState<S>> {
    return super.nullable() as StrField<O | null, NullableState<S>>
  }
  override nonNullable(): StrField<Exclude<O, null>, NonNullableState<S>> {
    return super.nonNullable() as StrField<Exclude<O, null>, NonNullableState<S>>
  }
  override required(): StrField<Exclude<O, undefined>, RequiredState<S>> {
    return super.required() as StrField<Exclude<O, undefined>, RequiredState<S>>
  }
  override default(
    v: string | (() => string),
  ): Defaulted<StrField<Exclude<O, undefined>, DefaultedState<S>>> {
    return super.default(v) as unknown as Defaulted<
      StrField<Exclude<O, undefined>, DefaultedState<S>>
    >
  }
  override withoutDefault(): StrField<WithoutDefaultOutput<O, S>, WithoutDefaultState<S>> {
    return super.withoutDefault() as StrField<WithoutDefaultOutput<O, S>, WithoutDefaultState<S>>
  }
  override hidden(): StrField<O, HiddenState<S>> {
    return super.hidden() as StrField<O, HiddenState<S>>
  }

  /** Accept any input and stringify it. Explicit — `trim()` no longer does this implicitly. */
  coerceString(): this {
    return this.coerce((v) => String(v))
  }

  // Transforms (post-type-check, string → string)
  /** Strip surrounding whitespace before constraints run. */
  trim(): this {
    return this.transform((v) => v.trim())
  }
  /** Lowercase before constraints run. */
  lowercase(): this {
    return this.transform((v) => v.toLowerCase())
  }
  /** Uppercase before constraints run. */
  uppercase(): this {
    return this.transform((v) => v.toUpperCase())
  }

  // Constraints
  /** At least `n` characters. Error code `MIN`. */
  min(n: number): this {
    return this._addC((v) => v.length >= n, `Min length ${n}`, 'MIN', 'error', { minLength: n })
  }
  /** At most `n` characters. Error code `MAX`. */
  max(n: number): this {
    return this._addC((v) => v.length <= n, `Max length ${n}`, 'MAX', 'error', { maxLength: n })
  }
  /** Exactly `n` characters. Error code `LENGTH`. */
  length(n: number): this {
    return this._addC((v) => v.length === n, `Length must be ${n}`, 'LENGTH', 'error', {
      minLength: n,
      maxLength: n,
    })
  }
  /** Not `''`. Error code `EMPTY`. A required field still accepts `''` without this. */
  nonempty(): this {
    return this._addC((v) => v.length > 0, 'Cannot be empty', 'EMPTY', 'error', { minLength: 1 })
  }
  /** Must match `re`. Error code `PATTERN`. */
  pattern(re: RegExp, msg = 'Invalid format'): this {
    return this._addC(
      (v) => {
        // A /g or /y regex keeps lastIndex between calls; start every check from 0.
        re.lastIndex = 0
        return re.test(v)
      },
      msg,
      'PATTERN',
      'error',
      // g and y only change how a regex is stepped, not what it matches.
      { pattern: re.source, flags: re.flags.replace(/[gy]/g, '') },
    )
  }
  /** Loose `a@b.c` check. Error code `EMAIL`. */
  email(): this {
    return this._addC(
      (v) => /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(v),
      'Invalid email',
      'EMAIL',
      'error',
      { format: 'email' },
    )
  }
  /** Parsable by `new URL()`. Error code `URL`. */
  url(): this {
    return this._addC(
      (v) => {
        try {
          new URL(v)
          return true
        } catch {
          return false
        }
      },
      'Invalid URL',
      'URL',
      'error',
      { format: 'uri' },
    )
  }
  /** 8-4-4-4-12 hex. Error code `UUID`. */
  uuid(): this {
    return this._addC(
      (v) => /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(v),
      'Invalid UUID',
      'UUID',
      'error',
      { format: 'uuid' },
    )
  }
  /**
   * One of `values`; the output type narrows to their union. Error code `ONE_OF`.
   *
   * @example
   * str().oneOf(['low', 'high']) // StrField<'low' | 'high'>
   */
  oneOf<const V extends readonly string[]>(values: V): StrField<V[number] | Exclude<O, string>> {
    return this._addC(
      (v) => (values as readonly string[]).includes(v),
      `Must be one of: ${values.join(', ')}`,
      'ONE_OF',
      'error',
      { enum: [...values] },
    ) as unknown as StrField<V[number] | Exclude<O, string>>
  }
}

/**
 * A required string field.
 *
 * @example
 * str().trim().nonempty().max(200)
 * str().email().optional()
 */
export const str = () => new StrField()
