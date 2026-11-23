/**
 * @module model.validation
 * Validation verbs: `check` / `checkAsync` (result), `parse` / `parseAsync` (value or throw).
 *
 * ---
 * One word per job. `check` answers "is this valid, and what are the errors";
 * `parse` answers "give me the value or throw". The sync verbs get the short
 * names because an async validator is the rare case, and asking for the async
 * form on a sync shape still works.
 */

import { type ValidationError, type ValidationResult, formatErrors } from '@coldmind/core/validation'
import type { Shape, Infer } from '@coldmind/model/src/models/model.shape.inference.ts'
import {
  type AnyModel,
  type Model,
  type NameOf,
  type ShapeInput,
  type ShapeOf,
  buildModel,
  metaOf,
} from '@coldmind/model/define-model'
import type { AsyncValidator, SyncValidator } from '@coldmind/model/src/fields/model.field.pipeline.ts'
import { ObjField } from '@coldmind/model/src/fields/model.field.object.ts'

/**
 * `strict: true` turns unknown keys from warnings into errors. A model can carry it too.
 *
 * ---
 * How a shape treats a key it does not declare. Lenient is the historical
 * behaviour — a warning, and the key is dropped from the value. Strict makes
 * it an error, which is what a trust boundary wants: an unexpected key means
 * the sender and the contract disagree, and guessing which one is right is
 * how data gets silently discarded.
 */
export type ShapeOptions = { readonly strict?: boolean }

/**
 * The object field that validates a top-level shape, built once per shape and strictness.
 * A top-level `check` and a nested `obj()` are then one code path with one set of codes.
 */
const roots = new WeakMap<object, { lenient?: ObjField<Shape>; strict?: ObjField<Shape> }>()

// Internal provenance for handing a parsed scalar record to a storage boundary.
// Deliberately not used by check/checkAsync: independent validation must still run.
const canonical = new WeakMap<object, { fields: Shape; values: Record<string, unknown> }>()

/** Record a valid, all-scalar result so {@link canonicalFor} can recognise it later, unchanged. */
function remember(target: ShapeInput, value: unknown): void {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return
  const values: Record<string, unknown> = {}
  for (const key of Object.keys(value)) {
    const v = (value as Record<string, unknown>)[key]
    // Mutable nested values need validation again; scalar fields can be compared exactly.
    if (v !== null && typeof v === 'object') return
    values[key] = v
  }
  canonical.set(value, { fields: metaOf(target).fields, values })
}

/** @internal Storage-only canonical handoff for an unchanged value and exact field projection. */
export function canonicalFor(target: ShapeInput, value: unknown): Record<string, unknown> | undefined {
  if (typeof value !== 'object' || value === null) return undefined
  const prior = canonical.get(value)
  if (!prior) return undefined
  const fields = metaOf(target).fields
  const a = Object.keys(fields)
  const b = Object.keys(prior.fields)
  if (a.length !== b.length || a.some((key) => fields[key] !== prior.fields[key])) return undefined
  const current = value as Record<string, unknown>
  const keys = Object.keys(current)
  const saved = Object.keys(prior.values)
  if (keys.length !== saved.length || keys.some((key) => !Object.is(current[key], prior.values[key]))) return undefined
  return current
}

/** @internal Whether synchronous validation is unavailable for this projection. */
export function hasAsyncValidation(target: ShapeInput): boolean {
  return rootOf(target).hasAsync()
}

/** The cached root field for `target`. `options.strict` overrides the model's own strictness. */
function rootOf(target: ShapeInput, options?: ShapeOptions): ObjField<Shape> {
  const { fields, strict, owner, refinements, asyncRefinements } = metaOf(target)
  const mode = (options?.strict ?? strict) ? 'strict' : 'lenient'
  let entry = roots.get(owner)
  if (!entry) roots.set(owner, (entry = {}))
  let field = entry[mode]
  if (!field) {
    let base: ObjField<Shape> = new ObjField(fields)
    if (mode === 'strict') base = base.strict()
    for (const fn of refinements ?? []) base = base.use(fn) as ObjField<Shape>
    for (const fn of asyncRefinements ?? []) base = base.useAsync(fn) as ObjField<Shape>
    field = entry[mode] = base
  }
  return field
}

/**
 * Add a synchronous whole-model refinement. It runs after every field has
 * parsed successfully, so the callback always receives the canonical parsed value.
 */
export function refine<M extends AnyModel>(
  model: M,
  fn: SyncValidator<Infer<M>>,
): Model<NameOf<M>, ShapeOf<M>> {
  const meta = metaOf(model)
  return buildModel(meta.name, meta.fields, meta.strict, {
    refinements: [...(meta.refinements ?? []), fn as SyncValidator<any>],
    asyncRefinements: meta.asyncRefinements,
  }) as Model<NameOf<M>, ShapeOf<M>>
}

