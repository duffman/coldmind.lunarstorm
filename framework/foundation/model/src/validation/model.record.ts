/**
 * @module model.record
 * `record(model)`: a model whose instances hold values (Active Record style).
 */
import type { ValidationResult } from '@coldmind/core'
import {
  type AnyModel,
  type Model,
  type NameOf,
  type ShapeInput,
  type ShapeOf,
  fieldsOf,
  nameOf,
} from '@coldmind/model/define-model'
import { cloneValue, defaultValue } from '@coldmind/model/src/fields/model.field.pipeline.ts'
import type { InferShape, Shape } from '@coldmind/model/src/models/model.shape.inference.ts'
import { ValidationFailed, check, checkAsync } from '@coldmind/model/src/validation/model.validation.ts'

// ─────────────────────────────────────────────────────────────
// RECORDS  (Active Record DTO)
//
// A record is built from a model rather than repeating its fields: the model
// is the one definition, projected with `pick` / `omit` / `merge` before it is
// recorded, and the record adds instances on top.
//
// A Proxy intercepts shape keys and forwards them to `_data`; everything
// else (methods, $modelName, shape) resolves on the instance. Writes to
// non-shape properties are rejected, which throws under ESM strict mode.
// ─────────────────────────────────────────────────────────────

/** A live instance: the field values as properties, plus the {@link RecordBase} methods. */
export type RecordOf<S extends Shape> = RecordBase<S> & InferShape<S>

/**
 * A mutable, value-holding instance over a shape. Create through {@link record}'s `create`.
 *
 * @example
 * const User = record(model('user', { name: str().nonempty(), age: int().optional() }))
 * const u = User.create({ name: 'Ada' })
 * u.age = 36
 * u.set({ name: '' }).isValid // false, and u.name is still 'Ada'
 */
export class RecordBase<S extends Shape> {
  /** @internal */
  _data: Record<string, unknown> = {}
  public readonly shape: S
  /** @internal What validation runs against: the model, so its strictness and refinements apply. */
  readonly _model: ShapeInput

  /**
   * Seeds each field from `initial`, otherwise from its explicit default. Missing nullable
   * fields stay missing; nullable means `null` is accepted, not synthesized. Nothing is validated.
   * Returns the Proxy, not `this`.
   */
  constructor(
    public readonly $modelName: string,
    shape: S,
    initial?: Partial<InferShape<S>>,
    model?: ShapeInput,
  ) {
    this.shape = Object.freeze({ ...shape }) as S
    this._model = model ?? this.shape
    for (const [key, field] of Object.entries(this.shape)) {
      let val: unknown =
        initial && Object.prototype.hasOwnProperty.call(initial, key)
          ? (initial as Record<string, unknown>)[key]
          : undefined

      if (val === undefined) {
        val = defaultValue(field._opts.def, field._opts.cloneValue)
      }

      // An absent optional value is not stored at all, so `toJSON` does not
      // print `nick: undefined` for a field nobody set.
      if (val !== undefined) this._data[key] = val
    }

    // Deliberate. The Proxy *is* the instance: shape keys forward to `_data`
    // and everything else resolves on the target, which is what makes
    // `user.email` and `user.check()` both work on one object. Returning
    // `this` instead would hand back something with no field accessors at all.
    // biome-ignore lint/correctness/noConstructorReturn: the Proxy is the instance
    return new Proxy(this, {
      get(target, prop) {
        if (typeof prop === 'string' && Object.hasOwn(target.shape, prop)) return target._data[prop]
        const value = target[prop as keyof RecordBase<S>]
        // Bound to the target, not handed back bare. A method reached through
        // the Proxy would otherwise run with the Proxy as `this`, and a `#`
        // field lives on the target alone — `set()` reaching `#commit` threw
        // "Cannot access private method" for exactly that reason.
        return typeof value === 'function' ? value.bind(target) : value
      },
      set(target, prop, value) {
        if (typeof prop === 'string' && Object.hasOwn(target.shape, prop)) {
          if (value === undefined) delete target._data[prop]
          else target._data[prop] = value
          return true
        }
        return false
      },
    }) as RecordOf<S>
  }

  /**
   * Validate the current values. Does not change them.
   *
   * @example
   * u.nick = ''
   * u.check().errors // [{ path: ['nick'], … }]
   *
   * @throws {Error} when the shape has async validators — use {@link RecordBase.checkAsync}
   */
  check(): ValidationResult<InferShape<S>> {
    return check(this._model, this._data) as ValidationResult<InferShape<S>>
  }

  /** Async {@link RecordBase.check}. */
  checkAsync(): Promise<ValidationResult<InferShape<S>>> {
    return checkAsync(this._model, this._data) as Promise<ValidationResult<InferShape<S>>>
  }

  /**
   * Validate the current values merged with `patch`, and apply them only if valid.
   *
   * @example
   * u.set({ email: 'bad' }).isValid // false — u.email is unchanged
   * u.set({ email: 'a@b.co' })      // applied
   *
   * ---
   * Validate-then-apply: the instance is never left dirty on failure.
   */
  set(patch: Partial<InferShape<S>>): ValidationResult<InferShape<S>> {
    const result = check(this._model, { ...this._data, ...patch }) as ValidationResult<InferShape<S>>
    if (result.isValid && result.value) this.#commit(result.value)
    return result
  }

