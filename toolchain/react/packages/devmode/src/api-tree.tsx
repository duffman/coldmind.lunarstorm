/**
 * The API TreeView: API calls grouped into categories.
 * Server categories come from the server describing itself with `COMMAND DOCS` (each command's `group`, `summary` and
 * `arguments`), so the tree always shows the API the connected endpoint really offers. The user's saved calls form
 * their own categories, listed first.
 */
import { useMemo, useState, type KeyboardEvent } from "react";
import type { Reply } from "./resp.ts";
import { usePersisted } from "./ui.tsx";

/** One API call in the tree. */
export interface ApiCall {
  /** Command name, upper case (`HGETALL`), or the first word of a saved call. */
  name: string;
  /** Editor text the call loads: a template with argument names, or the saved command lines. */
  command: string;
  /** Argument synopsis, e.g. `key field value [field value ...]`. */
  args: string;
  /** One-line description. */
  summary: string;
}

/** A category of calls. */
export interface ApiCategory {
  /** Unique key: `s:<group>` for server groups, `u:<name>` for saved categories. */
  id: string;
  /** Display name. */
  label: string;
  /** True for the user's own category (calls can be removed). */
  saved: boolean;
  /** Calls, sorted by name for server groups, in saving order for saved ones. */
  calls: ApiCall[];
}

/** A call the user saved: editor text filed under a category. */
export interface SavedCall {
  /** Category name. */
  category: string;
  /** Editor text; several lines are a pipeline. */
  command: string;
}

/** Display names of the standard Redis command groups; other groups (the app's own) are shown capitalised. */
const GROUP_LABELS: Record<string, string> = {
  generic: "Keys", string: "Strings", hash: "Hashes", list: "Lists", set: "Sets", "sorted-set": "Sorted sets",
  pubsub: "Pub/Sub", connection: "Connection", server: "Server", scripting: "Scripting", stream: "Streams",
  transactions: "Transactions", geo: "Geo", hyperloglog: "HyperLogLog", bitmap: "Bitmaps", cluster: "Cluster",
};

/** Text of a string reply. */
function str(r: Reply | undefined): string {
  return r && (r.t === "bulk" || r.t === "simple" || r.t === "verbatim") ? r.v : "";
}

/** Key/value pairs of a map reply, or of a RESP2 flat array of alternating keys and values. */
function pairs(r: Reply | undefined): [Reply, Reply][] {
  if (!r) return [];
  if (r.t === "map") return r.v;
  if (r.t !== "array") return [];
  const out: [Reply, Reply][] = [];
  for (let i = 0; i + 1 < r.v.length; i += 2) out.push([r.v[i]!, r.v[i + 1]!]);
  return out;
}

/** Field of a map-like reply by name. */
function field(r: Reply | undefined, name: string): Reply | undefined {
  return pairs(r).find(([k]) => str(k) === name)?.[1];
}

/**
 * Formats one `COMMAND DOCS` argument the way redis-cli help does: `key`, `[key]`, `key [key ...]`; blocks and
 * one-of groups list their sub-arguments.
 * @param a the argument's doc map
 * @return synopsis text
 */
function formatArg(a: Reply): string {
  const flags = field(a, "flags");
  const has = (f: string) => !!flags && (flags.t === "array" || flags.t === "set") && flags.v.some((x) => str(x) === f);
  const subs = field(a, "arguments");
  const kids = subs && (subs.t === "array" || subs.t === "set") ? subs.v.map(formatArg) : [];
  const type = str(field(a, "type"));
  const token = str(field(a, "token"));
  let text = kids.length ? kids.join(type === "oneof" ? " | " : " ") : str(field(a, "name"));
  if (token) text = type === "pure-token" ? token : `${token} ${text}`;
  if (has("multiple")) text = `${text} [${text} ...]`;
  return has("optional") ? `[${text}]` : text;
}

/**
 * Turns a `COMMAND DOCS` reply (RESP3 map or RESP2 flat array) into server categories.
 * The call template holds the command and its required argument names, ready to be overwritten in the editor.
 * @param r the reply
 * @return categories sorted by label, calls sorted by name; empty for an error or unexpected reply
 */