/** Async whole-model refinement. Makes the resulting model async-only. */
export function refineAsync<M extends AnyModel>(
  model: M,
  fn: AsyncValidator<Infer<M>>,
): Model<NameOf<M>, ShapeOf<M>> {
  const meta = metaOf(model)
  return buildModel(meta.name, meta.fields, meta.strict, {
    refinements: meta.refinements,
    asyncRefinements: [...(meta.asyncRefinements ?? []), fn as AsyncValidator<any>],
  }) as Model<NameOf<M>, ShapeOf<M>>
}

/**
 * Validate `data` against a model or a shape. Returns a result; the value is `result.value`, set only when `isValid`.
 *
 * @example
 * const r = check(TaskInput, body, { strict: true })
 * if (!r.isValid) return badRequest(formatErrors(r.errors))
 * r.value // Infer<typeof TaskInput>, defaults applied, unknown keys dropped
 *
 * @param target - a model from {@link model} or a projection, or a plain {@link Shape}
 * @throws {Error} when any field registered an async validator — use {@link checkAsync}
 * @see {@link parse} — the same check, returning the value or throwing
 *
 * ---
 * Non-object input fails with `TYPE`, `undefined` with `REQ`, `null` with `NULL` — the
 * same codes a nested `obj()` gives.
 */
export function check<X extends ShapeInput>(
  target: X,
  data: unknown,
  options?: ShapeOptions,
): ValidationResult<Infer<X>> {
  const root = rootOf(target, options)
  if (root.hasAsync()) {
    throw new Error('check() called on a shape with async validators — use checkAsync()')
  }
  const result = root._validateSyncUnchecked([], data) as ValidationResult<Infer<X>>
  if (result.isValid) remember(target, result.value)
  return result
}

/**
 * Async {@link check}. Required when a field uses `useAsync()`; works for sync shapes too.
 *
 * @example
 * const r = await checkAsync(Signup, body)
 */
export async function checkAsync<X extends ShapeInput>(
  target: X,
  data: unknown,
  options?: ShapeOptions,
): Promise<ValidationResult<Infer<X>>> {
  const result = (await rootOf(target, options).validate([], data)) as ValidationResult<Infer<X>>
  if (result.isValid) remember(target, result.value)
  return result
}

/**
 * Thrown by {@link parse}, {@link parseAsync} and {@link unwrapResult}.
 *
 * @example
 * try {
 *   parse(Account, body)
 * } catch (e) {
 *   if (e instanceof ValidationFailed) return badRequest(e.message) // formatErrors text
 * }
 *
 * ---
 * Carries the whole result, not only the message, so a caller that wants the
 * paths back does not have to parse them out of prose. `context` names what
 * was being validated, when the thrower knows it.
 */
export class ValidationFailed extends Error {
  override readonly name = 'ValidationFailed'
  readonly errors: readonly ValidationError[]
  readonly warnings: readonly ValidationError[]

  constructor(
    errors: readonly ValidationError[],
    warnings: readonly ValidationError[] = [],
    context?: string,
  ) {
    const text = formatErrors([...errors])
    super(context ? `${context}: ${text}` : text)
    this.errors = errors
    this.warnings = warnings
  }
}

/** Unwrap a result: the value, or a {@link ValidationFailed}. */
export function unwrapResult<T>(result: ValidationResult<T>, context?: string): T {
  if (!result.isValid) throw new ValidationFailed(result.errors, result.warnings, context)
  return result.value as T
}

/**
 * Validate and return the value, or throw. The throwing twin of {@link check}.
 *
 * @example
 * const account = parse(Account, body, { strict: true })
 * account.email // string, trimmed and lowercased
 * const signup = parse(makeOptional(Account, Account.id), body) // defaults applied
 *
 * @throws {ValidationFailed} when the data is invalid; `message` is the {@link formatErrors} text
 * @throws {Error} when any field registered an async validator — use {@link parseAsync}
 */
export function parse<X extends ShapeInput>(
  target: X,
  data: unknown,
  options?: ShapeOptions,
): Infer<X> {
  return unwrapResult(check(target, data, options))
}

/**
 * Async {@link parse}.
 *
 * @example
 * const signup = await parseAsync(Signup, body)
 *
 * @throws {ValidationFailed} when the data is invalid
 */
export async function parseAsync<X extends ShapeInput>(
  target: X,
  data: unknown,
  options?: ShapeOptions,
): Promise<Infer<X>> {
  return unwrapResult(await checkAsync(target, data, options))
}
