/**
 * The Postman-style API executor: a request builder (method, URL, params, headers, body, auth), `{{var}}`
 * environment, saved collection, history, cURL import/export, and a response viewer. Everything but the
 * response persists in localStorage.
 */
import { useEffect, useMemo, useState } from "react";
import {
  buildRequest, envVars, execute, formatBytes, METHODS, newRequest, parseCurl, toCurl, uid,
  type ApiRequest, type ApiResponse, type BodyMode, type Method, type Pair,
} from "./request.ts";
import { statusClass } from "./console-tab.tsx";
import { PairTable, TabBar, usePersisted, Value } from "./ui.tsx";

/** A request handed in from outside (the console's "→ API"); `seq` changes on every hand-off so the same request loads twice. */
export interface ApiLoad {
  seq: number;
  req: ApiRequest;
}

/** One executed request in the history list. */
interface HistoryItem {
  id: string;
  time: number;
  status: number;
  ms: number;
  req: ApiRequest;
}

/** History entries kept. */
const HISTORY_MAX = 50;

/** `{{name}}` references that no environment variable resolved, in the built call. */
function unresolved(parts: (string | undefined)[]): string[] {
  const names = new Set<string>();
  for (const p of parts) for (const m of p?.matchAll(/\{\{\s*([\w.-]+)\s*\}\}/g) ?? []) names.add(m[1]!);
  return [...names];
}

/** The response pane. */
function ResponseView({ res, busy }: { res: ApiResponse | null; busy: boolean }) {
  const [tab, setTab] = useState<"body" | "headers">("body");
  const [pretty, setPretty] = useState(true);
  const json = useMemo(() => {
    if (!res?.body) return undefined;
    try {
      return { v: JSON.parse(res.body) as unknown };
    } catch {
      return undefined;
    }
  }, [res]);

  if (busy) return <div className="lsdm-res lsdm-empty">Sending…</div>;
  if (!res) return <div className="lsdm-res lsdm-empty">Send a request to see the response (Ctrl+Enter)</div>;
  if (res.error) return <div className="lsdm-res"><div className="lsdm-res-error">{res.error}</div></div>;
  const copy = () => void navigator.clipboard?.writeText(json && pretty ? JSON.stringify(json.v, null, 2) : res.body);
  return (
    <div className="lsdm-res">
      <TabBar
        tabs={[["body", "Body"], ["headers", `Headers (${res.headers.length})`]]}
        value={tab}
        onChange={setTab}
        right={
          <>
            <span className={`lsdm-status ${statusClass(res.status)}`}>{res.status} {res.statusText}</span>
            <span className="lsdm-dim">{res.ms} ms</span>
            <span className="lsdm-dim">{formatBytes(res.size)}</span>
            {json && <button className="lsdm-chip" onClick={() => setPretty(!pretty)}>{pretty ? "Raw" : "Pretty"}</button>}
            <button className="lsdm-chip" onClick={copy}>Copy</button>
          </>
        }
      />
      <div className="lsdm-res-body">
        {tab === "headers" ? (
          <table className="lsdm-kv"><tbody>{res.headers.map(([k, v]) => <tr key={k}><td>{k}</td><td>{v}</td></tr>)}</tbody></table>
        ) : json && pretty ? (
          <div className="lsdm-json"><Value v={json.v} expanded /></div>
        ) : (
          <pre className="lsdm-pre">{res.body || <span className="lsdm-dim">(empty body)</span>}</pre>
        )}
      </div>
    </div>
  );
}

/**
 * The API tab.
 * @param props.load a request to open, from the console
 * @param props.onLoaded called once `load` is applied; the owner clears it so a remount does not apply it again
 *   over the user's edits
 * @param props.defaultEnv environment used until the user edits their own
 */
