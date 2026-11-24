/**
 * @module model.field.pipeline
 * `Field`, the immutable base every field kind extends: presence, defaults, coercion, validators.
 *
 * ---
 * The field base: immutable, fluent, sync by default.
 *
 *  - `optional()` / `nullable()` are type-level, not just runtime state. They
 *    widen the field's *output* type parameter, so `InferShape` produces
 *    genuinely optional / nullable properties.
 *  - Validation is synchronous unless you opt in with `useAsync()`. `use()`
 *    takes sync validators only; `validateSync` throws if any field in the
 *    tree registered an async one, so the fast path can never silently
 *    swallow a pending promise.
 */

import type { Tagged } from '@coldmind/core'
import type { Path, ValidationError, ValidationLevel, ValidationResult } from '@coldmind/core'

/**
 * A field that always produces a value: tagged by {@link Field.default}. Consumers
 * read the tag to know the field may be omitted on input and still be present on output.
 *
 * ---
 * Type-only: `Tagged` hangs the tag under a symbol nobody can name, so at runtime
 * the field is the same object. It lives here because `default()` is a field
 * method and has to be able to say it.
 */
export type Defaulted<F> = Tagged<F, 'defaulted'>

/**
 * Type-level field state. Presence is deliberately separate from the output
 * value type: a required field whose value type happens to contain
 * `undefined` is still a required property, while an optional field with a
 * default still produces a required output property.
 */
export type FieldState = {
  readonly required: boolean
  readonly nullable: boolean
  readonly defaulted: boolean
  readonly hidden: boolean
}

/** State of a newly-created field. */
export type InitialFieldState = {
  readonly required: true
  readonly nullable: false
  readonly defaulted: false
  readonly hidden: false
}

type SetState<S extends FieldState, K extends keyof FieldState, V extends boolean> = Omit<S, K> & {
  readonly [P in K]: V
}

export type OptionalState<S extends FieldState> = SetState<S, 'required', false>
export type RequiredState<S extends FieldState> = SetState<S, 'required', true>
export type NullableState<S extends FieldState> = SetState<S, 'nullable', true>
export type NonNullableState<S extends FieldState> = SetState<S, 'nullable', false>
export type DefaultedState<S extends FieldState> = SetState<S, 'defaulted', true>
export type WithoutDefaultState<S extends FieldState> = SetState<S, 'defaulted', false>
export type HiddenState<S extends FieldState> = SetState<S, 'hidden', true>

/** The state carried by a field type. */
// biome-ignore lint/suspicious/noExplicitAny: only the state parameter is inferred
export type FieldStateOf<F> = F extends Field<any, any, infer S extends FieldState> ? S : never

/** Optionality widens the value only when no default guarantees an output. */
export type OptionalOutput<O, S extends FieldState> = S['defaulted'] extends true
  ? Exclude<O, undefined>
  : O | undefined

/** Removing a default re-introduces `undefined` only for an optional field. */
export type WithoutDefaultOutput<O, S extends FieldState> = S['required'] extends false
  ? O | undefined
  : O

// ─────────────────────────────────────────────────────────────
// FIELD BASE
// ─────────────────────────────────────────────────────────────

/**
 * A custom check for {@link Field.use}: return `null` to pass, or an error at `ctx.path`.
 *
 * @example
 * const even: SyncValidator<number> = (v, ctx) =>
 *   v % 2 === 0 ? null : { path: ctx.path, msg: 'Must be even', code: 'EVEN', level: 'error' }
 */
export type SyncValidator<T> = (v: T, ctx: { path: Path }) => ValidationError | null
/** A custom check for {@link Field.useAsync}; may return a promise. */
export type AsyncValidator<T> = (
  v: T,
  ctx: { path: Path },
) => ValidationError | null | Promise<ValidationError | null>

/**
 * @internal A field's default for one use: a factory is called, an object or array literal is
 * copied so no two values share it, anything else is returned as-is.
 */
const isStructuredCloneBuiltin = (value: object): boolean =>
  value instanceof Date ||
  value instanceof RegExp ||
  value instanceof Map ||
  value instanceof Set ||
  value instanceof ArrayBuffer ||
  ArrayBuffer.isView(value)

