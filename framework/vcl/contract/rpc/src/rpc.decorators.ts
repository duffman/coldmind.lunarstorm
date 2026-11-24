import { Action } from "@coldmind/wire/src/core/action.ts";
import { defineDecorator, type FieldWrap } from "@coldmind/rpc/src/rpc.decorate.ts";
import { contract, event, type Contract, type ContractOptions, type EventDef, type EventOptions, type ProcDef, type ProcMap } from "@coldmind/rpc/src/rpc.proc.ts";
import type { Schema } from "@coldmind/rpc/src/rpc.model.ts";

// ---- decorators: declare a controller as a class instead of a contract object plus a handlers object ----

type ControllerInfo = { name: string; doc?: string | undefined };

const controllers = new WeakMap<Function, ControllerInfo>();
const procs = new WeakMap<Function, Map<string, ProcDef>>();
/** One contract object per class: a client and a server identify a contract by that object. */
const contractCache = new WeakMap<Function, Contract>();
const eventCache = new WeakMap<Function, Record<string, EventDef>>();

/** Handlers as a server takes them: one function per method, bound to the instance. */
export type BoundHandlers = Record<string, (args: any, call: any) => unknown>;

/**
 * Marks a class as the controller `name`. Its `@rpcProc` methods become the contract's procs, named
 * `name.method`. Works with Bun's native decorators and with `experimentalDecorators`.
 *
 * @param name the controller name, the prefix of every proc.
 * @param opts `doc` for the controller as a whole.
 * @example
 * @rpcController("orders", { doc: "Order handling" })
 * class Orders {
 *   @rpcProc({ args: { sku: field.string() }, access: "public" })
 *   create({ sku }: { sku: string }) { return { sku }; }
 * }
 */
export const rpcController = defineDecorator("rpcController", (site, name: string, opts: ContractOptions = {}) => {
  if (site.kind !== "class") throw new TypeError("@rpcController can only decorate a class");
  controllers.set(site.target, { name, doc: opts.doc });
});

/**
 * Declares a method as a proc of its class's controller. The method receives `(args, call)` like any handler
 * and is validated against `def.args` before it runs. The method name is the proc's name.
 *
 * @param def the proc definition without its name: `args`, `result`, `access`, `doc` and the rest of `ProcDef`.
 * @throws {TypeError} When applied to a static method: handlers belong to an instance.
 */
export const rpcProc = defineDecorator("rpcProc", (site, def: ProcDef) => {
  if (site.kind !== "method") throw new TypeError("@rpcProc can only decorate a method");
  if (site.isStatic) throw new TypeError(`@rpcProc ${String(site.name)}: static methods cannot be procs`);
  if (typeof site.name !== "string") throw new TypeError("@rpcProc: symbol-named methods cannot be procs");
  const key = site.name;
  site.onClass(ctor => {
    const own = procs.get(ctor) ?? new Map<string, ProcDef>();
    own.set(key, def);
    procs.set(ctor, own);
  });
});

type EventInfo = { schema: Schema; opts: EventOptions };
const events = new WeakMap<Function, Map<string, EventInfo>>();

/**
 * Declares a field as a server-initiated event of its class's controller, named `controller.field`. The field
 * is an `Action<T>` created with the instance: the server publishes what it emits to every peer, and on the
 * client the same field is an action of the events received.
 *
 * @param schema the event's data, as in `event(name, schema)`.
 * @param opts `doc` for the event.
 * @throws {TypeError} When applied to a static or symbol-named field, or to anything but a field.
 * @example
 * @rpcController("orders")
 * class Orders {
 *   @rpcEvent({ id: field.string() }) readonly created!: Action<{ id: string }>;
 *   @rpcProc({ args: {}, access: "public" }) place() { this.created.emit({ id: "1" }); }
 * }
 */
export const rpcEvent = defineDecorator("rpcEvent", (site, schema: Schema, opts: EventOptions = {}): FieldWrap => {
  if (site.kind !== "field") throw new TypeError("@rpcEvent can only decorate a field");
  if (site.isStatic) throw new TypeError(`@rpcEvent ${String(site.name)}: static fields cannot be events`);
  if (typeof site.name !== "string") throw new TypeError("@rpcEvent: symbol-named fields cannot be events");
  const key = site.name;
  site.onClass(ctor => {
    const own = events.get(ctor) ?? new Map<string, EventInfo>();
    own.set(key, { schema, opts });
    events.set(ctor, own);
  });
  return { init: () => new Action() };
});