export function ApiTab({ load, onLoaded, defaultEnv }: { load?: ApiLoad; onLoaded: () => void; defaultEnv: Pair[] }) {
  const [collection, setCollection] = usePersisted<ApiRequest[]>("api.collection", []);
  const [history, setHistory] = usePersisted<HistoryItem[]>("api.history", []);
  const [env, setEnv] = usePersisted<Pair[]>("api.env", defaultEnv);
  const [req, setReq] = usePersisted<ApiRequest>("api.current", () => newRequest());
  const [side, setSide] = usePersisted<"collection" | "history" | "env">("api.side", "collection");
  const [tab, setTab] = usePersisted<"params" | "headers" | "body" | "auth">("api.tab", "params");
  const [res, setRes] = useState<ApiResponse | null>(null);
  const [inflight, setInflight] = useState<AbortController | null>(null);
  const [curlText, setCurlText] = useState<string | null>(null);

  useEffect(() => {
    if (!load) return;
    setReq(load.req);
    setRes(null);
    setTab(load.req.bodyMode !== "none" ? "body" : "params");
    onLoaded();
  }, [load, onLoaded, setReq, setTab]);

  const vars = envVars(env);
  const built = buildRequest(req, vars);
  const missing = unresolved([built.url, built.body, ...built.headers.flat()]);
  const patch = (p: Partial<ApiRequest>) => setReq((r) => ({ ...r, ...p }));
  const saved = collection.some((c) => c.id === req.id);

  const send = async () => {
    if (!built.url) return;
    inflight?.abort();
    const ctrl = new AbortController();
    setInflight(ctrl);
    const sent = req;
    const r = await execute(built, ctrl.signal);
    setInflight((c) => (c === ctrl ? null : c));
    setRes(r);
    setHistory((h) => [{ id: uid(), time: Date.now(), status: r.status, ms: r.ms, req: sent }, ...h].slice(0, HISTORY_MAX));
  };

  const save = () => setCollection((c) => (saved ? c.map((x) => (x.id === req.id ? req : x)) : [...c, req]));

  const importCurl = () => {
    const parsed = curlText ? parseCurl(curlText) : null;
    if (!parsed) return;
    setReq(parsed);
    setRes(null);
    setCurlText(null);
  };

  return (
    <div
      className="lsdm-api"
      onKeyDown={(e) => {
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          void send();
        }
      }}
    >
      <aside className="lsdm-side">
        <TabBar tabs={[["collection", "Saved"], ["history", "History"], ["env", "Env"]]} value={side} onChange={setSide} />
        <div className="lsdm-side-body">
          {side === "collection" && (
            <>
              <div className="lsdm-side-actions">
                <button className="lsdm-btn" onClick={() => { setReq(newRequest()); setRes(null); }}>+ New</button>
                <button className="lsdm-btn" onClick={() => setCurlText("")}>Import cURL</button>
              </div>
              {collection.length === 0 && <div className="lsdm-empty">Nothing saved yet</div>}
              {collection.map((c) => (
                <div key={c.id} className={c.id === req.id ? "lsdm-item lsdm-on" : "lsdm-item"} onClick={() => { setReq(structuredClone(c)); setRes(null); }}>
                  <span className={`lsdm-m lsdm-m-${c.method}`}>{c.method}</span>
                  <span className="lsdm-item-name">{c.name}</span>
                  <button className="lsdm-x" title="Delete" onClick={(e) => { e.stopPropagation(); setCollection((all) => all.filter((x) => x.id !== c.id)); }}>×</button>
                </div>
              ))}
            </>
          )}
          {side === "history" && (
            <>
              <div className="lsdm-side-actions"><button className="lsdm-btn" onClick={() => setHistory([])}>Clear history</button></div>
              {history.length === 0 && <div className="lsdm-empty">No requests yet</div>}
              {history.map((h) => (
                <div key={h.id} className="lsdm-item" title={new Date(h.time).toLocaleString()} onClick={() => { setReq(structuredClone(h.req)); setRes(null); }}>
                  <span className={`lsdm-m lsdm-m-${h.req.method}`}>{h.req.method}</span>
                  <span className={`lsdm-status ${statusClass(h.status)}`}>{h.status || "ERR"}</span>
                  <span className="lsdm-item-name">{h.req.url}</span>
                </div>
              ))}
            </>
          )}
          {side === "env" && (
            <>
              <div className="lsdm-hint">Write <code>{"{{name}}"}</code> in the URL, params, headers, body or auth.</div>
              <PairTable pairs={env} onChange={setEnv} keyLabel="Variable" />
            </>
          )}
        </div>
      </aside>

      <section className="lsdm-main">
        <div className="lsdm-namebar">
          <input className="lsdm-name" value={req.name} onChange={(e) => patch({ name: e.target.value })} aria-label="Request name" />
          <button className="lsdm-btn" onClick={save}>{saved ? "Save" : "Save to collection"}</button>
          <button className="lsdm-btn" onClick={() => void navigator.clipboard?.writeText(toCurl(built))}>Copy cURL</button>
        </div>
        <div className="lsdm-urlbar">
          <select className={`lsdm-method lsdm-m-${req.method}`} value={req.method} onChange={(e) => patch({ method: e.target.value as Method })} aria-label="Method">
            {METHODS.map((m) => <option key={m}>{m}</option>)}
          </select>
          <input
            className="lsdm-urlinput"
            placeholder="{{baseUrl}}/api/users or https://…"
            value={req.url}
            spellCheck={false}
            aria-label="URL"
            onChange={(e) => patch({ url: e.target.value })}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.ctrlKey && !e.metaKey) void send();
            }}
          />
          {inflight ? (
            <button className="lsdm-send lsdm-cancel" onClick={() => inflight.abort()}>Cancel</button>
          ) : (
            <button className="lsdm-send" onClick={() => void send()} disabled={!req.url.trim()}>Send</button>
          )}
        </div>
        {missing.length > 0 && <div className="lsdm-warn">Unknown variables: {missing.join(", ")} (define them under Env)</div>}

        <TabBar
          tabs={[
            ["params", `Params${req.params.length ? ` (${req.params.length})` : ""}`],
            ["headers", `Headers${req.headers.length ? ` (${req.headers.length})` : ""}`],
            ["body", `Body${req.bodyMode !== "none" ? " ●" : ""}`],
            ["auth", `Auth${req.auth.kind !== "none" ? " ●" : ""}`],
          ]}
          value={tab}
          onChange={setTab}
        />
        <div className="lsdm-req-body">
          {tab === "params" && <PairTable pairs={req.params} onChange={(params) => patch({ params })} />}
          {tab === "headers" && <PairTable pairs={req.headers} onChange={(headers) => patch({ headers })} />}
          {tab === "body" && (
            <>
              <div className="lsdm-row-flex">
                {(["none", "json", "text", "form"] as BodyMode[]).map((m) => (
                  <label key={m} className="lsdm-radio">
                    <input type="radio" checked={req.bodyMode === m} onChange={() => patch({ bodyMode: m })} />
                    {{ none: "None", json: "JSON", text: "Text", form: "Form (urlencoded)" }[m]}
                  </label>
                ))}
                {req.bodyMode === "json" && (
                  <button
                    className="lsdm-chip"
                    onClick={() => {
                      try {
                        patch({ body: JSON.stringify(JSON.parse(req.body), null, 2) });
                      } catch {
                        // invalid JSON: left as typed; the hint below says so
                      }
                    }}
                  >
                    Format
                  </button>
                )}
                {(req.method === "GET" || req.method === "HEAD") && req.bodyMode !== "none" && <span className="lsdm-dim">{req.method} requests are sent without a body</span>}
              </div>
              {(req.bodyMode === "json" || req.bodyMode === "text") && (
                <textarea className="lsdm-bodyedit" spellCheck={false} value={req.body} onChange={(e) => patch({ body: e.target.value })} placeholder={req.bodyMode === "json" ? '{\n  "key": "{{value}}"\n}' : ""} />
              )}
              {req.bodyMode === "json" && req.body.trim() && !isJson(interpolateSafe(req.body)) && <div className="lsdm-warn">Body is not valid JSON</div>}
              {req.bodyMode === "form" && <PairTable pairs={req.form} onChange={(form) => patch({ form })} />}
            </>
          )}
          {tab === "auth" && (
            <div className="lsdm-auth">
              <select value={req.auth.kind} onChange={(e) => patch({ auth: { ...req.auth, kind: e.target.value as ApiRequest["auth"]["kind"] } })} aria-label="Auth type">
                <option value="none">No auth</option>
                <option value="bearer">Bearer token</option>
                <option value="basic">Basic auth</option>
                <option value="apikey">API key header</option>
              </select>
              {req.auth.kind === "bearer" && <input placeholder="Token or {{token}}" value={req.auth.token} onChange={(e) => patch({ auth: { ...req.auth, token: e.target.value } })} />}
              {req.auth.kind === "basic" && (
                <>
                  <input placeholder="Username" value={req.auth.user} onChange={(e) => patch({ auth: { ...req.auth, user: e.target.value } })} />
                  <input placeholder="Password" type="password" value={req.auth.pass} onChange={(e) => patch({ auth: { ...req.auth, pass: e.target.value } })} />
                </>
              )}
              {req.auth.kind === "apikey" && (
                <>
                  <input placeholder="Header name" value={req.auth.keyName} onChange={(e) => patch({ auth: { ...req.auth, keyName: e.target.value } })} />
                  <input placeholder="Value" value={req.auth.keyValue} onChange={(e) => patch({ auth: { ...req.auth, keyValue: e.target.value } })} />
                </>
              )}
            </div>
          )}
        </div>

        <ResponseView res={res} busy={inflight !== null} />
      </section>

      {curlText !== null && (
        <div className="lsdm-modal" onClick={() => setCurlText(null)}>
          <div className="lsdm-dialog" onClick={(e) => e.stopPropagation()}>
            <div>Paste a cURL command (e.g. from the browser's Network panel, "Copy as cURL")</div>
            <textarea autoFocus className="lsdm-bodyedit" value={curlText} onChange={(e) => setCurlText(e.target.value)} spellCheck={false} />
            {curlText.trim() && !parseCurl(curlText) && <div className="lsdm-warn">No URL found in the command</div>}
            <div className="lsdm-row-flex">
              <span className="lsdm-grow" />
              <button className="lsdm-btn" onClick={() => setCurlText(null)}>Cancel</button>
              <button className="lsdm-send" onClick={importCurl} disabled={!curlText.trim()}>Import</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

/** Replaces `{{var}}` with a JSON-neutral placeholder so `"{{id}}"` and bare `{{id}}` both validate. */
function interpolateSafe(body: string): string {
  return body.replace(/\{\{\s*[\w.-]+\s*\}\}/g, "0");
}

/** True when the text parses as JSON. */
function isJson(text: string): boolean {
  try {
    JSON.parse(text);
    return true;
  } catch {
    return false;
  }
}
