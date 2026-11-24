import { Field, introspect, introspectField, isModel } from "@coldmind/model";
import type { FieldInfo, ShapeInfo, ShapeInput } from "@coldmind/model";
import type { Schema } from "@coldmind/rpc/src/rpc.model.ts";
import { procsOf, type Access, type Contract, type EventDef, type TProc } from "@coldmind/rpc/src/rpc.proc.ts";

// ---- describe: contracts, procs and events as plain data, docs included ----
//
// The texts come from `doc` on procs, contracts and events, and from `.doc()` on the fields of
// args, result, chunk and event data. Everything else a doc page, a CLI help screen or a form
// needs (kinds, required, defaults, constraints) comes from `@coldmind/model`'s `introspect`.

/** A schema slot as data: a shape or model, a single field, or a `ParseSchema` that cannot be looked into. */
export type SchemaInfo =
  | { readonly form: "shape"; readonly shape: ShapeInfo }
  | { readonly form: "field"; readonly field: FieldInfo }
  | { readonly form: "opaque" };

/** One proc as data, for doc pages, help screens and forms. */
export type ProcInfo = {
  /** `controller.method` */
  readonly name: string;
  /** `""` when none, like `FieldInfo.doc`. */
  readonly doc: string;
  /** As declared; `"authenticated"` when left out. */
  readonly access: Access;
  readonly idempotent: boolean;
  readonly noTimeout: boolean;
  readonly args: SchemaInfo;
  readonly result?: SchemaInfo;
  readonly chunk?: SchemaInfo;
};

/** A contract as data: its procs keyed by method, in declaration order. */
export type ContractInfo = {
  readonly name: string;
  readonly doc: string;
  /** Keyed by method. */
  readonly procs: Readonly<Record<string, ProcInfo>>;
  /** Declaration order of the methods. */
  readonly order: readonly string[];
};

/** An event as data. */
export type EventInfo = {
  readonly name: string;
  readonly doc: string;
  readonly data: SchemaInfo;
};

/** A schema slot as data; a `ParseSchema` is opaque. */
export function describeSchema(schema: Schema): SchemaInfo {
  if (schema instanceof Field) return { form: "field", field: introspectField(schema) };
  if (isModel(schema) || typeof (schema as { parse?: unknown }).parse !== "function")
    return { form: "shape", shape: introspect(schema as ShapeInput) };
  return { form: "opaque" };
}

/** One proc as data, defaults filled in: access "authenticated", idempotent and noTimeout false. */
export function describeProc(proc: TProc): ProcInfo {
  return {
    name: proc.name,
    doc: proc.doc ?? "",
    access: proc.access ?? "authenticated",
    idempotent: proc.idempotent === true,
    noTimeout: proc.noTimeout === true,
    args: describeSchema(proc.args),
    ...(proc.result === undefined ? {} : { result: describeSchema(proc.result) }),
    ...(proc.chunk === undefined ? {} : { chunk: describeSchema(proc.chunk) }),
  };
}

/**
 * A contract as data, docs included.
 * @example
 * const info = describeContract(orders);
 * info.procs.create.doc               // "Places an order."
 * info.procs.create.args              // { form: "shape", shape: { fields: { sku: { doc: "Article number", … } } } }
 */
export function describeContract(c: Contract): ContractInfo {
  const procs: Record<string, ProcInfo> = {};
  const order: string[] = [];
  for (const proc of procsOf(c)) {
    const method = proc.name.slice(c.name.length + 1);
    procs[method] = describeProc(proc);
    order.push(method);
  }
  return { name: c.name, doc: c.doc ?? "", procs, order };
}

/** An event as data. */
export function describeEvent(def: EventDef): EventInfo {
  return { name: def.name, doc: def.doc ?? "", data: describeSchema(def.data) };
}
