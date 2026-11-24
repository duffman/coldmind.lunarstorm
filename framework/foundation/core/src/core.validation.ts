/**
 * @module core.validation
 * `ValidationError`, `ValidationResult`, and `formatErrors` for rendering them.
 *
 * ---
 * What a failed check looks like, everywhere. A field reports it, a shape
 * collects it, a table rejects a row with it, and an RPC puts it on a wire —
 * one vocabulary, so an error never has to be translated between layers.
 */

/** Location of a value: object keys and array indexes from the root. */
export type Path = (string | number)[]

/** `'error'` fails validation; `'warning'` is reported but does not. */
export type ValidationLevel = 'error' | 'warning'

/**
 * One failed check.
 *
 * @example
 * { path: ['items', 0, 'qty'], msg: 'Min 1', code: 'MIN', level: 'error' }
 */
export type ValidationError = {
  path: Path
  msg: string
  code: string
  level: ValidationLevel
}

/**
 * What every `validate*` and `parse*` returns. It is not the value: read `.value` after checking `.isValid`.
 *
 * @example
 * const r = check(shape, input)
 * if (!r.isValid) throw new Error(formatErrors(r.errors))
 * use(r.value!)
 */
export type ValidationResult<T> = {
  isValid: boolean
  value: T | undefined
  errors: ValidationError[]
  warnings: ValidationError[]
}

/** Render an error path as a dotted/indexed string: `["a", 0, "b"]` → `a[0].b`. */
export function formatPath(path: Path): string {
  let out = ''
  for (const seg of path) {
    if (typeof seg === 'number') out += `[${seg}]`
    else out += out ? `.${seg}` : seg
  }
  return out
}

/**
 * Errors on one line: `path: msg; path: msg`.
 *
 * @example
 * formatErrors(r.errors) // 'title: Cannot be empty; priority: Max 5'
 *
 * ---
 * the model layer's errors, rendered on one line. Every layer reports them the same way.
 */
export function formatErrors(errors: readonly ValidationError[]): string {
  return errors.map((e) => `${formatPath(e.path) || '(root)'}: ${e.msg}`).join('; ')
}