/** `@rpcController`: names a class as a controller, the server side of a contract. */
export const expose = rpcController;
/** `@rpcProc`: declares a method as a proc. */
export const procedure = rpcProc;

/** Classes from the base-most to `ctor`. */
function lineage(ctor: Function): Function[] {
  const chain: Function[] = [];
  for (let c: Function | null = ctor; typeof c === "function" && c !== Function.prototype; c = Object.getPrototypeOf(c) as Function | null) chain.unshift(c);
  return chain;
}

/**
 * The contract a decorated class declares. Procs of base classes come first; a subclass may redeclare one.
 *
 * @param ctor a class decorated with `@rpcController`.
 * @returns the contract, ready for `procsOf`, `describeContract` or a server's `controller()`.  The same object for every call: a client and a server identify a contract by it.
 * @throws {TypeError} When `ctor` has no `@rpcController`.
 */
export function contractOf(ctor: Function): Contract {
  const cached = contractCache.get(ctor);
  if (cached) return cached;
  const info = lineage(ctor).map(c => controllers.get(c)).reverse().find(Boolean);
  if (!info) throw new TypeError(`${ctor.name || "class"} is not decorated with @rpcController`);
  const map: ProcMap = {};
  for (const c of lineage(ctor)) for (const [key, def] of procs.get(c) ?? []) map[key] = def;
  const built = contract(info.name, map, { doc: info.doc });
  contractCache.set(ctor, built);
  return built;
}

/**
 * The handlers object for an instance of a decorated class: each `@rpcProc` method bound to `instance`.
 *
 * @param instance an instance of a class decorated with `@rpcController`.
 * @throws {TypeError} When its class has no `@rpcController`, or a declared proc is no longer a method.
 * @example
 * const orders = new Orders();
 * controller(contractOf(Orders), handlersOf(orders));
 */
export function handlersOf(instance: object): BoundHandlers {
  const ctor = instance.constructor;
  const handlers: BoundHandlers = {};
  for (const key of Object.keys(contractOf(ctor).procs)) {
    const fn = (instance as Record<string, unknown>)[key];
    if (typeof fn !== "function") throw new TypeError(`${ctor.name}.${key} is declared as a proc but is not a method`);
    handlers[key] = (args, call) => fn.call(instance, args, call);
  }
  return handlers;
}

/**
 * The events a decorated class declares, by field name, each named `controller.field`. Events of base classes
 * come first. `{}` when the class declares none.
 *
 * @throws {TypeError} When the class declares events but is not decorated with `@rpcController`.
 * @example
 * const { created } = eventsOf(Orders); // created.name === "orders.created"
 */
export function eventsOf(ctor: Function): Record<string, EventDef> {
  const cached = eventCache.get(ctor);
  if (cached) return cached;
  const chain = lineage(ctor);
  const own = chain.flatMap(c => [...(events.get(c) ?? [])]);
  if (own.length === 0) return {};
  const info = chain.map(c => controllers.get(c)).reverse().find(Boolean);
  if (!info) throw new TypeError(`${ctor.name || "class"} declares events but is not decorated with @rpcController`);
  const map: Record<string, EventDef> = {};
  for (const [key, e] of own) map[key] = event(`${info.name}.${key}`, e.schema, e.opts);
  eventCache.set(ctor, map);
  return map;
}

/** What every `@rpcServer` declares: the controllers. A server adds its own settings next to them. */
export type ServerShape = {
  /** Classes decorated with `@expose`. */
  controllers: readonly Function[];
  /** Controllers that stay in-process: provided, not exposed to network peers. */
  internal?: readonly Function[] | undefined;
};

const servers = new WeakMap<Function, ServerShape>();

/**
 * Declares a class as a server and keeps its options, whatever they are, for `serverShapeOf`. The browser-safe
 * half of `@Server` in `@coldmind/app`: the client reads which controllers a server has, and nothing else, so the
 * compiler can strip a server's settings (databases, adapters) from the client build and keep this call.
 *
 * @throws {TypeError} When applied to anything but a class.
 */
export const rpcServer = defineDecorator("rpcServer", (site, options: ServerShape) => {
  if (site.kind !== "class") throw new TypeError("@Server can only decorate a class");
  servers.set(site.target, options);
});

/**
 * The options a class declared with `@Server`.
 * @throws {TypeError} When `ctor` is not decorated with `@Server`.
 */
export function serverShapeOf<O extends ServerShape = ServerShape>(ctor: Function): O {
  const options = servers.get(ctor);
  if (!options) throw new TypeError(`${ctor.name || "class"} is not decorated with @Server`);
  return options as O;
}