export function parseCommandDocs(r: Reply): ApiCategory[] {
  const groups = new Map<string, ApiCall[]>();
  for (const [nameReply, doc] of pairs(r)) {
    const name = str(nameReply).toUpperCase();
    if (!name) continue;
    const argsReply = field(doc, "arguments");
    const argDocs = argsReply && (argsReply.t === "array" || argsReply.t === "set") ? argsReply.v : [];
    const parts = argDocs.map(formatArg);
    const required = parts.filter((p) => !p.startsWith("["));
    const group = str(field(doc, "group")) || "other";
    const call: ApiCall = { name, command: [name, ...required.map((p) => p.replace(/ \[.*$/, ""))].join(" "), args: parts.join(" "), summary: str(field(doc, "summary")) };
    groups.set(group, [...(groups.get(group) ?? []), call]);
  }
  return [...groups].map(([g, calls]) => ({
    id: `s:${g}`,
    label: GROUP_LABELS[g] ?? g.charAt(0).toUpperCase() + g.slice(1),
    saved: false,
    calls: calls.sort((a, b) => a.name.localeCompare(b.name)),
  })).sort((a, b) => a.label.localeCompare(b.label));
}

/** The HostApi spec (`HostApi::spec` in extensions/hostapi): the host's whole API, described once. */
export interface ApiSpec {
  /** Format version; only 1 is read. */
  version: number;
  /** Every area the host offers. */
  areas: { name: string; module: string; functions: { name: string; summary: string; args: { name: string; optional: boolean; multiple: boolean }[] }[] }[];
}

/**
 * Turns a HostApi spec into one category per area.
 * @param spec the spec, as received (untrusted shape)
 * @return categories sorted by label, calls sorted by name; empty for anything but a version 1 spec
 */
export function parseApiSpec(spec: unknown): ApiCategory[] {
  const s = spec as ApiSpec | null;
  // Decision: a spec of another version is ignored rather than half-read, so a newer host cannot show a wrong tree.
  if (!s || s.version !== 1 || !Array.isArray(s.areas)) return [];
  return s.areas.map((area) => ({
    id: `a:${area.name}`,
    label: area.name.charAt(0).toUpperCase() + area.name.slice(1),
    saved: false,
    calls: (area.functions ?? []).map((f) => {
      // Same synopsis rules as COMMAND DOCS (`formatArg`): `x [x ...]` repeats, `[x]` may be left out.
      const parts = (f.args ?? []).map((a) => {
        const text = a.multiple ? `${a.name} [${a.name} ...]` : a.name;
        return a.optional ? `[${text}]` : text;
      });
      // Decision: a call is `AREA.FUNCTION`, Redis-style upper case, qualified by its area so two areas cannot clash.
      const name = `${area.name}.${f.name}`.toUpperCase();
      const required = (f.args ?? []).filter((a) => !a.optional).map((a) => a.name);
      return { name, command: [name, ...required].join(" "), args: parts.join(" "), summary: f.summary ?? "" };
    }).sort((a, b) => a.name.localeCompare(b.name)),
  })).sort((a, b) => a.label.localeCompare(b.label));
}

/**
 * Groups saved calls into categories, in first-saved order.
 * @param saved the user's calls
 * @return saved categories
 */
export function savedCategories(saved: SavedCall[]): ApiCategory[] {
  const out = new Map<string, ApiCategory>();
  for (const s of saved) {
    const cat = out.get(s.category) ?? { id: `u:${s.category}`, label: s.category, saved: true, calls: [] };
    const first = s.command.split("\n")[0]!.trim();
    const lines = s.command.split("\n").filter((l) => l.trim()).length;
    cat.calls.push({ name: first.split(/\s+/)[0]!.toUpperCase(), command: s.command, args: first.replace(/^\S+\s*/, ""), summary: lines > 1 ? `Pipeline of ${lines} commands` : "" });
    out.set(s.category, cat);
  }
  return [...out.values()];
}

/** Props of `ApiTree`. */
export interface ApiTreeProps {
  /** Categories to show, saved ones first. */
  categories: ApiCategory[];
  /** Status line under the server categories (loading, unsupported, not connected); empty for none. */
  note: string;
  /** Loads a call into the editor. */
  onPick: (call: ApiCall) => void;
  /** Loads a call and runs it. */
  onRun: (call: ApiCall) => void;
  /** Removes a saved call. */
  onRemove: (category: string, command: string) => void;
}

/**
 * The tree: collapsible categories with a count, calls as `NAME args` rows; a filter on top matches name, arguments and
 * summary and opens every category with a match. Arrow keys move between rows, Right/Left open/close, Enter picks.
 */
export function ApiTree({ categories, note, onPick, onRun, onRemove }: ApiTreeProps) {
  // Decision: only the user's own "Saved" category starts open; server APIs can list hundreds of calls.
  const [open, setOpen] = usePersisted<string[]>("redis.tree.open", ["u:Saved"]);
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState("");
  const q = filter.trim().toLowerCase();

  const shown = useMemo(() => categories
    .map((c) => ({ ...c, calls: q ? c.calls.filter((k) => `${k.name} ${k.args} ${k.summary} ${c.label}`.toLowerCase().includes(q)) : c.calls }))
    .filter((c) => !q || c.calls.length), [categories, q]);
  const isOpen = (id: string) => !!q || open.includes(id);
  const toggle = (id: string, to = !open.includes(id)) => setOpen((o) => (to ? [...o.filter((x) => x !== id), id] : o.filter((x) => x !== id)));

  /** Visible rows in order, for keyboard navigation: category ids and `<category>\n<command>` call keys. */
  const rows = shown.flatMap((c) => [c.id, ...(isOpen(c.id) ? c.calls.map((k) => `${c.id}\n${k.command}`) : [])]);
  const callOf = (key: string) => {
    const [cid, ...rest] = key.split("\n");
    return shown.find((c) => c.id === cid)?.calls.find((k) => k.command === rest.join("\n"));
  };
  const onKey = (e: KeyboardEvent) => {
    const i = rows.indexOf(selected);
    const isCat = !selected.includes("\n");
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setSelected(rows[Math.max(0, Math.min(rows.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))] ?? "");
    } else if (e.key === "ArrowRight" && isCat) toggle(selected, true);
    else if (e.key === "ArrowLeft") {
      if (isCat) toggle(selected, false);
      else setSelected(selected.split("\n")[0]!);
    } else if (e.key === "Enter") {
      const call = callOf(selected);
      if (call) (e.shiftKey ? onRun : onPick)(call);
      else if (isCat && selected) toggle(selected);
    }
  };

  return (
    <div className="lsdm-tree-wrap">
      <input className="lsdm-tree-filter" placeholder="Filter API calls" value={filter} spellCheck={false} onChange={(e) => setFilter(e.target.value)} />
      <div className="lsdm-tree" role="tree" aria-label="API calls" tabIndex={0} onKeyDown={onKey}>
        {shown.map((c) => (
          <div key={c.id} role="treeitem" aria-expanded={isOpen(c.id)} className={`lsdm-cat${c.saved ? " lsdm-cat-saved" : ""}`}>
            <div className={`lsdm-cat-row${selected === c.id ? " lsdm-on" : ""}`} onClick={() => { setSelected(c.id); toggle(c.id); }}>
              <span className={`lsdm-chev${isOpen(c.id) ? " lsdm-chev-open" : ""}`} />
              <span className="lsdm-cat-label">{c.label}</span>
              <span className="lsdm-cat-count">{c.calls.length}</span>
            </div>
            {isOpen(c.id) && (
              <div role="group">
                {c.calls.map((k) => {
                  const key = `${c.id}\n${k.command}`;
                  return (
                    <div
                      key={key}
                      role="treeitem"
                      className={`lsdm-call${selected === key ? " lsdm-on" : ""}`}
                      title={[k.summary, `${k.name} ${k.args}`.trim()].filter(Boolean).join("\n")}
                      onClick={() => { setSelected(key); onPick(k); }}
                      onDoubleClick={() => onRun(k)}
                    >
                      <span className="lsdm-cmd-name">{k.name}</span>
                      <span className="lsdm-item-name">{k.args}</span>
                      {c.saved && <button className="lsdm-x" title="Remove" onClick={(e) => { e.stopPropagation(); onRemove(c.label, k.command); }}>×</button>}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        ))}
        {note && <div className="lsdm-tree-note">{note}</div>}
        {q && shown.length === 0 && <div className="lsdm-empty">No call matches “{filter}”</div>}
      </div>
    </div>
  );
}
