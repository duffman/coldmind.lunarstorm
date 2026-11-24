/**
 * @module model.shape.inference
 * `Shape`, `InferShape` / `Infer`, and the key type algebra for fields and models.
 *
 * ---
 * A shape is a record of fields, and everything derived from one.
 * Type inference lives here; validation logic is in `validation/model.validate.ts`.
 */

import type { Simplify } from '@coldmind/core'
import type { AnyModel, ShapeInput, ShapeOf } from '@coldmind/model/define-model'
import type { Field, FieldStateOf } from '@coldmind/model/src/fields/model.field.pipeline.ts'

/** A field of unknown base and output type. Use in constraints, not as a value type. */
// biome-ignore lint/suspicious/noExplicitAny: Field is invariant in T; unknown would reject every concrete field
export type SomeField = Field<any, any, any>

/**
 * A plain record of fields. Models are built from one.
 *
 * @example
 * const input = { title: str().nonempty(), priority: int().min(1).optional() } satisfies Shape
 */
export type Shape = Record<string, SomeField>

/** The *output* type of a field — what you get after a successful parse. */
// biome-ignore lint/suspicious/noExplicitAny: Field is invariant in T; only O is inferred
export type InferField<F> = F extends Field<any, infer O, any> ? O : never

/**
 * Keys omitted from a successful output when absent. Presence comes from
 * explicit field state, not from whether the value type happens to contain
 * `undefined`.
 */
type OptionalKeys<S extends Shape> = {
  [K in keyof S]-?: FieldStateOf<S[K]>['required'] extends false
    ? FieldStateOf<S[K]>['defaulted'] extends false
      ? K
      : never
    : never
}[keyof S]

/**
 * The value type a shape validates to.
 *
 * @example
 * type Input = InferShape<typeof input> // { title: string; priority?: number }
 *
 * ---
 * Fields whose output includes `undefined` become optional *properties*, not
 * merely `T | undefined` ones — so `{ a: str().optional() }` accepts `{}`.
 */
export type InferShape<S extends Shape> = Simplify<
  { [K in Exclude<keyof S, OptionalKeys<S>>]: InferField<S[K]> } & {
    [K in OptionalKeys<S>]?: InferField<S[K]>
  }
>

/** Keys callers may omit on input: explicitly optional fields and fields with defaults. */
type InputOptionalKeys<S extends Shape> = {
  [K in keyof S]-?: FieldStateOf<S[K]>['required'] extends false
    ? K
    : FieldStateOf<S[K]>['defaulted'] extends true
      ? K
      : never
}[keyof S]

/**
 * The value a shape accepts before defaults are materialized. Unlike
 * {@link InferShape}, this describes the input side of validation.
 */
export type InferInputShape<S extends Shape> = Simplify<
  { [K in Exclude<keyof S, InputOptionalKeys<S>>]: InferField<S[K]> } & {
    [K in InputOptionalKeys<S>]?: InferField<S[K]>
  }
>

/**
 * The value type of a model or a shape — what `parse` returns.
 *
 * @example
 * type Account = Infer<typeof Account> // { id: number; email: string }
 */
export type Infer<X> = X extends AnyModel
  ? InferShape<ShapeOf<X>>
  : X extends Shape
    ? InferShape<X>
    : never

