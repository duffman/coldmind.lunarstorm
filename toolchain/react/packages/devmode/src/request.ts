/**
 * The API executor's model: a request as the user edits it, how it becomes a `fetch` call, and cURL in and out.
 * Pure functions only; the React side lives in `api-tab.tsx`.
 */

/** HTTP methods offered in the method picker. */
export const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] as const;
export type Method = (typeof METHODS)[number];

/** One row of a key/value table (params, headers, form fields, environment). Disabled rows are kept but not sent. */
export interface Pair {
  key: string;
  value: string;
  enabled: boolean;
}

/** How the body editor's text is sent. `form` sends `form` pairs as `application/x-www-form-urlencoded`. */
export type BodyMode = "none" | "json" | "text" | "form";

/** Auth helper. Decision: flat string fields rather than a union, so switching `kind` keeps what was typed. */
export interface Auth {
  kind: "none" | "bearer" | "basic" | "apikey";
  token: string;
  user: string;
  pass: string;
  keyName: string;
  keyValue: string;
}

/** A request as edited and saved in a collection. Every string field may hold `{{var}}` references. */
export interface ApiRequest {
  id: string;
  name: string;
  method: Method;
  url: string;
  params: Pair[];
  headers: Pair[];
  bodyMode: BodyMode;
  body: string;
  form: Pair[];
  auth: Auth;
}

/** The concrete call after variables are substituted: what `fetch` receives and what cURL export prints. */
export interface BuiltRequest {
  method: Method;
  url: string;
  headers: [string, string][];
  body?: string;
}

/** A completed (or failed) execution. `status` is 0 when no response arrived; `error` then says why. */
export interface ApiResponse {
  status: number;
  statusText: string;
  headers: [string, string][];
  body: string;
  ms: number;
  size: number;
  error?: string;
}

