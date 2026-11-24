import { Field, ValidationFailed, check, checkAsync, isModel, unwrapResult } from "@coldmind/model";
import type { AnyModel, Infer, InferInputShape, Shape, ShapeOf, SomeField } from "@coldmind/model";
import { hasAsyncValidation } from "@coldmind/model/internal";

/** A validated value as a proc receives it: `onCall(proc, args: Model)`. */
export type Model = Record<string, unknown>;

/** The escape hatch: anything with a `parse` that returns the typed value or throws. */
export type ParseSchema<T = unknown> = { parse(input: unknown): T };

/**
 * What goes in a schema slot (`args`, `result`, `chunk`, an event's data): a model, a plain shape
 * of fields, or a single field from `@coldmind/model`. A `ParseSchema` is accepted for values that
 * are validated elsewhere.
 */
export type Schema = SomeField | AnyModel | Shape | ParseSchema;

/** The value a schema validates to: defaults applied, unknown keys dropped. */
export type OutputOf<S> = S extends Field<any, infer O, any>
  ? O
  : S extends AnyModel
    ? Infer<S>
    : S extends ParseSchema<infer T>
      ? T
      : S extends Shape
        ? Infer<S>
        : never;

/** The value a caller passes: fields with a default may be left out. */
export type InputOf<S> = S extends Field<any, infer O, any>
  ? O
  : S extends AnyModel
    ? InferInputShape<ShapeOf<S>>
    : S extends ParseSchema<infer T>
      ? T
      : S extends Shape
        ? InferInputShape<S>
        : never;

const isParse = (s: Schema): s is ParseSchema =>
  !(s instanceof Field) && !isModel(s) && typeof (s as { parse?: unknown }).parse === "function";

/** Whether the schema has async validators (`useAsync`, `refineAsync`). Those run on the server only. */
export function needsAsync(schema: Schema): boolean {
  if (schema instanceof Field) return schema.hasAsync();
  return isParse(schema) ? false : hasAsyncValidation(schema);
}

/** Validates synchronously. Throws `ValidationFailed`; throws a plain Error when the schema needs `validateAsync`. */
export function validate<S extends Schema>(schema: S, input: unknown): OutputOf<S> {
  if (schema instanceof Field) return unwrapResult(schema.validateSync([], input)) as OutputOf<S>;
  if (isParse(schema)) return schema.parse(input) as OutputOf<S>;
  return unwrapResult(check(schema as AnyModel | Shape, input)) as OutputOf<S>;
}

/** Validates, running async validators too. Throws `ValidationFailed`. */
export async function validateAsync<S extends Schema>(schema: S, input: unknown): Promise<OutputOf<S>> {
  if (schema instanceof Field) return unwrapResult(await schema.validate([], input)) as OutputOf<S>;
  if (isParse(schema)) return schema.parse(input) as OutputOf<S>;
  return unwrapResult(await checkAsync(schema as AnyModel | Shape, input)) as OutputOf<S>;
}

/**
 * Validation on the side that does not own the data (a client checking what it sends and receives):
 * everything except schemas with async validators, which only the server can run.
 */
export function validateLocal<S extends Schema>(schema: S, input: unknown): OutputOf<S> {
  return needsAsync(schema) ? (input as OutputOf<S>) : validate(schema, input);
}

export { ValidationFailed };
