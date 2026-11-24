/**
 * @module routes.table — the route table: every declared route compiled once, matched by method and path with
 * a fixed precedence. The table, not the bus, decides which route a request is for.
 */
import { procContractOf, type HttpContract, type HttpMethod, type HttpRouteDef } from "@coldmind/http-routes/src/routes.contract.ts";
import { compilePath, compareSpecificity, matchPath, type CompiledPath } from "@coldmind/http-routes/src/routes.path.ts";

/**
 * A route with everything the ingress needs to dispatch it.
 *
 * @example
 * const route = new RouteTable([orders]).routes[0]!;
 * route.topic; // "http/orders/list"
 */
export type CompiledRoute = {
  readonly controller: string;
  readonly key: string;
  readonly def: HttpRouteDef;
  readonly path: CompiledPath;
  /** The proc name the server registers: `http.<controller>.<route>`. */
  readonly proc: string;
  /** The bus topic: `http/<controller>/<route>`. */
  readonly topic: string;
};

/**
 * What {@link RouteTable.match} found.
 *
 * @example
 * const found = new RouteTable([orders]).match("GET", "/orders");
 * if (found.kind === "match") found.route.proc; // "http.orders.list"
 */
export type RouteMatch =
  | { readonly kind: "match"; readonly route: CompiledRoute; readonly params: Record<string, string>; readonly viaHead: boolean }
  | { readonly kind: "method-not-allowed"; readonly allow: readonly string[] }
  | { readonly kind: "not-found" }
  | { readonly kind: "bad-path" };

/**
 * Routes of one or more contracts. Precedence among routes that match the same path: literal segment over
 * parameter over wildcard, compared left to right. `HEAD` falls back to `GET`.
 *
 * @example
 * const table = new RouteTable([orders]);
 * table.match("GET", "/orders/42"); // { kind: "match", route, params: { id: "42" }, viaHead: false }
 */
export class RouteTable {
  readonly routes: readonly CompiledRoute[];
  readonly contracts: readonly HttpContract[];

  /**
   * @throws {Error} On a duplicate contract name, or two routes with the same method and path shape
   * (`/a/:x` and `/a/:y` are the same route).
   */
  constructor(contracts: readonly HttpContract[]) {
    const routes: CompiledRoute[] = [];
    const names = new Set<string>();
    const shapes = new Map<string, string>();
    for (const c of contracts) {
      if (names.has(c.name)) throw new Error(`Duplicate route contract: ${c.name}`);
      names.add(c.name);
      for (const [key, def] of Object.entries(c.routes)) {
        const path = compilePath(def.path);
        const id = `${def.method} ${path.shape}`;
        const prior = shapes.get(id);
        if (prior) throw new Error(`Duplicate route ${def.method} ${path.pattern}: ${c.name}.${key} and ${prior}`);
        shapes.set(id, `${c.name}.${key}`);
        routes.push({ controller: c.name, key, def, path, proc: `http.${c.name}.${key}`, topic: `http/${c.name}/${key}` });
      }
    }
    this.routes = routes;
    this.contracts = [...contracts];
  }

  /** Routes registered as rpc controllers' contracts, for `Server.controller()`. */
  procContracts() {
    return this.contracts.map(procContractOf);
  }

  /**
   * @param method the request method, uppercase.
   * @param rawPath the request path without query, still percent-encoded.
   */
  match(method: string, rawPath: string): RouteMatch {
    const hits: { route: CompiledRoute; params: Record<string, string> }[] = [];
    try {
      for (const route of this.routes) {
        const params = matchPath(route.path, rawPath);
        if (params) hits.push({ route, params });
      }
    } catch {
      return { kind: "bad-path" };
    }
    if (hits.length === 0) return { kind: "not-found" };
    const pick = (m: HttpMethod | string) =>
      hits.filter(h => h.route.def.method === m).sort((a, b) => compareSpecificity(a.route.path, b.route.path))[0];
    const exact = pick(method);
    if (exact) return { kind: "match", ...exact, viaHead: false };
    const head = method === "HEAD" ? pick("GET") : undefined;
    if (head) return { kind: "match", ...head, viaHead: true };
    const allow = new Set<string>();
    for (const h of hits) {
      allow.add(h.route.def.method);
      if (h.route.def.method === "GET") allow.add("HEAD");
    }
    return { kind: "method-not-allowed", allow: [...allow].sort() };
  }
}
