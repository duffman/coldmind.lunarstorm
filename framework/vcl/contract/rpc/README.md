# @coldmind/rpc

The procedure layer of the protocol, on `@coldmind/model`: contracts (named sets of procs with args, result, chunk, access
and idempotency), events, `ProcObj2` (the router every call goes through), `RpcError` with validation helpers, contract
introspection, and a decorator toolkit (`@expose`, `@procedure`, `@rpcEvent`) that declares a contract on a class and
works under both TypeScript decorator dialects. No transport, no server.

## Public API

`src/index.ts` re-exports six modules:

- Contracts: `contract(name, procs, opts)`, `procsOf`, `event(name, schema)`, `ProcObj2(procs, onCall, "full" | "local")`,
  types `Contract`, `ProcDef`, `TProc`, `Access` (`"public" | "authenticated" | { roles }`), `EventDef`, `ArgsOf`,
  `InputArgsOf`, `ResultOf`, `ChunkOf`.
- Validation: `validate`, `validateAsync`, `validateLocal`, `needsAsync`, `ValidationFailed`; `parseWith`,
  `parseWithAsync`, `parseLocal` (failures become `RpcError` with `data.errors`), `validationErrors(error)`.
- `RpcError(code, message, data?)`.
- Introspection: `describeContract`, `describeProc`, `describeSchema`, `describeEvent`.
- Decorators: `defineDecorator(name, impl)`, `expose` (= `rpcController`), `procedure` (= `rpcProc`), `rpcEvent`,
  `rpcServer`, `contractOf`, `handlersOf`, `eventsOf`, `serverShapeOf`, `strayMembers`.

```ts
@expose("orders")
class Orders {
  @rpcEvent({ id: str() }) readonly created!: Action<{ id: string }>;
  @procedure({ args: {}, access: "public" }) place() { this.created.emit({ id: "1" }); }
}
contractOf(Orders).procs;        // { place }
eventsOf(Orders).created.name;   // "orders.created"
```

## How it fits

Depends on `@coldmind/{core,model,wire}`. Used by `@coldmind/protocol` (the `auth` and `bus` contracts are `contract(...)`),
`@coldmind/http-routes` (routes convert to these contracts) and `@coldmind/net.client` (`Client.api(contract)`,
`remoteOf`, `busApi`); `@coldmind/vcl.app` imports it for its remote controllers. For the VCL (Delphi-style component
library) this is how a form's server side is declared: the same decorated class gives the server its handlers and the
client its typed `Remote`. `doc/plans/main.md` has no section on it.

## Scripts

- `pnpm --filter @coldmind/rpc typecheck` runs `tsc --noEmit -p tsconfig.json`.
- No `test` script in `package.json`, although `vitest.config.ts` (via `@coldmind/config/vitest`) and
  `tests/decorate.test.ts` exist; `pnpm test:js` at the root (`pnpm -r --if-present test`) therefore skips this package.
  Run with `pnpm --filter @coldmind/rpc exec vitest run`.

## Decisions and hazards

- `defineDecorator` hides the two dialects: standard (class: `(value, context)`) and legacy `experimentalDecorators`
  (class: `(ctor)`, method: `(target, key, descriptor)`); the implementation sees one `DecoratorSite`. The legacy test
  runs `tests/legacy/legacy.fixture.ts` through `framework/scripts/run.script.mjs` in a child process (60 s timeout).
- Standard member decorators run before their class exists; `site.onClass` defers to the class decorator. A class with
  member decorators but no class decorator never fires it: `strayMembers()` lists those.
- A plain field cannot be wrapped under standard decorators: wrapping needs `accessor` (error mentions it); a plain field may
  only get an initial value.
- `access` defaults to `"authenticated"`; `"public"` must be written. `idempotent` procs go QoS 1 over the bus
  (retried, then dead-lettered); `noTimeout` exempts a subscription from call timeouts.
- `ProcObj2` throws on duplicate proc names; its index is cached per proc list in a `WeakMap`.
- `"local"` validation skips async validators: the caller does not own the data, the server re-runs them.
- Error codes of the neutral set are documented in `rpc.errors.ts`; handlers may throw `RpcError` with their own.
- Every `src` file imports siblings by the package's own name through `"./*": "./*"`.