  /** Async {@link RecordBase.set}. */
  async setAsync(patch: Partial<InferShape<S>>): Promise<ValidationResult<InferShape<S>>> {
    const result = (await checkAsync(this._model, { ...this._data, ...patch })) as ValidationResult<
      InferShape<S>
    >
    if (result.isValid && result.value) this.#commit(result.value)
    return result
  }

  /**
   * Replace `_data` wholesale rather than `Object.assign`-ing, so a key that
   * validated away to `undefined` is actually cleared instead of going stale.
   */
  #commit(value: InferShape<S>): void {
    for (const key of Object.keys(this.shape)) {
      const v = (value as Record<string, unknown>)[key]
      if (v === undefined) delete this._data[key]
      else this._data[key] = v
    }
  }

  /** Assign without validating. Deliberate use only. */
  assign(data: Partial<InferShape<S>>): this {
    for (const [key, value] of Object.entries(data)) {
      if (!Object.hasOwn(this.shape, key)) continue
      if (value === undefined) delete this._data[key]
      else this._data[key] = value
    }
    return this
  }

  /** Deep clone — nested objects and arrays are not shared with the original. */
  clone(): RecordOf<S> {
    const data: Record<string, unknown> = {}
    for (const [key, value] of Object.entries(this._data)) {
      const field = this.shape[key]
      if (!field) continue
      data[key] = cloneValue(value, field._opts.cloneValue as ((value: unknown) => unknown) | undefined)
    }
    return new RecordBase(this.$modelName, this.shape, data as Partial<InferShape<S>>, this._model) as RecordOf<S>
  }

  /** Shallow snapshot of the set values. Use `clone()` to mutate nested values. */
  toJSON(): InferShape<S> {
    return { ...(this._data as InferShape<S>) }
  }
}

/** Instance members a field name would shadow. `record()` rejects such a model. */
const RESERVED: ReadonlySet<string> = new Set([
  ...Object.getOwnPropertyNames(RecordBase.prototype),
  '_data',
  '_model',
  '$modelName',
  'shape',
])

// ─────────────────────────────────────────────────────────────
// RECORD TYPE  (factory)
// ─────────────────────────────────────────────────────────────

/**
 * What {@link record} returns: stateless validation plus `create` for live instances.
 *
 * ---
 * Written out rather than inferred so declaration emit has a name to print.
 */
export interface RecordType<N extends string, S extends Shape> {
  /** The model this record was built from. Project it, then `record()` the projection. */
  readonly model: Model<N, S>
  readonly name: N
  readonly shape: S

  /** A live instance. Defaults are applied; nothing is validated until `check()`. */
  create(initial?: Partial<InferShape<S>>): RecordOf<S>
  /** Validate without an instance. */
  check(data: unknown): ValidationResult<InferShape<S>>
  /** Async {@link RecordType.check}. */
  checkAsync(data: unknown): Promise<ValidationResult<InferShape<S>>>
  /** Validate and return a live instance, or throw `ValidationFailed`. */
  parse(data: unknown): RecordOf<S>
  /** Async {@link RecordType.parse}. */
  parseAsync(data: unknown): Promise<RecordOf<S>>
}

/**
 * Instances over a model (Active Record style).
 *
 * @example
 * const User = record(model('user', { name: str().nonempty(), age: int().optional() }))
 * const u = User.parse({ name: 'Ada' }) // live instance, or ValidationFailed
 * u.age = 36
 * const Public = record(pick(UserModel, UserModel.name)) // project first, then record
 *
 * @throws {Error} when a field is named like a record member (`set`, `check`, `clone`, `toJSON`, `shape`, …)
 * @see {@link model} for handles without instances
 */
export function record<M extends AnyModel>(m: M): RecordType<NameOf<M>, ShapeOf<M>> {
  const name = nameOf(m)
  const shape = fieldsOf(m)
  const clash = Object.keys(shape).filter((k) => RESERVED.has(k))
  if (clash.length > 0) {
    throw new Error(
      `record(): model "${name || '(anonymous)'}" has fields that shadow record members: ${clash.join(', ')}`,
    )
  }
  type V = InferShape<ShapeOf<M>>
  const create = (initial?: Partial<V>) =>
    new RecordBase(name, shape, initial, m) as RecordOf<ShapeOf<M>>

  return {
    model: m as unknown as Model<NameOf<M>, ShapeOf<M>>,
    name,
    shape,
    create,
    check: (data) => check(m, data) as ValidationResult<V>,
    checkAsync: (data) => checkAsync(m, data) as Promise<ValidationResult<V>>,
    parse(data) {
      const r = check(m, data) as ValidationResult<V>
      if (!r.isValid || !r.value)
        throw new ValidationFailed(r.errors, r.warnings, name || undefined)
      return create(r.value as Partial<V>)
    },
    async parseAsync(data) {
      const r = (await checkAsync(m, data)) as ValidationResult<V>
      if (!r.isValid || !r.value)
        throw new ValidationFailed(r.errors, r.warnings, name || undefined)
      return create(r.value as Partial<V>)
    },
  }
}
