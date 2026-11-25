/**
 * The API tab: a redis-cli style command executor for the app's Redis-protocol API.
 * Connection bar (endpoint, protocol, status), command editor (Enter runs, several lines run as one pipeline),
 * a transcript of commands and typed replies, a pub/sub push feed, and on the side the API TreeView (calls grouped
 * into categories) plus history.
 */
import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent } from "react";
import { ApiTree, parseApiSpec, parseCommandDocs, savedCategories, type ApiCall, type ApiCategory, type SavedCall } from "./api-tree.tsx";
import { RedisClient } from "./redis-client.ts";
import { formatReply, parseCommands, quote, type Line, type Reply } from "./resp.ts";
import { TabBar, usePersisted } from "./ui.tsx";

/** One run of the editor: a single command or a pipeline. */
interface Run {
  /** Unique id. */
  id: number;
  /** Commands sent. */
  cmds: string[][];
  /** One reply per command; empty when `failure` is set. */
  replies: Reply[];
  /** Connection or parse failure; nothing was answered. */
  failure?: string;
  /** Round trip in ms for the whole pipeline. */
  ms: number;
  /** Wall-clock time sent. */
  at: string;
}

/** One received push frame. */
interface Push {
  /** Unique id. */
  id: number;
  /** The frame. */
  reply: Reply;
  /** Wall-clock time received. */
  at: string;
}

/** History and push feed caps; the oldest entries drop first. */
const MAX_HISTORY = 100;
const MAX_PUSHES = 500;

let nextId = 1;
/** Current time as HH:MM:SS.mmm. */
const now = () => new Date().toISOString().slice(11, 23);

/**
 * Prints a command back as typed: arguments with spaces, quotes or control characters are quoted.
 * @param args the command
 */