function hasCustomPrototype(value: unknown, seen = new WeakSet<object>()): boolean {
  if (typeof value !== 'object' || value === null) return false
  if (seen.has(value)) return false
  seen.add(value)

  if (Array.isArray(value)) return value.some((v) => hasCustomPrototype(v, seen))
  if (value instanceof Map) {
    for (const [k, v] of value) {
      if (hasCustomPrototype(k, seen) || hasCustomPrototype(v, seen)) return true
    }
    return false
  }
  if (value instanceof Set) {
    for (const v of value) if (hasCustomPrototype(v, seen)) return true
    return false
  }
  if (isStructuredCloneBuiltin(value)) return false

  const proto = Object.getPrototypeOf(value)
  if (proto !== Object.prototype && proto !== null) return true
  return Object.values(value).some((v) => hasCustomPrototype(v, seen))
}

/** @internal Clone a field value without silently erasing custom prototypes. */
export function cloneValue<T>(value: T, clone?: (value: T) => T): T {
  if (typeof value !== 'object' || value === null) return value
  if (clone) return clone(value)
  if (hasCustomPrototype(value)) {
    throw new TypeError(
      'Value contains a custom class instance; use a factory default or defineDataType({ clone })',
    )
  }
  return structuredClone(value)
}

export function defaultValue<T>(
  def: T | (() => T) | undefined,
  clone?: (value: T) => T,
): T | undefined {
  if (typeof def === 'function') return (def as () => T)()
  if (typeof def === 'object' && def !== null) return cloneValue(def, clone)
  return def
}

/** @internal crosses a file boundary; not part of the public surface. */
export type ParseOut<T> = { value: T; errors: ValidationError[]; warnings: ValidationError[] }

/**
 * The runtime options a field carries. Set through modifiers, not by hand.
 *
 * ---
 * The runtime options a field carries. Named rather than inlined so another
 * package can declaration-merge its own annotations in without this one importing
 * or knowing about it.
 */
export interface FieldOpts<T> {
  req: boolean
  null: boolean
  def: T | (() => T) | undefined
  doc: string
  /** Set by `.hidden()`: left out of the model's public view. */
  hidden?: boolean
  /** Set by a `defineDataType` factory: the data type this field was made from. */
  typeName?: string
  /** Optional value clone supplied by an extension data type. */
  cloneValue?: (value: T) => T
}

/**
 * Base of every field. Immutable: each modifier returns a new field.
 *
 * Pipeline per value: default → null check → coerce → type check → transform → constraints → validators.
 *
 * @example
 * const nick = str().trim().min(2).optional().doc('Display name')
 * nick.validateSync([], '  a ').errors // [{ path: [], code: 'MIN', … }]
 *
 * @typeParam T - the base type this field parses and constrains
 * @typeParam O - the output type, widened by `optional()` / `nullable()`
 */
export abstract class Field<
  T,
  O = T,
  S extends FieldState = InitialFieldState,
