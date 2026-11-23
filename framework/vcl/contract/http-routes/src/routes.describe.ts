/**
 * @module routes.describe — route contracts as plain JSON data (paths, methods, schemas, docs), the
 * machine-readable form for documentation, client generation and agents.
 */
import { describeSchema, type Access, type SchemaInfo } from "@coldmind/rpc";
import type { HttpContract, HttpMethod, RouteStream } from "@coldmind/http-routes/src/routes.contract.ts";

/**
 * One route as data. Schema slots are present only when declared.
 *
 * @example
 * describeRoutes(orders).routes.list.topic; // "http/orders/list"
 */
export type RouteInfo = {
  readonly name: string;
  readonly method: HttpMethod;
  readonly path: string;
  /** The bus topic the ingress publishes to. */
  readonly topic: string;
  readonly doc: string;
  readonly access: Access;
  readonly status: number;
  readonly idempotent: boolean;
  readonly noTimeout: boolean;
  readonly stream?: RouteStream;
  readonly params?: SchemaInfo;
  readonly query?: SchemaInfo;
  readonly headers?: SchemaInfo;
  readonly body?: SchemaInfo;
  readonly result?: SchemaInfo;
  readonly chunk?: SchemaInfo;
};

/**
 * A route group as data: its name, doc, and every route by name in declaration order.
 *
 * @example
 * const info: RouteContractInfo = describeRoutes(orders);
 * JSON.stringify(info); // the machine-readable contract
 */
export type RouteContractInfo = {
  readonly name: string;
  readonly doc: string;
  /** Keyed by route name. */
  readonly routes: Readonly<Record<string, RouteInfo>>;
  /** Declaration order of the route names. */
  readonly order: readonly string[];
};

/**
 * Describes a route group as JSON-serializable data.
 *
 * @example
 * const info = describeRoutes(orders);
 * info.routes.get.path; // "/orders/:id"
 * JSON.stringify(info); // the machine-readable contract
 */
export function describeRoutes(c: HttpContract): RouteContractInfo {
  const routes: Record<string, RouteInfo> = {};
  const order: string[] = [];
  for (const [key, def] of Object.entries(c.routes)) {
    order.push(key);
    routes[key] = {
      name: `${c.name}.${key}`,
      method: def.method,
      path: def.path,
      topic: `http/${c.name}/${key}`,
      doc: def.doc ?? "",
      access: def.access ?? "authenticated",
      status: def.status ?? 200,
      idempotent: def.idempotent === true,
      noTimeout: def.noTimeout === true || def.stream !== undefined,
      ...(def.stream === undefined ? {} : { stream: def.stream }),
      ...(def.params === undefined ? {} : { params: describeSchema(def.params) }),
      ...(def.query === undefined ? {} : { query: describeSchema(def.query) }),
      ...(def.headers === undefined ? {} : { headers: describeSchema(def.headers) }),
      ...(def.body === undefined ? {} : { body: describeSchema(def.body) }),
      ...(def.result === undefined ? {} : { result: describeSchema(def.result) }),
      ...(def.chunk === undefined ? {} : { chunk: describeSchema(def.chunk) }),
    };
  }
  return { name: c.name, doc: c.doc ?? "", routes, order };
}