/** A unique id for requests and collection entries. */
export function uid(): string {
  return globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

/**
 * A blank request with defaults filled in.
 * @param over fields to set instead of the defaults
 * @return a new request with a fresh id unless `over.id` is given
 */
export function newRequest(over: Partial<ApiRequest> = {}): ApiRequest {
  return {
    id: uid(), name: "Untitled", method: "GET", url: "", params: [], headers: [], bodyMode: "none", body: "", form: [],
    auth: { kind: "none", token: "", user: "", pass: "", keyName: "X-API-Key", keyValue: "" },
    ...over,
  };
}

/**
 * Replaces `{{name}}` with the variable's value. Unknown names stay as written so the user sees what is missing.
 * @param text text to substitute in
 * @param vars variable values by name
 * @return the substituted text
 */
export function interpolate(text: string, vars: Record<string, string>): string {
  return text.replace(/\{\{\s*([\w.-]+)\s*\}\}/g, (all, name: string) => (name in vars ? vars[name]! : all));
}

/**
 * Turns enabled environment rows into a lookup.
 * @param env environment rows
 * @return value by name; later rows win
 */
export function envVars(env: Pair[]): Record<string, string> {
  const vars: Record<string, string> = {};
  for (const p of env) if (p.enabled && p.key) vars[p.key] = p.value;
  return vars;
}

/** Base64 of UTF-8 text. `btoa` alone throws on characters outside Latin-1. */
function base64(text: string): string {
  let bin = "";
  for (const b of new TextEncoder().encode(text)) bin += String.fromCharCode(b);
  return btoa(bin);
}

/** Case-insensitive header lookup. */
function hasHeader(headers: [string, string][], name: string): boolean {
  const lower = name.toLowerCase();
  return headers.some(([k]) => k.toLowerCase() === lower);
}

/**
 * Resolves a request into the concrete call: substitutes variables, appends params, applies auth and body.
 * Decision: params are appended by string, not through `URL`, so relative same-origin paths (`/api/users`) work.
 * @param req the edited request
 * @param vars environment variables
 * @return the call to make
 */
export function buildRequest(req: ApiRequest, vars: Record<string, string>): BuiltRequest {
  const sub = (s: string) => interpolate(s, vars);
  let url = sub(req.url.trim());
  const query = req.params
    .filter((p) => p.enabled && p.key)
    .map((p) => `${encodeURIComponent(sub(p.key))}=${encodeURIComponent(sub(p.value))}`)
    .join("&");
  if (query) {
    const hash = url.indexOf("#");
    const [head, tail] = hash < 0 ? [url, ""] : [url.slice(0, hash), url.slice(hash)];
    url = `${head}${head.includes("?") ? (head.endsWith("?") || head.endsWith("&") ? "" : "&") : "?"}${query}${tail}`;
  }

  const headers: [string, string][] = req.headers.filter((p) => p.enabled && p.key).map((p) => [sub(p.key), sub(p.value)]);
  const a = req.auth;
  if (a.kind === "bearer" && a.token) headers.push(["Authorization", `Bearer ${sub(a.token)}`]);
  if (a.kind === "basic" && (a.user || a.pass)) headers.push(["Authorization", `Basic ${base64(`${sub(a.user)}:${sub(a.pass)}`)}`]);
  if (a.kind === "apikey" && a.keyName) headers.push([sub(a.keyName), sub(a.keyValue)]);

  let body: string | undefined;
  if (req.method !== "GET" && req.method !== "HEAD") {
    const type = { json: "application/json", text: "text/plain", form: "application/x-www-form-urlencoded", none: "" }[req.bodyMode];
    if (req.bodyMode === "form") {
      body = new URLSearchParams(req.form.filter((p) => p.enabled && p.key).map((p) => [sub(p.key), sub(p.value)])).toString();
    } else if (req.bodyMode !== "none") {
      body = sub(req.body);
    }
    if (body !== undefined && type && !hasHeader(headers, "content-type")) headers.push(["Content-Type", type]);
  }
  return { method: req.method, url, headers, body };
}

/**
 * Executes a request and reads the whole response as text. Never throws: network errors, CORS refusals and
 * aborts come back as `status: 0` with `error` set.
 * @param built the call from `buildRequest`
 * @param signal cancels the call
 * @param fetchImpl the fetch to use; defaults to the global one
 * @return the response, with wall time in ms and body size in bytes
 */
export async function execute(built: BuiltRequest, signal?: AbortSignal, fetchImpl: typeof fetch = globalThis.fetch): Promise<ApiResponse> {
  const start = performance.now();
  try {
    const res = await fetchImpl(built.url, { method: built.method, headers: built.headers, body: built.body, signal });
    const body = await res.text();
    return {
      status: res.status, statusText: res.statusText, headers: [...res.headers.entries()], body,
      ms: Math.round(performance.now() - start), size: new TextEncoder().encode(body).length,
    };
  } catch (e) {
    const aborted = e instanceof Error && e.name === "AbortError";
    return {
      status: 0, statusText: "", headers: [], body: "", ms: Math.round(performance.now() - start), size: 0,
      error: aborted ? "Request cancelled" : `${e instanceof Error ? e.message : String(e)} (network error, CORS refusal or bad URL)`,
    };
  }
}

/** Quotes a shell word with single quotes. */
function shq(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`;
}

/**
 * Prints a call as a cURL command line.
 * @param built the call from `buildRequest`
 * @return the command, one option per line
 */
export function toCurl(built: BuiltRequest): string {
  const parts = [`curl${built.method === "GET" ? "" : ` -X ${built.method}`} ${shq(built.url)}`];
  for (const [k, v] of built.headers) parts.push(`-H ${shq(`${k}: ${v}`)}`);
  if (built.body !== undefined) parts.push(`--data-raw ${shq(built.body)}`);
  return parts.join(" \\\n  ");
}

/**
 * Splits shell words: single quotes, double quotes with backslash escapes, backslash-newline continuations.
 * @param text command line
 * @return the words
 */
function shellWords(text: string): string[] {
  const words: string[] = [];
  let cur = "";
  let inWord = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i]!;
    if (c === "'") {
      const end = text.indexOf("'", i + 1);
      cur += text.slice(i + 1, end < 0 ? text.length : end);
      i = end < 0 ? text.length : end;
      inWord = true;
    } else if (c === '"') {
      i++;
      while (i < text.length && text[i] !== '"') {
        if (text[i] === "\\" && i + 1 < text.length) i++;
        cur += text[i++];
      }
      inWord = true;
    } else if (c === "\\" && text[i + 1] === "\n") {
      i++;
    } else if (c === "\\" && i + 1 < text.length) {
      cur += text[++i];
      inWord = true;
    } else if (/\s/.test(c)) {
      if (inWord) words.push(cur);
      cur = "";
      inWord = false;
    } else {
      cur += c;
      inWord = true;
    }
  }
  if (inWord) words.push(cur);
  return words;
}

/**
 * Splits a URL's query string into param rows, so imported requests edit like hand-written ones.
 * @param url URL that may carry `?a=1&b=2`
 * @return the URL without its query, and the rows
 */
export function splitQuery(url: string): { url: string; params: Pair[] } {
  const q = url.indexOf("?");
  if (q < 0) return { url, params: [] };
  const hash = url.indexOf("#", q);
  const query = url.slice(q + 1, hash < 0 ? undefined : hash);
  const params = [...new URLSearchParams(query)].map(([key, value]) => ({ key, value, enabled: true }));
  return { url: url.slice(0, q) + (hash < 0 ? "" : url.slice(hash)), params };
}

/** cURL flags that take a value and are ignored on import. */
const SKIPPED_WITH_VALUE = new Set(["-o", "--output", "-m", "--max-time", "--connect-timeout", "-w", "--write-out", "-e", "--referer", "-x", "--proxy"]);

/**
 * Parses a cURL command line (as copied from browser devtools) into a request.
 * @param text the command
 * @return the request, or `null` when the text holds no URL
 */
export function parseCurl(text: string): ApiRequest | null {
  const words = shellWords(text.trim());
  if (words[0] === "curl") words.shift();
  let method: string | undefined;
  let url = "";
  const headers: Pair[] = [];
  const data: string[] = [];
  let basic: string | undefined;
  for (let i = 0; i < words.length; i++) {
    const w = words[i]!;
    const next = () => words[++i] ?? "";
    if (w === "-X" || w === "--request") method = next().toUpperCase();
    else if (w === "-H" || w === "--header") {
      const h = next();
      const colon = h.indexOf(":");
      if (colon > 0) headers.push({ key: h.slice(0, colon).trim(), value: h.slice(colon + 1).trim(), enabled: true });
    } else if (w === "-d" || w.startsWith("--data")) data.push(next());
    else if (w === "-u" || w === "--user") basic = next();
    else if (w === "-A" || w === "--user-agent") headers.push({ key: "User-Agent", value: next(), enabled: true });
    else if (w === "-b" || w === "--cookie") headers.push({ key: "Cookie", value: next(), enabled: true });
    else if (w === "--url") url = next();
    else if (SKIPPED_WITH_VALUE.has(w)) next();
    else if (!w.startsWith("-") && !url) url = w;
  }
  if (!url) return null;
  const split = splitQuery(url);
  const body = data.join("&");
  const req = newRequest({
    name: split.url.replace(/^https?:\/\/[^/]+/, "") || split.url,
    method: (METHODS as readonly string[]).includes(method ?? "") ? (method as Method) : data.length ? "POST" : "GET",
    url: split.url, params: split.params, headers,
  });
  if (data.length) {
    let json = false;
    try {
      JSON.parse(body);
      json = true;
    } catch {
      // not JSON: sent as text, with the Content-Type header from the command kept
    }
    req.bodyMode = json ? "json" : "text";
    req.body = json ? JSON.stringify(JSON.parse(body), null, 2) : body;
  }
  if (basic !== undefined) {
    const colon = basic.indexOf(":");
    req.auth = { ...req.auth, kind: "basic", user: colon < 0 ? basic : basic.slice(0, colon), pass: colon < 0 ? "" : basic.slice(colon + 1) };
  }
  return req;
}

/**
 * Formats a byte count for display.
 * @param n bytes
 * @return e.g. `512 B`, `1.4 KB`
 */
export function formatBytes(n: number): string {
  return n < 1024 ? `${n} B` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`;
}
