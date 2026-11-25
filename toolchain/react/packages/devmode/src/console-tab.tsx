/**
 * The console tab: captured logs, errors and requests with level filters and search, plus a JS REPL that
 * evaluates in the page's global scope.
 */
import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent } from "react";
import { capture, type EntryType, type LogEntry, type NetRecord } from "./capture.ts";
import { usePersisted, Value } from "./ui.tsx";

/** Filter buttons and the entry types each one shows. */
const FILTERS: [string, string, EntryType[] | null][] = [
  ["all", "All", null],
  ["error", "Errors", ["error"]],
  ["warn", "Warnings", ["warn"]],
  ["info", "Info", ["info", "log", "debug", "input", "result"]],
  ["net", "Network", ["net"]],
];

/** Lines of REPL history kept. */
const HISTORY_MAX = 100;

/**
 * Evaluates REPL input as a global script and logs the input and its result (awaited if it is a promise).
 * Decision: indirect `eval`, so code runs in global scope like the browser console, not in this module.
 * Pages with a CSP that forbids `unsafe-eval` get the CSP error logged instead.
 * @param code source to run
 */
export async function evaluate(code: string): Promise<void> {
  capture.push("input", [code]);
  try {
    const result: unknown = (0, eval)(code);
    const value = result !== null && typeof result === "object" && typeof (result as PromiseLike<unknown>).then === "function" ? await result : result;
    (globalThis as { $_?: unknown }).$_ = value;
    capture.push("result", [value]);
  } catch (e) {
    capture.push("error", [e]);
  }
}

/** Text of an entry for the search box. */
function searchText(e: LogEntry): string {
  if (e.net) return `${e.net.method} ${e.net.url} ${e.net.status}`;
  return e.args.map((a) => (typeof a === "string" ? a : a instanceof Error ? a.message : (() => {
    try {
      return JSON.stringify(a) ?? String(a);
    } catch {
      return String(a);
    }
  })())).join(" ");
}

/** CSS class of a status code. */
export function statusClass(status: number): string {
  return status === 0 ? "lsdm-s-fail" : status < 300 ? "lsdm-s-ok" : status < 400 ? "lsdm-s-redir" : "lsdm-s-fail";
}

/** One rendered entry. */
function Row({ e, onOpenInApi }: { e: LogEntry; onOpenInApi: (n: NetRecord) => void }) {
  const time = new Date(e.time).toLocaleTimeString(undefined, { hour12: false });
  if (e.net) {
    const n = e.net;
    return (
      <div className="lsdm-row lsdm-t-net">
        <span className="lsdm-time">{time}</span>
        <span className="lsdm-method">{n.method}</span>
        <span className={`lsdm-status ${statusClass(n.status)}`}>{n.status || "ERR"}</span>
        <span className="lsdm-url" title={n.url}>{n.url}</span>
        <span className="lsdm-dim">{n.error ?? `${n.ms} ms`}</span>
        <button className="lsdm-link" title="Open this request in the API executor" onClick={() => onOpenInApi(n)}>→ API</button>
      </div>
    );
  }
  const mark = e.type === "input" ? "›" : e.type === "result" ? "‹" : "";
  return (
    <div className={`lsdm-row lsdm-t-${e.type}`}>
      <span className="lsdm-time">{time}</span>
      {mark && <span className="lsdm-mark">{mark}</span>}
      <span className="lsdm-args">
        {e.type === "input" ? <span className="lsdm-code">{String(e.args[0])}</span> : e.args.map((a, i) => <Value key={i} v={a} top={e.type !== "result"} />)}
      </span>
    </div>
  );
}

/**
 * The console tab.
 * @param props.onOpenInApi called with a request row's details when "→ API" is clicked
 */
export function ConsoleTab({ onOpenInApi }: { onOpenInApi: (n: NetRecord) => void }) {
  const entries = useSyncExternalStore(capture.subscribe, capture.snapshot);
  const [filter, setFilter] = usePersisted("console.filter", "all");
  const [search, setSearch] = useState("");
  const [input, setInput] = useState("");
  const [history, setHistory] = usePersisted<string[]>("console.history", []);
  const [cursor, setCursor] = useState(-1);
  const list = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  const shown = useMemo(() => {
    const types = FILTERS.find(([id]) => id === filter)?.[2];
    const needle = search.toLowerCase();
    return entries.filter((e) => (!types || types.includes(e.type)) && (!needle || searchText(e).toLowerCase().includes(needle)));
  }, [entries, filter, search]);

  const counts = useMemo(() => {
    const c: Record<string, number> = { error: 0, warn: 0, net: 0 };
    for (const e of entries) if (e.type in c) c[e.type]!++;
    return c;
  }, [entries]);

  // Keep the newest line in view unless the user scrolled up to read.
  useLayoutEffect(() => {
    const el = list.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [shown]);
  useEffect(() => setCursor(-1), [history]);

  const run = () => {
    const code = input.trim();
    if (!code) return;
    setHistory((h) => [...h.filter((x) => x !== code), code].slice(-HISTORY_MAX));
    setInput("");
    stick.current = true;
    void evaluate(code);
  };

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    const el = e.currentTarget;
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      run();
    } else if (e.key === "ArrowUp" && el.selectionStart === 0 && history.length) {
      e.preventDefault();
      const next = cursor < 0 ? history.length - 1 : Math.max(0, cursor - 1);
      setCursor(next);
      setInput(history[next]!);
    } else if (e.key === "ArrowDown" && el.selectionEnd === el.value.length && cursor >= 0) {
      e.preventDefault();
      const next = cursor + 1;
      setCursor(next >= history.length ? -1 : next);
      setInput(next >= history.length ? "" : history[next]!);
    } else if (e.key === "l" && e.ctrlKey) {
      e.preventDefault();
      capture.clear();
    }
  };

  return (
    <div className="lsdm-console">
      <div className="lsdm-toolbar">
        {FILTERS.map(([id, label]) => (
          <button key={id} className={filter === id ? "lsdm-chip lsdm-on" : "lsdm-chip"} onClick={() => setFilter(id)}>
            {label}{counts[id] ? <span className={`lsdm-count lsdm-c-${id}`}>{counts[id]}</span> : null}
          </button>
        ))}
        <input className="lsdm-search" placeholder="Filter" value={search} onChange={(e) => setSearch(e.target.value)} />
        <button className="lsdm-btn" title="Clear (Ctrl+L)" onClick={() => capture.clear()}>Clear</button>
      </div>
      <div
        className="lsdm-list"
        ref={list}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
      >
        {shown.length ? shown.map((e) => <Row key={e.id} e={e} onOpenInApi={onOpenInApi} />) : <div className="lsdm-empty">No entries</div>}
      </div>
      <div className="lsdm-repl">
        <span className="lsdm-prompt">›</span>
        <textarea
          rows={Math.min(8, input.split("\n").length)}
          placeholder="Evaluate JavaScript (Enter runs, Shift+Enter for a new line, ↑ history, $_ is the last result)"
          value={input}
          spellCheck={false}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={onKey}
        />
      </div>
    </div>
  );
}
