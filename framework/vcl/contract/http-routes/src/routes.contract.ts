/**
 * @module routes.contract — HTTP route contracts: a named group of routes declared like an rpc `contract()`,
 * plus the conversion to the rpc contract the server registers them as.
 */
import { Field, isModel, obj, str, toShape, type AnyModel, type Shape, type SomeField } from "@coldmind/model";
import { contract, type Access, type Contract, type OutputOf, type ProcMap, type Schema } from "@coldmind/rpc";
import { compilePath } from "@coldmind/http-routes/src/routes.path.ts";

/**
 * Methods a route can be declared with. `HEAD` is answered from `GET`; `OPTIONS` belongs to the CORS layer.
 *
 * @example
 * const method: HttpMethod = "GET";
 */
export type HttpMethod = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

/**
 * The methods a route may declare, in a fixed order.
 *
 * @example
 * HTTP_METHODS.includes("PATCH"); // true
 */
export const HTTP_METHODS: readonly HttpMethod[] = ["GET", "POST", "PUT", "PATCH", "DELETE"];

/**
 * How a route sends its result: `chunk`s as a chunked body, or as server-sent events.
 *
 * How a streaming route frames its chunks: newline-delimited (`chunked`) or Server-Sent Events (`sse`).
 *
 * @example
 * const stream: RouteStream = "sse";
 */
export type RouteStream = "chunked" | "sse";

/**
 * A shape or model of fields; each value arrives from the URL or a header as a string and is coerced by field kind.
 *
 * A schema for params, query or headers: a shape of fields or a model.
 *
 * @example
 * const query: FieldsSchema = { limit: num().default(10) };
 */
export type FieldsSchema = Shape | AnyModel;

/**
 * One route. The schema slots mirror what a request carries; the ingress assembles them into the arguments of
 * the route's proc as `{ params, query, headers, body }`, each present only when its schema is declared
 * (`params` is always present: the path parameters as strings when no schema is declared).
 *
 * @example
 * const def: HttpRouteDef = { method: "GET", path: "/orders/:id", params: { id: str() }, result: { sku: str() }, access: "public" };
 */
export type HttpRouteDef = {
  method: HttpMethod;
  /** `/orders/:id` (one segment) or `/files/*rest` (one or more trailing segments). */
  path: string;
  /** Path parameters. Keys must be exactly the parameters in `path`. */
  params?: FieldsSchema;
  query?: FieldsSchema;
  /** Request headers to read, by lowercase name. */
  headers?: FieldsSchema;
  /** The JSON body. Not allowed on `GET`. */
  body?: Schema;
  /** Shape of the JSON response body. */
  result?: Schema;
  /** Shape of one streamed chunk when `stream` is set. */
  chunk?: Schema;
  stream?: RouteStream;
  /** Status of a successful response. Default 200. */
  status?: number;
  /** Read by the access check of the server chain. Default `"authenticated"`, like procs. */
  access?: Access;
  /** Informational text. First line is the summary, the rest the description. */
  doc?: string;
  /** Calling it twice has the effect of calling it once; the request is retried over the bus. */
  idempotent?: boolean;
  /** A response that stays open on purpose (a stream). Exempt from the call timeout. */
  noTimeout?: boolean;
};

/**
 * Route definitions by route name.
 *
 * @example
 * const routes: RouteMap = { list: { method: "GET", path: "/orders", access: "public" } };
 */
export type RouteMap = Record<string, HttpRouteDef>;

/**
 * A named group of routes. `name` becomes part of the bus topic, so it is restricted to `[A-Za-z0-9_.-]`.
 *
 * @example
 * const orders: HttpContract = httpContract("orders", { list: { method: "GET", path: "/orders", access: "public" } });
 * orders.routes.list.path; // "/orders"
 */
export type HttpContract<R extends RouteMap = RouteMap> = {
  name: string;
  routes: R;
  /** Informational text for the group as a whole. */
  doc?: string;
};

/**
 * Options of {@link httpContract}.
 *
 * @example
 * httpContract("orders", routes, { doc: "Order routes" });
 */
export type HttpContractOptions = { doc?: string | undefined };

const TOKEN = /^[A-Za-z0-9_][A-Za-z0-9_.-]*$/;

/**
 * Throws unless `name` is safe to put in a bus topic segment.
 *
 * @example
 * assertTopicSafe("route", "list");   // fine
 * assertTopicSafe("route", "a/b");    // throws TypeError
 */
export function assertTopicSafe(kind: string, name: string): void {
  if (!TOKEN.test(name)) throw new TypeError(`Invalid ${kind} "${name}": use letters, digits, "_", "." and "-"`);
}

/**
 * The field names of a params, query or headers schema.
 *
 * @example
 * fieldNamesOf({ id: str(), limit: num() }); // ["id", "limit"]
 */
export const fieldNamesOf = (s: FieldsSchema): string[] => Object.keys(isModel(s as AnyModel) ? toShape(s as AnyModel) : (s as Shape));