> {
  /** @internal type-only state; never emitted at runtime. */
  protected declare readonly __fieldState: S

  /** @internal exposed so RecordBase and consumers can read options without reflection */
  _opts: FieldOpts<T> = { req: true, null: false, def: undefined, doc: '' }

  /** @internal */
  _sync: SyncValidator<T>[] = []
  /** @internal */
  _async: AsyncValidator<T>[] = []
  /** @internal runs BEFORE the type check — explicit opt-in coercion only */
  _coerceFn?: (v: unknown) => unknown
  /** @internal runs AFTER the type check, so it can never smuggle in a bad type */
  _transformFn?: (v: T) => T

  // ── Immutable modifier helpers ──────────────────────────

  /** Clone, apply `fn` to the clone, return it. Every modifier goes through here. */
  protected _mod(fn: (c: this) => void): this {
    const c = this.clone()
    fn(c)
    return c
  }

  /** Copy options, validators, coerce and transform onto a fresh clone. Call from every `clone()`. */
  // biome-ignore lint/suspicious/noExplicitAny: a clone may have a different output type than its source
  protected _copyBase(target: Field<T, any, any>): void {
    target._opts = { ...this._opts }
    target._sync = [...this._sync]
    target._async = [...this._async]
    target._coerceFn = this._coerceFn
    target._transformFn = this._transformFn
  }

  // ── Shared fluent API ───────────────────────────────────

  /** `undefined` is an error (code `REQ`). Fields are required by default. */
  required(): Field<T, Exclude<O, undefined>, RequiredState<S>> {
    return this._mod((c) => {
      c._opts.req = true
    }) as unknown as Field<T, Exclude<O, undefined>, RequiredState<S>>
  }

  /** `undefined` is accepted; the property becomes optional in `InferShape`. */
  optional(): Field<T, OptionalOutput<O, S>, OptionalState<S>> {
    return this._mod((c) => {
      c._opts.req = false
    }) as unknown as Field<T, OptionalOutput<O, S>, OptionalState<S>>
  }

  /** `null` is accepted (otherwise code `NULL`); the output type gains `null`. */
  nullable(): Field<T, O | null, NullableState<S>> {
    return this._mod((c) => {
      c._opts.null = true
    }) as unknown as Field<T, O | null, NullableState<S>>
  }

  /** Reject `null` and remove it from the output type. */
  nonNullable(): Field<T, Exclude<O, null>, NonNullableState<S>> {
    return this._mod((c) => {
      c._opts.null = false
    }) as unknown as Field<T, Exclude<O, null>, NonNullableState<S>>
  }

  /**
   * Value used when input is `undefined`. A function is called per use; an object or array
   * literal is copied per use.
   * The field is tagged {@link Defaulted}, and `introspect` / JSON Schema report the literal.
   *
   * @example
   * int().default(0)
   * str().default(() => crypto.randomUUID())
   * done: bool().default(false)
   *
   * ---
   * A field with a default always produces a value, so `undefined` leaves the output type.
   */
  default(
    v: T | (() => T),
  ): Defaulted<Field<T, Exclude<O, undefined>, DefaultedState<S>>> {
    return this._mod((c) => {
      c._opts.def = v
    }) as unknown as Defaulted<Field<T, Exclude<O, undefined>, DefaultedState<S>>>
  }

  /**
   * Remove a default without changing any other field behaviour. Primarily
   * used by patch projections where omission must remain omission.
   */
  withoutDefault(): Field<T, WithoutDefaultOutput<O, S>, WithoutDefaultState<S>> {
    return this._mod((c) => {
      c._opts.def = undefined
    }) as unknown as Field<T, WithoutDefaultOutput<O, S>, WithoutDefaultState<S>>
  }

  /** Attach a description, surfaced by `introspect`. */
  doc(s: string): this {
    return this._mod((c) => {
      c._opts.doc = s
    })
  }

  /**
   * Leave the field out of the model's public view. It stays in the shape and is
   * still validated. Hidden state survives all presence modifiers.
   */
  hidden(): Field<T, O, HiddenState<S>> {
    return this._mod((c) => {
      c._opts.hidden = true
    }) as unknown as Field<T, O, HiddenState<S>>
  }

  /**
   * Coerce raw input before the type check. This is the only way to accept a
   * value of the wrong type — `transform` cannot, by construction.
   */
  coerce(fn: (v: unknown) => unknown): this {
    return this._mod((c) => {
      const prev = c._coerceFn
      c._coerceFn = prev ? (v) => fn(prev(v)) : fn
    })
  }

  /** Map a value of the field's own type. Runs after the type check. */
  transform(fn: (v: T) => T): this {
    return this._mod((c) => {
      const prev = c._transformFn
      c._transformFn = prev ? (v: T) => fn(prev(v)) : fn
    })
  }

  /**
   * Register a synchronous validator. Keeps the field on the sync fast path.
   *
   * @example
   * int().use((v, ctx) => (v % 2 ? { path: ctx.path, msg: 'odd', code: 'EVEN', level: 'error' } : null))
   */
  use(fn: SyncValidator<T>): this {
    return this._mod((c) => {
      c._sync = [...c._sync, fn]
    })
  }

  /** Register an async validator. Makes this field (and its ancestors) async-only. */
  useAsync(fn: AsyncValidator<T>): this {
    return this._mod((c) => {
      c._async = [...c._async, fn]
    })
  }

  /** True if this field or anything nested under it needs `await`. */
  hasAsync(): boolean {
    return this._async.length > 0
  }

  // ── Shared prologue: default → null → coerce → type → transform ──

  /**
   * Returns either a finished result (default/null short circuit, or a type
   * failure) or the value to hand to `_parse*`.
   */
  protected _prologue(
    path: Path,
    raw: unknown,
  ): { done: true; result: ValidationResult<O> } | { done: false; value: T } {
    let val: unknown = raw

    // 1. Default resolution
    if (val === undefined) {
      val = defaultValue(this._opts.def, this._opts.cloneValue)

      if (val === undefined) {
        const errors = this._opts.req
          ? [{ path, msg: 'Required', code: 'REQ', level: 'error' as const }]
          : []
        return {
          done: true,
          result: { isValid: errors.length === 0, value: undefined, errors, warnings: [] },
        }
      }
    }

    // 2. Null check
    if (val === null) {
      const errors = this._opts.null
        ? []
        : [{ path, msg: 'Cannot be null', code: 'NULL', level: 'error' as const }]
      return {
        done: true,
        result: {
          isValid: errors.length === 0,
          value: this._opts.null ? (null as O) : undefined,
          errors,
          warnings: [],
        },
      }
    }

    // 3. Coercion (explicit opt-in only)
    if (this._coerceFn) {
      try {
        val = this._coerceFn(val)
      } catch (e) {
        return {
          done: true,
          result: {
            isValid: false,
            value: undefined,
            errors: [
              { path, msg: `Coercion failed: ${String(e)}`, code: 'COERCE_ERROR', level: 'error' },
            ],
            warnings: [],
          },
        }
      }
    }

    // 4. Type check
    if (!this._checkType(val)) {
      return {
        done: true,
        result: {
          isValid: false,
          value: undefined,
          errors: [{ path, msg: 'Type mismatch', code: 'TYPE', level: 'error' }],
          warnings: [],
        },
      }
    }

    // 5. Transform (typed T → T; cannot change the type)
    let typed = val as T
    if (this._transformFn) {
      try {
        typed = this._transformFn(typed)
      } catch (e) {
        return {
          done: true,
          result: {
            isValid: false,
            value: undefined,
            errors: [
              {
                path,
                msg: `Transform failed: ${String(e)}`,
                code: 'TRANSFORM_ERROR',
                level: 'error',
              },
            ],
            warnings: [],
          },
        }
      }
    }

    return { done: false, value: typed }
  }

  /** Run the `use()` validators, sorting what they return into errors and warnings by level. */
  private _runSyncTests(
    path: Path,
    value: T,
    errs: ValidationError[],
    warns: ValidationError[],
  ): void {
    for (const t of this._sync) {
      const e = t(value, { path })
      if (e) (e.level === 'warning' ? warns : errs).push(e)
    }
  }

  // ── Public entry points ─────────────────────────────────

  /**
   * Validate one value. Returns a result — the value is `result.value`, set only when `isValid`.
   *
   * @example
   * const r = int().min(1).validateSync(['priority'], 0)
   * r.isValid // false; r.errors[0].path is ['priority']
   *
   * @param path - prefix for error paths; `[]` at the root
   * @throws {Error} when this field or a nested one registered an async validator
   */
  validateSync(path: Path, raw: unknown): ValidationResult<O> {
    if (this.hasAsync()) {
      throw new Error(
        `validateSync() called on a field with async validators at path ${JSON.stringify(path)} — use validate()`,
      )
    }
    return this._validateSyncUnchecked(path, raw)
  }

  /** @internal skips the async guard; callers must have checked already. */
  _validateSyncUnchecked(path: Path, raw: unknown): ValidationResult<O> {
    const pro = this._prologue(path, raw)
    if (pro.done) return pro.result

    const sub = this._parseSync(path, pro.value)
    const errs = sub.errors
    const warns = sub.warnings

    // Custom validators are skipped on structural failure
    if (errs.length === 0) this._runSyncTests(path, sub.value, errs, warns)

    return {
      isValid: errs.length === 0,
      value: errs.length === 0 ? (sub.value as unknown as O) : undefined,
      errors: errs,
      warnings: warns,
    }
  }

  /** Asynchronous validation. Works for sync fields too. */
  async validate(path: Path, raw: unknown): Promise<ValidationResult<O>> {
    const pro = this._prologue(path, raw)
    if (pro.done) return pro.result

    const sub = await this._parseAsync(path, pro.value)
    const errs = sub.errors
    const warns = sub.warnings

    if (errs.length === 0) {
      this._runSyncTests(path, sub.value, errs, warns)
      for (const t of this._async) {
        const e = await t(sub.value, { path })
        if (e) (e.level === 'warning' ? warns : errs).push(e)
      }
    }

    return {
      isValid: errs.length === 0,
      value: errs.length === 0 ? (sub.value as unknown as O) : undefined,
      errors: errs,
      warnings: warns,
    }
  }

  /** A new instance of the same subclass carrying this field's state. Must call {@link Field._copyBase}. */
  abstract clone(): this
  /** Whether `v` is this field's base type. `false` fails the value with code `TYPE`. */
  protected abstract _checkType(v: unknown): boolean
  /** Structural parse after the type check: constraints, items, keys or variants. */
  protected abstract _parseSync(path: Path, v: T): ParseOut<T>

  /** Async structural parse. Only container fields need to override this. */
  protected async _parseAsync(path: Path, v: T): Promise<ParseOut<T>> {
    return this._parseSync(path, v)
  }
}

