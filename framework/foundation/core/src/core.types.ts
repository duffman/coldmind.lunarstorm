/**
 * @module core.types
 * type-fest re-exports every layer uses: `Simplify`, `UnknownRecord`, `Tagged`, …
 *
 * ---
 * Type utilities every layer uses. type-fest is a type-only dependency: none
 * of this survives compilation.
 */

export type {
  Exact,
  JsonValue,
  Merge,
  Promisable,
  ReadonlyDeep,
  Simplify,
  Tagged,
  UnknownRecord,
} from 'type-fest'
