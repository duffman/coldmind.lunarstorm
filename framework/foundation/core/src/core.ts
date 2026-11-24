/**
 * @module @coldmind/core
 * What every coldmind package shares: validation results and errors, and the
 * type-fest helpers. Foundation layer: it depends on nothing of ours.
 *
 * Nothing here knows about fields, tables, SQL, HTTP, the kernel or a runtime.
 *
 * @example
 * import { formatErrors, type ValidationResult } from '@coldmind/core';
 * const r: ValidationResult<unknown> = check(Account, body);
 * if (!r.isValid) console.error(formatErrors(r.errors)); // "email: Invalid email"
 */

// ── validation results ──────────────────────────────────────
export {
  formatErrors,
  formatPath,
  type Path,
  type ValidationError,
  type ValidationLevel,
  type ValidationResult,
} from '@coldmind/core/validation';

// ── type helpers (type-fest) ────────────────────────────────
export type {
  Exact,
  JsonValue,
  Merge,
  Promisable,
  ReadonlyDeep,
  Simplify,
  Tagged,
  UnknownRecord,
} from '@coldmind/core/type-utils';
