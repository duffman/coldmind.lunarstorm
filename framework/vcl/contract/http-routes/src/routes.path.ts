/**
 * @module routes.path — compiles route paths (`/orders/:id`, `/files/*rest`) and matches request paths
 * against them. Pure string work: no I/O.
 */

/**
 * One piece of a compiled path.
 *
 * One compiled segment: a literal, a `:param` or a `*wildcard`.
 *
 * @example
 * compilePath("/orders/:id").segments;
 * // [{ kind: "literal", value: "orders" }, { kind: "param", name: "id" }]
 */
export type PathSegment =
  | { readonly kind: "literal"; readonly value: string }
  | { readonly kind: "param"; readonly name: string }
  | { readonly kind: "wildcard"; readonly name: string };

/**
 * A path pattern ready for matching.
 *
 * @example
 * const path: CompiledPath = compilePath("/files/*rest");
 * path.params; // ["rest"]
 */
export type CompiledPath = {
  /** The pattern as written, normalized to have no trailing slash (except the root `/`). */
  readonly pattern: string;
  readonly segments: readonly PathSegment[];
  /** Parameter names in order of appearance, wildcard last. */
  readonly params: readonly string[];
  /** Pattern with parameter names erased (`/orders/:`); two routes with the same key are the same route. */
  readonly shape: string;
};

const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/**
 * The request path was not valid percent-encoding.
 *
 * @example
 * try {
 *   matchPath(compilePath("/o/:id"), "/o/%E0%A4%A");
 * } catch (e) {
 *   e instanceof InvalidPathError; // true
 * }
 */
export class InvalidPathError extends Error {
  constructor(path: string) {
    super(`Invalid percent-encoding in path: ${path}`);
    this.name = "InvalidPathError";
  }
}

/** Splits a path into raw segments. `/` is no segments; one trailing slash is ignored; `//` yields an empty segment. */
function split(path: string): string[] {
  const parts = path.split("/").slice(1);
  if (parts.length > 0 && parts[parts.length - 1] === "") parts.pop();
  return parts;
}

/**
 * Compiles a route path. `:name` matches exactly one non-empty segment; `*name` must be last and matches
 * one or more remaining segments, joined by `/`. Literal segments are compared after percent-decoding.
 *
 * @param path starts with `/`, no query or fragment, no empty segments, unique parameter names.
 * @throws {TypeError} When the path is malformed.
 * @example
 * compilePath("/orders/:id/items").params; // ["id"]
 */
export function compilePath(path: string): CompiledPath {
  if (typeof path !== "string" || !path.startsWith("/")) throw new TypeError(`Route path must start with "/": ${String(path)}`);
  if (/[?#\s]/.test(path)) throw new TypeError(`Route path must not contain a query, fragment or whitespace: ${path}`);
  const raw = split(path);
  const segments: PathSegment[] = [];
  const seen = new Set<string>();
  raw.forEach((part, i) => {
    if (part === "") throw new TypeError(`Route path has an empty segment: ${path}`);
    const lead = part[0];
    if (lead === ":" || lead === "*") {
      const name = part.slice(1);
      if (!NAME.test(name)) throw new TypeError(`Invalid parameter name "${name}" in ${path}`);
      if (seen.has(name)) throw new TypeError(`Duplicate parameter "${name}" in ${path}`);
      seen.add(name);
      if (lead === "*") {
        if (i !== raw.length - 1) throw new TypeError(`A wildcard must be the last segment: ${path}`);
        segments.push({ kind: "wildcard", name });
      } else segments.push({ kind: "param", name });
      return;
    }
    let value: string;
    try {
      value = decodeURIComponent(part);
    } catch {
      throw new TypeError(`Invalid percent-encoding in route path: ${path}`);
    }
    segments.push({ kind: "literal", value });
  });
  const pattern = segments.length === 0 ? "/" : `/${raw.join("/")}`;
  const shape =
    segments.length === 0
      ? "/"
      : `/${segments.map(s => (s.kind === "literal" ? encodeURIComponent(s.value) : s.kind === "param" ? ":" : "*")).join("/")}`;
  return { pattern, segments, params: segments.flatMap(s => (s.kind === "literal" ? [] : [s.name])), shape };
}

/**
 * Matches a request path against a compiled path.
 *
 * @param compiled the pattern.
 * @param rawPath the request path without query, still percent-encoded. A `%2F` stays inside its segment.
 * @returns the decoded parameters, or `null` when it does not match.
 * @throws {InvalidPathError} When a segment is not valid percent-encoding.
 *
 * @example
 * matchPath(compilePath("/orders/:id"), "/orders/42"); // { id: "42" }
 * matchPath(compilePath("/orders/:id"), "/other");     // null
 */
export function matchPath(compiled: CompiledPath, rawPath: string): Record<string, string> | null {
  const parts = rawPath.startsWith("/") ? split(rawPath) : null;
  if (!parts) return null;
  const decoded = parts.map(p => {
    try {
      return decodeURIComponent(p);
    } catch {
      throw new InvalidPathError(rawPath);
    }
  });
  const out: Record<string, string> = {};
  const { segments } = compiled;
  for (let i = 0; i < segments.length; i++) {
    const seg = segments[i]!;
    if (seg.kind === "wildcard") {
      if (decoded.length - i < 1 || parts.slice(i).some(p => p === "")) return null;
      out[seg.name] = decoded.slice(i).join("/");
      return out;
    }
    const part = decoded[i];
    if (part === undefined || parts[i] === "") return null;
    if (seg.kind === "literal") {
      if (part !== seg.value) return null;
    } else out[seg.name] = part;
  }
  return decoded.length === segments.length ? out : null;
}

const RANK = { literal: 2, param: 1, wildcard: 0 } as const;

/**
 * Orders two patterns that both match a path: negative when `a` is more specific. Segments are compared left
 * to right; a literal beats a parameter, which beats a wildcard.
 *
 * @example
 * compareSpecificity(compilePath("/o/latest"), compilePath("/o/:id")) < 0; // literal beats parameter
 */
export function compareSpecificity(a: CompiledPath, b: CompiledPath): number {
  const n = Math.max(a.segments.length, b.segments.length);
  for (let i = 0; i < n; i++) {
    const x = a.segments[i];
    const y = b.segments[i];
    if (!x || !y) return x ? -1 : 1;
    const d = RANK[y.kind] - RANK[x.kind];
    if (d !== 0) return d;
  }
  return 0;
}