function checkRoute(controller: string, key: string, def: HttpRouteDef): void {
  const at = `${controller}.${key}`;
  assertTopicSafe("route name", key);
  if (!HTTP_METHODS.includes(def.method)) throw new TypeError(`${at}: unsupported method "${String(def.method)}"`);
  const { params } = compilePath(def.path);
  if (def.params !== undefined) {
    const declared = fieldNamesOf(def.params).sort().join(",");
    if (declared !== [...params].sort().join(",")) throw new TypeError(`${at}: params schema keys [${declared}] must equal the path parameters [${params.join(",")}]`);
  }
  if (def.body !== undefined && def.method === "GET") throw new TypeError(`${at}: a GET route cannot declare a body`);
  if (def.status !== undefined && !(Number.isInteger(def.status) && def.status >= 200 && def.status <= 599))
    throw new TypeError(`${at}: invalid status ${String(def.status)}`);
  if (def.chunk !== undefined && def.stream === undefined) throw new TypeError(`${at}: chunk requires stream`);
  if (def.stream !== undefined && def.stream !== "chunked" && def.stream !== "sse") throw new TypeError(`${at}: invalid stream "${String(def.stream)}"`);
}

/**
 * Declares a group of routes, validating every definition: path syntax, `params` keys equal to the path
 * parameters, no body on `GET`, a valid `status`, `chunk` only with `stream`, and topic-safe names.
 *
 * @param name the group name; the second segment of the bus topic `http/<name>/<route>`.
 * @param routes route definitions by route name.
 * @throws {TypeError} When a definition is invalid.
 * @example
 * const orders = httpContract("orders", {
 *   get: { method: "GET", path: "/orders/:id", params: { id: str() }, result: { sku: str() }, access: "public" },
 *   create: { method: "POST", path: "/orders", body: { sku: str() }, status: 201 },
 * });
 */
export function httpContract<R extends RouteMap>(name: string, routes: R, opts: HttpContractOptions = {}): HttpContract<R> {
  assertTopicSafe("contract name", name);
  for (const [key, def] of Object.entries(routes)) checkRoute(name, key, def);
  return opts.doc === undefined ? { name, routes } : { name, routes, doc: opts.doc };
}

/**
 * The values a handler receives for route `D`: validated and defaulted.
 *
 * @example
 * type Args = RouteArgs<{ method: "GET"; path: "/o/:id"; params: { id: ReturnType<typeof str> }; query: { n: ReturnType<typeof num> } }>;
 * // { params: { id: string }; query: { n: number } }
 */
export type RouteArgs<D extends HttpRouteDef> = {
  params: D extends { params: infer S extends Schema } ? OutputOf<S> : Record<string, string>;
} & (D extends { query: infer S extends Schema } ? { query: OutputOf<S> } : unknown) &
  (D extends { headers: infer S extends Schema } ? { headers: OutputOf<S> } : unknown) &
  (D extends { body: infer S extends Schema } ? { body: OutputOf<S> } : unknown);

const asField = (s: FieldsSchema): SomeField => obj(isModel(s as AnyModel) ? toShape(s as AnyModel) : (s as Shape));

/**
 * The rpc contract a route group is registered as: controller `http.<name>`, one proc per route whose
 * `args` are `{ params, query?, headers?, body? }`. The server then validates, authorizes and times each
 * request like any other call. `result` and `chunk` are not on the procs: what a responder replies is an
 * HTTP reply, not the bare result, so the ingress validates the body against the route's `result` itself.
 *
 * @param c a contract from {@link httpContract}.
 *
 * @example
 * const rpc = procContractOf(orders);
 * rpc.name; // "http.orders"
 */
export function procContractOf(c: HttpContract): Contract {
  const procs: ProcMap = {};
  for (const [key, def] of Object.entries(c.routes)) {
    const args: Record<string, SomeField> = {};
    args.params =
      def.params !== undefined ? asField(def.params) : obj(Object.fromEntries(compilePath(def.path).params.map(name => [name, str()])));
    if (def.query !== undefined) args.query = asField(def.query);
    if (def.headers !== undefined) args.headers = asField(def.headers);
    if (def.body !== undefined) args.body = bodyField(def.body);
    procs[key] = {
      args,
      ...(def.access === undefined ? {} : { access: def.access }),
      ...(def.doc === undefined ? {} : { doc: def.doc }),
      ...(def.idempotent === undefined ? {} : { idempotent: def.idempotent }),
      ...(def.noTimeout === undefined && def.stream === undefined ? {} : { noTimeout: def.noTimeout ?? true }),
    };
  }
  return contract(`http.${c.name}`, procs, { doc: c.doc });
}

function bodyField(s: Schema): SomeField {
  if (s instanceof Field) return s;
  if (isModel(s as AnyModel)) return obj(toShape(s as AnyModel));
  if (typeof (s as { parse?: unknown }).parse === "function") throw new TypeError("A route body must be a field, a shape or a model; an opaque parse schema cannot be nested");
  return obj(s as Shape);
}
