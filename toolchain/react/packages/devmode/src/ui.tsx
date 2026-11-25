/**
 * Small pieces both tabs use: persisted state, the key/value table, and the lazy value inspector.
 */
import { useEffect, useState, type ReactNode } from "react";
import type { Pair } from "./request.ts";

/** Prefix of every localStorage key the dev mode writes. */
export const STORAGE_PREFIX = "lunarstorm.devmode.";

/**
 * `useState` mirrored to localStorage as JSON. Unreadable or missing values fall back to `initial`.
 * @param key key under `STORAGE_PREFIX`
 * @param initial value when nothing is stored
 * @return state and setter
 */
export function usePersisted<T>(key: string, initial: T | (() => T)): [T, (v: T | ((prev: T) => T)) => void] {
  const [value, setValue] = useState<T>(() => {
    try {
      const raw = globalThis.localStorage?.getItem(STORAGE_PREFIX + key);
      if (raw !== null && raw !== undefined) return JSON.parse(raw) as T;
    } catch {
      // corrupt or unavailable storage: start from the default
    }
    return typeof initial === "function" ? (initial as () => T)() : initial;
  });
  useEffect(() => {
    try {
      globalThis.localStorage?.setItem(STORAGE_PREFIX + key, JSON.stringify(value));
    } catch {
      // quota exceeded or storage disabled: the state still works for this session
    }
  }, [key, value]);
  return [value, setValue];
}

/**
 * Editable key/value rows with an enable checkbox. A blank row is always shown last; typing in it adds a row.
 * @param props.pairs the rows
 * @param props.onChange receives the new rows
 * @param props.keyLabel placeholder of the key column
 */
export function PairTable({ pairs, onChange, keyLabel = "Key" }: { pairs: Pair[]; onChange: (p: Pair[]) => void; keyLabel?: string }) {
  const rows = [...pairs, { key: "", value: "", enabled: true }];
  const set = (i: number, patch: Partial<Pair>) => {
    const next = rows.map((r, j) => (j === i ? { ...r, ...patch } : r));
    onChange(next.filter((r, j) => j < pairs.length || r.key || r.value));
  };
  return (
    <table className="lsdm-pairs">
      <tbody>
        {rows.map((p, i) => (
          <tr key={i} className={p.enabled ? "" : "lsdm-off"}>
            <td className="lsdm-check">
              {i < pairs.length && <input type="checkbox" checked={p.enabled} onChange={(e) => set(i, { enabled: e.target.checked })} />}
            </td>
            <td><input placeholder={keyLabel} value={p.key} onChange={(e) => set(i, { key: e.target.value })} /></td>
            <td><input placeholder="Value" value={p.value} onChange={(e) => set(i, { value: e.target.value })} /></td>
            <td className="lsdm-check">
              {i < pairs.length && <button className="lsdm-x" title="Remove" onClick={() => onChange(pairs.filter((_, j) => j !== i))}>×</button>}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/**
 * A row of tab buttons.
 * @param props.tabs `[id, label]` pairs
 * @param props.value selected id
 * @param props.onChange receives the clicked id
 */
export function TabBar<T extends string>({ tabs, value, onChange, right }: { tabs: [T, ReactNode][]; value: T; onChange: (t: T) => void; right?: ReactNode }) {
  return (
    <div className="lsdm-tabbar">
      {tabs.map(([id, label]) => (
        <button key={id} className={id === value ? "lsdm-tab lsdm-on" : "lsdm-tab"} onClick={() => onChange(id)}>{label}</button>
      ))}
      <span className="lsdm-grow" />
      {right}
    </div>
  );
}

/** Constructor name for object previews, or `""` for plain objects. */
function ctorName(v: object): string {
  const name = (Object.getPrototypeOf(v) as { constructor?: { name?: string } } | null)?.constructor?.name;
  return name && name !== "Object" ? name : "";
}

/** Reads a property without letting a throwing getter break the panel. */
function read(v: object, k: PropertyKey): unknown {
  try {
    return (v as Record<PropertyKey, unknown>)[k];
  } catch (e) {
    return e;
  }
}

/** One-line text for a value inside a preview. */
function short(v: unknown): string {
  if (typeof v === "string") return JSON.stringify(v.length > 40 ? `${v.slice(0, 40)}…` : v);
  if (typeof v === "function") return "ƒ";
  if (Array.isArray(v)) return `Array(${v.length})`;
  if (v && typeof v === "object") return ctorName(v) || "{…}";
  return String(v);
}

/** One-line preview of an object or array, like the browser console's collapsed form. */
function preview(v: object): string {
  if (Array.isArray(v)) {
    const items = v.slice(0, 6).map(short);
    return `(${v.length}) [${items.join(", ")}${v.length > 6 ? ", …" : ""}]`;
  }
  if (v instanceof Map) return `Map(${v.size})`;
  if (v instanceof Set) return `Set(${v.size})`;
  const keys = Object.keys(v);
  const items = keys.slice(0, 5).map((k) => `${k}: ${short(read(v, k))}`);
  return `${ctorName(v)} {${items.join(", ")}${keys.length > 5 ? ", …" : ""}}`.trim();
}

/** Children of an expanded object as `[label, value]`. */
function children(v: object): [string, unknown][] {
  if (v instanceof Map) return [...v.entries()].map(([k, val], i) => [`${i}: ${short(k)} =>`, val]);
  if (v instanceof Set) return [...v.values()].map((val, i) => [String(i), val]);
  const keys: string[] = Array.isArray(v) ? [...v.keys()].map(String) : Object.getOwnPropertyNames(v);
  return keys.map((k) => [k, read(v, k)]);
}

/**
 * Renders any value the way a browser console does. Objects expand on click; children render only when open,
 * so circular and huge structures cost nothing until explored.
 * @param props.v the value
 * @param props.top true for a direct console argument: strings print without quotes
 * @param props.expanded start open (the response viewer opens the root)
 */
export function Value({ v, top = false, expanded = false }: { v: unknown; top?: boolean; expanded?: boolean }) {
  const [open, setOpen] = useState(expanded);
  if (typeof v === "string") return top ? <span className="lsdm-str-top">{v}</span> : <span className="lsdm-str">{JSON.stringify(v)}</span>;
  if (typeof v === "number" || typeof v === "bigint" || typeof v === "boolean") return <span className="lsdm-num">{String(v)}{typeof v === "bigint" ? "n" : ""}</span>;
  if (v === null || v === undefined) return <span className="lsdm-nil">{String(v)}</span>;
  if (typeof v === "symbol") return <span className="lsdm-num">{v.toString()}</span>;
  if (typeof v === "function") return <span className="lsdm-fn">ƒ {v.name || "anonymous"}()</span>;
  if (v instanceof Error) return <span className="lsdm-err-val">{v.stack || `${v.name}: ${v.message}`}</span>;
  if (typeof Node !== "undefined" && v instanceof Element) {
    const cls = typeof v.className === "string" && v.className ? `.${v.className.trim().split(/\s+/).join(".")}` : "";
    return <span className="lsdm-fn">&lt;{v.tagName.toLowerCase()}{v.id ? `#${v.id}` : ""}{cls}&gt;</span>;
  }
  const obj = v as object;
  return (
    <span className="lsdm-obj">
      <span className="lsdm-toggle" onClick={() => setOpen(!open)}>{open ? "▾" : "▸"} {preview(obj)}</span>
      {open && (
        <span className="lsdm-children">
          {children(obj).map(([k, child]) => (
            <span key={k} className="lsdm-child"><span className="lsdm-key">{k}</span>: <Value v={child} /></span>
          ))}
        </span>
      )}
    </span>
  );
}
