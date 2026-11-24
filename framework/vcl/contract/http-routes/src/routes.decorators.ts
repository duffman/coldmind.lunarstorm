/**
 * @module routes.decorators — declare a route group as a class: `@httpController` names it, `@get`, `@post`
 * and friends declare each route on a method. Built on `defineDecorator`, so they work with Bun's native
 * decorators and with `experimentalDecorators`.
 */
import { defineDecorator } from "@coldmind/rpc";
import { httpContract, type HttpContract, type HttpContractOptions, type HttpMethod, type HttpRouteDef, type RouteMap } from "@coldmind/http-routes/src/routes.contract.ts";

type ControllerInfo = { name: string; doc?: string | undefined };

const controllers = new WeakMap<Function, ControllerInfo>();
const routes = new WeakMap<Function, Map<string, HttpRouteDef>>();

/**
 * Handlers as the responder registration takes them: one function per route, bound to the instance.
 *
 * The handlers of a decorated instance, by route name, bound to it.
 *
 * @example
 * const handlers: BoundRouteHandlers = routeHandlersOf(new Orders());
 * handlers.get?.({ args: { params: { id: "1" } } }, {});
 */
export type BoundRouteHandlers = Record<string, (input: any, call: any) => unknown>;

/**
 * Marks a class as the route group `name`. Its route-decorated methods become the group's routes.
 *
 * @param name the group name, the second segment of the bus topic `http/<name>/<route>`.
 * @param opts `doc` for the group as a whole.
 * @example
 * @httpController("orders")
 * class Orders {
 *   @get("/orders/:id", { params: { id: str() }, access: "public" })
 *   get({ args }: { args: { params: { id: string } } }) { return { id: args.params.id }; }
 * }
 */
export const httpController = defineDecorator("httpController", (site, name: string, opts: HttpContractOptions = {}) => {
  if (site.kind !== "class") throw new TypeError("@httpController can only decorate a class");
  controllers.set(site.target, { name, doc: opts.doc });
});

/**
 * Declares a method as a route of its class's group. The method name is the route name.
 *
 * @param def the full route definition including `method` and `path`.
 * @throws {TypeError} When applied to a static or symbol-named method, or to anything but a method.
 *
 * @example
 * @httpController("orders")
 * class Orders {
 *   @httpRoute({ method: "GET", path: "/orders", access: "public" })
 *   list() { return []; }
 * }
 */
export const httpRoute = defineDecorator("httpRoute", (site, def: HttpRouteDef) => {
  if (site.kind !== "method") throw new TypeError("@httpRoute can only decorate a method");
  if (site.isStatic) throw new TypeError(`@httpRoute ${String(site.name)}: static methods cannot be routes`);
  if (typeof site.name !== "string") throw new TypeError("@httpRoute: symbol-named methods cannot be routes");
  const key = site.name;
  site.onClass(ctor => {
    const own = routes.get(ctor) ?? new Map<string, HttpRouteDef>();
    own.set(key, def);
    routes.set(ctor, own);
  });
});

type VerbDef = Omit<HttpRouteDef, "method" | "path">;
const verb = (method: HttpMethod) => (path: string, def: VerbDef = {}) => httpRoute({ ...def, method, path });

/** `@httpRoute` for `GET`. @example `@get("/orders/:id", { params: { id: str() } })` */
export const get = verb("GET");
/**
 * `@httpRoute` for `POST`.
 *
 * `@httpRoute` for `POST`: `@post(path, def)`; `def` carries `body`, `status` and the rest.
 *
 * @example
 * @httpController("orders")
 * class Orders {
 *   @post("/orders", { body: { sku: str() }, status: 201 })
 *   create({ args }: { args: { body: { sku: string } } }) { return { sku: args.body.sku }; }
 * }
 */
export const post = verb("POST");
/**
 * `@httpRoute` for `PUT`.
 *
 * @example
 * @httpController("orders")
 * class Orders {
 *   @put("/orders/:id", { body: { sku: str() } })
 *   replace() { return {}; }
 * }
 */
export const put = verb("PUT");
/**
 * `@httpRoute` for `PATCH`.
 *
 * @example
 * @httpController("orders")
 * class Orders {
 *   @patch("/orders/:id", { body: { sku: str() } })
 *   update() { return {}; }
 * }
 */
export const patch = verb("PATCH");
/**
 * `@httpRoute` for `DELETE` (`delete` is a reserved word).
 *
 * @example
 * @httpController("orders")
 * class Orders {
 *   @del("/orders/:id")
 *   remove() { return {}; }
 * }
 */
export const del = verb("DELETE");

function lineage(ctor: Function): Function[] {
  const chain: Function[] = [];
  for (let c: Function | null = ctor; typeof c === "function" && c !== Function.prototype; c = Object.getPrototypeOf(c) as Function | null) chain.unshift(c);
  return chain;
}

/**
 * The route group a decorated class declares. Routes of base classes come first; a subclass may redeclare one.
 *
 * @param ctor a class decorated with `@httpController`.
 * @throws {TypeError} When `ctor` has no `@httpController`, or a route definition is invalid.
 *
 * @example
 * const orders = routesOf(Orders);
 * Object.keys(orders.routes); // ["get", "create"]
 */
export function routesOf(ctor: Function): HttpContract {
  const info = lineage(ctor).map(c => controllers.get(c)).reverse().find(Boolean);
  if (!info) throw new TypeError(`${ctor.name || "class"} is not decorated with @httpController`);
  const map: RouteMap = {};
  for (const c of lineage(ctor)) for (const [key, def] of routes.get(c) ?? []) map[key] = def;
  return httpContract(info.name, map, { doc: info.doc });
}

/**
 * The handlers for an instance of a decorated class: each route method bound to `instance`.
 *
 * @param instance an instance of a class decorated with `@httpController`.
 * @throws {TypeError} When its class has no `@httpController`, or a declared route is no longer a method.
 *
 * @example
 * const handlers = routeHandlersOf(new Orders());
 * Object.keys(handlers); // the route names
 */
export function routeHandlersOf(instance: object): BoundRouteHandlers {
  const ctor = instance.constructor;
  const handlers: BoundRouteHandlers = {};
  for (const key of Object.keys(routesOf(ctor).routes)) {
    const fn = (instance as Record<string, unknown>)[key];
    if (typeof fn !== "function") throw new TypeError(`${ctor.name}.${key} is declared as a route but is not a method`);
    handlers[key] = (input, call) => fn.call(instance, input, call);
  }
  return handlers;
}
