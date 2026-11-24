/**
 * @module routes.coerce — turns the strings a URL or header carries into the values a field expects, so a
 * `num()` query parameter can be declared like any other field.
 */
import { Field, introspectField, isModel, toShape, type AnyModel, type FieldInfo, type Shape } from "@coldmind/model";

/**
 * Raw strings from a URL or headers: one value, or several for a repeated name.
 *
 * URL query or header values as they arrive: strings, or lists for repeated names.
 *
 * @example
 * const raw: RawFields = { limit: "5", tag: ["a", "b"] };
 */
export type RawFields = Record<string, string | readonly string[]>;

function coerceOne(info: FieldInfo, value: string): unknown {
  if (info.kind === "number") {
    if (value.trim() === "") return value;
    const n = Number(value);
    return Number.isFinite(n) ? n : value;
  }
  if (info.kind === "boolean") return value === "true" || value === "1" ? true : value === "false" || value === "0" ? false : value;
  return value;
}

/**
 * Coerces raw string values by the kind of the field that will receive them: `number` from a numeric string,
 * `boolean` from `true`/`false`/`1`/`0`, `list` from one or several values (items coerced by the item
 * kind). Anything that does not convert is left as the string, so validation reports it. Keys without a
 * field are left untouched (and dropped by validation).
 *
 * @param schema the shape or model of the slot.
 * @param raw values by name.
 * @example
 * coerceFields({ limit: num(), tags: list(str()) }, { limit: "5", tags: "a" }); // { limit: 5, tags: ["a"] }
 */
export function coerceFields(schema: Shape | AnyModel, raw: RawFields): Record<string, unknown> {
  const shape = isModel(schema as AnyModel) ? toShape(schema as AnyModel) : (schema as Shape);
  const out: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(raw)) {
    const field = (shape as Record<string, unknown>)[name];
    if (!(field instanceof Field)) {
      out[name] = value;
      continue;
    }
    const info = introspectField(field);
    if (info.kind === "list") {
      const values = typeof value === "string" ? [value] : value;
      const item = info.item;
      out[name] = item ? values.map(v => coerceOne(item, v)) : [...values];
    } else out[name] = typeof value === "string" ? coerceOne(info, value) : value;
  }
  return out;
}
