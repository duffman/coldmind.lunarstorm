/**
 * @module model.field.list
 * Array field: `list()`, `listOf()` factories and {@link ListField}.
 */

import type { ValidationError, Path } from '@coldmind/core'
import type { AnyModel, Model, ShapeOf } from '@coldmind/model/define-model'
import { model } from '@coldmind/model/define-model'
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
import type { InferField, InferShape, Shape, SomeField } from '@coldmind/model/src/models/model.shape.inference.ts'
import { obj, type ObjField } from '@coldmind/model/src/fields/model.field.object.ts'

/**
 * An array whose every item is validated by `item`. Errors carry the index in their path.
 *
 * @example
 * const tags = list(str().nonempty()).maxLength(10)
 * tags.validateSync([], ['a', '']).errors[0]?.path // [1]
 *
 * @typeParam F - the item field
 */
export class ListField<
  F extends SomeField,
  O = InferField<F>[],
  S extends FieldState = InitialFieldState,
> extends Field<InferField<F>[], O, S> {
  /** @internal JSON Schema `minItems`. */
  _minItems?: number
  /** @internal JSON Schema `maxItems`. */
  _maxItems?: number

  constructor(private _item: F) {
    super()
  }

  /** Access the inner item field. */
  get item(): F {
    return this._item
  }

  /** The item field is shared, not cloned: fields are immutable. */
  clone(): this {
    const c = new ListField<F, O, S>(this._item) as unknown as this
    this._copyBase(c)
    c._minItems = this._minItems
    c._maxItems = this._maxItems
    return c
  }

  override hasAsync(): boolean {
    return super.hasAsync() || this._item.hasAsync()
  }

  protected _checkType(v: unknown): boolean {
    return Array.isArray(v)
  }

  override optional(): ListField<F, OptionalOutput<O, S>, OptionalState<S>> {
    return super.optional() as ListField<F, OptionalOutput<O, S>, OptionalState<S>>
  }
  override nullable(): ListField<F, O | null, NullableState<S>> {
    return super.nullable() as ListField<F, O | null, NullableState<S>>
  }
  override nonNullable(): ListField<F, Exclude<O, null>, NonNullableState<S>> {
    return super.nonNullable() as ListField<F, Exclude<O, null>, NonNullableState<S>>
  }
  override required(): ListField<F, Exclude<O, undefined>, RequiredState<S>> {
    return super.required() as ListField<F, Exclude<O, undefined>, RequiredState<S>>
  }
  override default(
    v: InferField<F>[] | (() => InferField<F>[]),
  ): Defaulted<ListField<F, Exclude<O, undefined>, DefaultedState<S>>> {
    return super.default(v) as unknown as Defaulted<
      ListField<F, Exclude<O, undefined>, DefaultedState<S>>
    >
  }
  override withoutDefault(): ListField<
    F,
    WithoutDefaultOutput<O, S>,
    WithoutDefaultState<S>
  > {
    return super.withoutDefault() as ListField<
      F,
      WithoutDefaultOutput<O, S>,
      WithoutDefaultState<S>
    >
  }
  override hidden(): ListField<F, O, HiddenState<S>> {
    return super.hidden() as ListField<F, O, HiddenState<S>>
  }

  protected _parseSync(path: Path, v: InferField<F>[]): ParseOut<InferField<F>[]> {
    const value: InferField<F>[] = []
    const errors: ValidationError[] = []
    const warnings: ValidationError[] = []

    for (let i = 0; i < v.length; i++) {
      const r = this._item._validateSyncUnchecked([...path, i], v[i])
      value.push(r.value as InferField<F>)
      errors.push(...r.errors)
      warnings.push(...r.warnings)
    }

    return { value, errors, warnings }
  }

  protected override async _parseAsync(
    path: Path,
    v: InferField<F>[],
  ): Promise<ParseOut<InferField<F>[]>> {
    const results = await Promise.all(v.map((item, i) => this._item.validate([...path, i], item)))
    return {
      value: results.map((r) => r.value) as InferField<F>[],
      errors: results.flatMap((r) => r.errors),
      warnings: results.flatMap((r) => r.warnings),
    }
  }

  /** At least `n` items. Error code `MIN_LENGTH`. */
  minLength(n: number): this {
    return this._mod((c) => {
      c._minItems = c._minItems === undefined ? n : Math.max(c._minItems, n)
      c._sync = [
        ...c._sync,
        (v, ctx) =>
          v.length >= n
            ? null
            : { path: ctx.path, msg: `Min ${n} items`, code: 'MIN_LENGTH', level: 'error' },
      ]
    })
  }

  /** At most `n` items. Error code `MAX_LENGTH`. */
  maxLength(n: number): this {
    return this._mod((c) => {
      c._maxItems = c._maxItems === undefined ? n : Math.min(c._maxItems, n)
      c._sync = [
        ...c._sync,
        (v, ctx) =>
          v.length <= n
            ? null
            : { path: ctx.path, msg: `Max ${n} items`, code: 'MAX_LENGTH', level: 'error' },
      ]
    })
  }
}

/**
 * A required array of `item`. Item may be a field, a model, or a shape.
 *
 * @example
 * list(int().positive()).minLength(1)
 * list(Account)
 */
export function list<F extends SomeField>(item: F): ListField<F>
export function list<M extends AnyModel>(item: M): ListField<ObjField<ShapeOf<M>>>
export function list<S extends Shape>(item: S): ListField<ObjField<S>>
export function list(item: SomeField | AnyModel | Shape): ListField<SomeField> {
  if (item instanceof Field) return new ListField(item)
  return new ListField(obj(item as Shape | AnyModel))
}

/**
 * `{ items: T[] }` — the list envelope every list procedure was writing by hand.
 *
 * @example
 * listOf(User)          // model({ items: list(User) })
 * listOf(pick(User, User.id, User.name))
 */
export function listOf<M extends AnyModel>(
  item: M,
): Model<'', { items: ListField<ObjField<ShapeOf<M>>> }>
export function listOf<F extends SomeField>(item: F): Model<'', { items: ListField<F> }>
export function listOf(item: SomeField | AnyModel) {
  return model({ items: item instanceof Field ? list(item) : list(item) })
}