// ─────────────────────────────────────────────────────────────
// CONSTRAINT MIXIN (shared by StrField / NumField)
// ─────────────────────────────────────────────────────────────

/** @internal crosses a file boundary; not part of the public surface. */
export type Constraint<T> = {
  check: (v: T) => boolean
  msg: string
  code: string
  level: ValidationLevel
  /** Bound values (`minLength`, `enum`, …) for JSON Schema and docs. */
  params?: Readonly<Record<string, unknown>>
}

/**
 * @internal crosses a file boundary; not part of the public surface.
 *
 * Base of the fields with declarative constraints (`StrField`, `NumField`). A constraint is a
 * check plus the code, message and params that `introspect` and JSON Schema report.
 */
export abstract class ConstrainedField<
  T,
  O = T,
  S extends FieldState = InitialFieldState,
> extends Field<T, O, S> {
  /** @internal */
  _constraints: Constraint<T>[] = []

  /** Copy constraints onto a fresh clone. Call from `clone()` after {@link Field._copyBase}. */
  // biome-ignore lint/suspicious/noExplicitAny: a clone may have a different output type than its source
  protected _copyConstraints(target: ConstrainedField<T, any, any>): void {
    target._constraints = [...this._constraints]
  }

  /** Append a constraint and return the new field. `params` use JSON Schema keyword names. */
  protected _addC(
    check: (v: T) => boolean,
    msg: string,
    code: string,
    level: ValidationLevel = 'error',
    params?: Readonly<Record<string, unknown>>,
  ): this {
    return this._mod((c) => {
      const cc = c as unknown as ConstrainedField<T, O, S>
      cc._constraints = [
        ...cc._constraints,
        params ? { check, msg, code, level, params } : { check, msg, code, level },
      ]
    })
  }

  /** Run every constraint. None short-circuits, so all failures are reported together. */
  protected _parseSync(path: Path, v: T): ParseOut<T> {
    const errors: ValidationError[] = []
    const warnings: ValidationError[] = []
    for (const con of this._constraints) {
      if (!con.check(v)) {
        const e = { path, msg: con.msg, code: con.code, level: con.level }
        ;(con.level === 'warning' ? warnings : errors).push(e)
      }
    }
    return { value: v, errors, warnings }
  }
}