export function commandText(args: string[]): string {
  return args.map((a) => (a === "" || /[\s"'\\\x00-\x1f]/.test(a) ? quote(a) : a)).join(" ");
}

/** Renders formatted reply lines with one class per segment style. */
function ReplyLines({ lines }: { lines: Line[] }) {
  return (
    <div className="lsdm-reply">
      {lines.map((l, i) => <div key={i}>{l.map((s, j) => <span key={j} className={`lsdm-r-${s.c}`}>{s.s}</span>)}</div>)}
    </div>
  );
}

/** Saved calls a first run starts with, so the Saved category shows how filing works. */
const STARTER: SavedCall[] = [
  { category: "Saved", command: "PING" },
  { category: "Saved", command: "SUBSCRIBE news" },
  { category: "Saved", command: "SET greeting hello\nINCR counter\nHGETALL user:1" },
];

/**
 * The tab.
 * @param props.defaultUrl endpoint used until the user edits it
 * @param props.active true while the tab is on screen; connecting (and loading the API tree) starts then
 * @param props.api the host's API spec (`HostApi::spec`); when given the tree lists it instead of COMMAND DOCS
 */
export function RedisTab({ defaultUrl, active, api }: { defaultUrl: string; active: boolean; api?: unknown }) {
  const [url, setUrl] = usePersisted("redis.url", defaultUrl);
  const [source, setSource] = usePersisted("redis.source", "");
  const [history, setHistory] = usePersisted<string[]>("redis.history", []);
  const [collection, setCollection] = usePersisted<SavedCall[]>("redis.collection", STARTER);
  const [saveTo, setSaveTo] = usePersisted("redis.saveTo", "Saved");
  const [side, setSide] = usePersisted<"api" | "history">("redis.side", "api");
  const [docs, setDocs] = useState<ApiCategory[]>([]);
  const [docsNote, setDocsNote] = useState("");
  const [out, setOut] = useState<"output" | "pushes">("output");
  const [runs, setRuns] = useState<Run[]>([]);
  const [pushes, setPushes] = useState<Push[]>([]);
  const [busy, setBusy] = useState(false);
  const [client, setClient] = useState(() => new RedisClient(url));
  const state = useSyncExternalStore(client.subscribe, () => client.state);
  const histPos = useRef(-1);
  const body = useRef<HTMLDivElement>(null);

  useEffect(() => client.onPush((reply) => setPushes((p) => [...p, { id: nextId++, reply, at: now() }].slice(-MAX_PUSHES))), [client]);
  // Fix: closing in the unmount cleanup killed the connect-on-show socket under StrictMode, whose dev remount runs the
  // cleanup and the effect again at once; the client then sat in "error" ("Unreachable"). Decision: defer the close one
  // tick and let a remount with the same client cancel it; a real unmount or a client swap still closes.
  const pendingClose = useRef<{ client: RedisClient; timer: ReturnType<typeof setTimeout> } | null>(null);
  useEffect(() => {
    if (pendingClose.current?.client === client) clearTimeout(pendingClose.current.timer);
    return () => {
      pendingClose.current = { client, timer: setTimeout(() => client.close()) };
    };
  }, [client]);

  // Connect when the tab is first shown, so the API tree fills without a click.
  useEffect(() => {
    if (active && client.state === "idle") void client.connect().catch(() => {});
  }, [active, client]);

  // Decision: a HostApi spec, when given, is the API tree's only source, so the API is described in one place
  // (`HostApi::spec`); COMMAND DOCS is asked only when there is none. Parsed once per spec object.
  const specCategories = useMemo(() => (api === undefined ? null : parseApiSpec(api)), [api]);

  // Load the API tree from the server's own description each time a connection opens.
  useEffect(() => {
    if (specCategories) {
      setDocs(specCategories);
      setDocsNote(specCategories.length ? "" : "The host's API spec lists no areas.");
      return;
    }
    if (state !== "open") {
      setDocsNote(state === "connecting" ? "Loading API…" : "Connect to load the server's API.");
      return;
    }
    let live = true;
    setDocsNote("Loading API…");
    client.send([["COMMAND", "DOCS"]]).then(([r]) => {
      if (!live || !r) return;
      const cats = r.t === "error" ? [] : parseCommandDocs(r);
      setDocs(cats);
      setDocsNote(r.t === "error" ? `This endpoint does not describe its API (COMMAND DOCS: ${r.v}).` : cats.length ? "" : "The endpoint lists no API calls.");
    }, (e: Error) => live && setDocsNote(e.message));
    return () => {
      live = false;
    };
  }, [client, state, specCategories]);
  useEffect(() => {
    body.current?.scrollTo({ top: body.current.scrollHeight });
  }, [runs, pushes, out]);

  /** The client for the current URL; an edited URL replaces the old connection. */
  const current = () => {
    if (client.url === url.trim()) return client;
    client.close();
    const c = new RedisClient(url.trim());
    setClient(c);
    setDocs([]);
    return c;
  };

  /**
   * Runs the editor text, or `override` (a call double-clicked in the tree).
   * @param override text to run instead of the editor's
   */
  const run = async (override?: string) => {
    const text = (override ?? source).trim();
    if (!text || busy) return;
    let cmds: string[][];
    try {
      cmds = parseCommands(text);
    } catch (e) {
      setRuns((r) => [...r, { id: nextId++, cmds: [], replies: [], failure: (e as Error).message, ms: 0, at: now() }]);
      return;
    }
    if (!cmds.length) return;
    setHistory((h) => [text, ...h.filter((x) => x !== text)].slice(0, MAX_HISTORY));
    histPos.current = -1;
    if (override === undefined) setSource("");
    setOut("output");
    setBusy(true);
    const t0 = performance.now();
    try {
      const replies = await current().send(cmds);
      setRuns((r) => [...r, { id: nextId++, cmds, replies, ms: performance.now() - t0, at: now() }]);
    } catch (e) {
      setRuns((r) => [...r, { id: nextId++, cmds, replies: [], failure: (e as Error).message, ms: performance.now() - t0, at: now() }]);
    } finally {
      setBusy(false);
    }
  };

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void run();
      return;
    }
    // History recall only while the editor holds a single line, so arrows still move between lines of a pipeline.
    if ((e.key === "ArrowUp" || e.key === "ArrowDown") && !source.includes("\n") && history.length) {
      e.preventDefault();
      const pos = Math.max(-1, Math.min(history.length - 1, histPos.current + (e.key === "ArrowUp" ? 1 : -1)));
      histPos.current = pos;
      setSource(pos < 0 ? "" : history[pos]!);
    }
  };

  const lines = source.split("\n").length;
  const count = (() => {
    try {
      return parseCommands(source).length;
    } catch {
      return 0;
    }
  })();
  const info = state === "open" ? client.info : null;
  const stateText = { idle: "Not connected", connecting: "Connecting…", open: "Connected", closed: "Disconnected", error: "Unreachable" }[state];

  return (
    <div className="lsdm-api lsdm-redis">
      <aside className="lsdm-side">
        <TabBar tabs={[["api", "API"], ["history", "History"]]} value={side === "history" ? "history" : "api"} onChange={setSide} />
        <div className="lsdm-side-body">
          {side !== "history" && (
            <>
              <div className="lsdm-save">
                <input list="lsdm-categories" value={saveTo} placeholder="Category" spellCheck={false} onChange={(e) => setSaveTo(e.target.value)} />
                <datalist id="lsdm-categories">{[...new Set(collection.map((c) => c.category))].map((c) => <option key={c} value={c} />)}</datalist>
                <button
                  className="lsdm-btn"
                  title="File the editor's command under this category"
                  disabled={!source.trim() || !saveTo.trim()}
                  onClick={() => {
                    const entry = { category: saveTo.trim(), command: source.trim() };
                    setCollection((all) => (all.some((x) => x.category === entry.category && x.command === entry.command) ? all : [...all, entry]));
                  }}
                >Save</button>
              </div>
              <ApiTree
                categories={[...savedCategories(collection), ...docs]}
                note={docsNote}
                onPick={(call: ApiCall) => setSource(call.command)}
                onRun={(call: ApiCall) => void run(call.command)}
                onRemove={(category, command) => setCollection((all) => all.filter((x) => !(x.category === category && x.command === command)))}
              />
            </>
          )}
          {side === "history" && (history.length === 0
            ? <div className="lsdm-empty">No history</div>
            : history.map((h) => (
              <div key={h} className="lsdm-item" title={h} onClick={() => setSource(h)}>
                <span className="lsdm-cmd-name">{h.split(/\s/)[0]!.toUpperCase()}</span>
                <span className="lsdm-item-name">{h.replace(/^\S+\s*/, "").split("\n")[0]}</span>
              </div>
            )))}
        </div>
      </aside>

      <section className="lsdm-main">
        <div className="lsdm-urlbar">
          <span className="lsdm-proto" title="Wire protocol">{info ? `RESP${info.proto}` : "RESP"}</span>
          <input className="lsdm-urlinput" value={url} spellCheck={false} placeholder="ws://host/resp" onChange={(e) => setUrl(e.target.value)} onKeyDown={(e) => e.key === "Enter" && void current().connect().catch(() => {})} />
          {info?.server && <span className="lsdm-dim">{info.server} {info.version}</span>}
          <span className={`lsdm-conn lsdm-st-${state}`} title={state === "error" ? client.lastError : url}>{stateText}</span>
          {state === "open"
            ? <button className="lsdm-btn" onClick={() => client.close()}>Disconnect</button>
            : <button className="lsdm-btn" disabled={state === "connecting"} onClick={() => void current().connect().catch(() => {})}>Connect</button>}
        </div>

        <div className="lsdm-cmdbox">
          <span className="lsdm-prompt">›</span>
          <textarea
            value={source}
            rows={Math.min(8, lines)}
            spellCheck={false}
            placeholder="Redis command · Enter runs · Shift+Enter adds a line (lines run as one pipeline) · ↑↓ history"
            onChange={(e) => setSource(e.target.value)}
            onKeyDown={onKey}
          />
          <button className="lsdm-send" disabled={busy || count === 0} onClick={() => void run()}>{count > 1 ? `Pipeline ${count}` : "Run"}</button>
        </div>

        <div className="lsdm-res">
          <TabBar
            tabs={[["output", "Output"], ["pushes", <>Pushes{pushes.length > 0 && <span className="lsdm-count lsdm-c-push">{pushes.length}</span>}</>]]}
            value={out}
            onChange={setOut}
            right={<button className="lsdm-btn" onClick={() => (out === "output" ? setRuns([]) : setPushes([]))}>Clear</button>}
          />
          <div className="lsdm-res-body" ref={body}>
            {out === "output" && (runs.length === 0
              ? <div className="lsdm-empty">Run a command. Try <code>PING</code>, <code>KEYS *</code>, <code>HGETALL user:1</code> or <code>SUBSCRIBE news</code> then <code>PUBLISH news hi</code>.</div>
              : runs.map((r) => (
                <div key={r.id} className={`lsdm-run${r.failure ? " lsdm-run-err" : ""}`}>
                  <div className="lsdm-run-meta"><span>{r.at}</span>{r.cmds.length > 1 && <span className="lsdm-chip-sm">pipeline · {r.cmds.length}</span>}<span>{r.ms.toFixed(1)} ms</span></div>
                  {r.failure && <div className="lsdm-r-err">{r.failure}</div>}
                  {r.cmds.map((c, i) => (
                    <div key={i} className={`lsdm-exchange${r.replies[i]?.t === "error" ? " lsdm-exchange-err" : ""}`}>
                      <div className="lsdm-cmdline"><span className="lsdm-prompt">›</span> {commandText(c)}</div>
                      {r.replies[i] && <ReplyLines lines={formatReply(r.replies[i])} />}
                    </div>
                  ))}
                </div>
              )))}
            {out === "pushes" && (pushes.length === 0
              ? <div className="lsdm-empty">Pub/sub messages and other server pushes appear here.</div>
              : pushes.map((p) => (
                <div key={p.id} className="lsdm-run lsdm-run-push">
                  <div className="lsdm-run-meta"><span>{p.at}</span></div>
                  <ReplyLines lines={formatReply(p.reply)} />
                </div>
              )))}
          </div>
        </div>
      </section>
    </div>
  );
}
