# @coldmind/net.client

The calling side of the Coldmind protocol: a `Client` that sends `call` frames over a `Link` (WebSocket, HTTP, JSON-RPC),
a `RemoteBus` that implements the same `IPubsub` as a local bus over the server's `bus` controller, and wrappers that
turn streams, events and topics into `Action`s from `@coldmind/wire`. No server code, no I/O beyond the links.

## Public API

`src/index.ts` also re-exports all of `@coldmind/rpc` and `@coldmind/protocol`, so one import is enough.

- Links: `wsLink(url, { codec, codecs })`, `httpLink(url)`, `jsonRpcLink(url, { sessionHeader, headersFor })`,
  `reconnectingLink(connect, { minDelayMs, maxDelayMs, maxQueued })`, `linkEndpoint(...)` (building block), type `Link`.
- `Client(link, { timeoutMs, auth, telemetry })`: `api(contract)`, `call(name, args, opts)`, `register`, `on(def, cb)`,
  `login`, `logout`, `whoami`, `state` / `onState` / `whenUp`, `sid`, `close`.
- `remoteOf(client, ControllerClass)` -> `Remote<C>`: the decorated class's procs as methods, its events as actions.
- `RemoteBus(client, opts)`, `busApi(bus, contract, opts)` (typed proc calls over the bus; idempotent procs go QoS 1).
- Actions: `topicAction`, `eventAction`, `stateAction`, `streamAction`, `SignalActionBridge`, `defineSignalTopic`,
  `createSignalObserver`, `toBusError`.

```ts
const client = new Client(reconnectingLink(() => wsLink("wss://host/ws")), { timeoutMs: 10_000 });
const notes = remoteOf(client, NotesController);
await notes.add({ text: "hi" });
notes.added.onData(note => render(note));
stateAction(client).onData(state => setBanner(state)); // "up" | "down" | "closed"
```

## How it fits

Depends on `@coldmind/{bus,protocol,rpc,wire}`. `@coldmind/vcl.app` depends on it: `provideClient` in
`vcl.client.ts` registers the `Client`, `ConnectionState` and one `remoteToken(Controller)` per server controller in the
kernel container, so a VCL form injects its remotes. Protocol and frames: `vcl/contract/protocol/PROTOCOL.md`. The spec
(`doc/plans/main.md`) covers the desktop form pipeline and has no section on this network layer.

## Scripts

- `pnpm --filter @coldmind/net.client typecheck` runs `tsc --noEmit -p tsconfig.json`.
- No `test` or `build` script, and no test files in the package.

## Decisions and hazards

- `Client` validates args before sending and results on arrival; async validators are left to the server
  (`ProcObj2` in `"local"` mode).
- With `reconnectingLink`, calls in flight when the connection drops fail with `CLOSED` ("Connection lost"); later calls
  wait in the link, up to `maxQueued`. A plain link that closes makes the client `closed` for good.
- A reply may only replace `sid` if the call was sent under the current `sid`, so a late reply cannot overwrite a newer one.
- `RemoteBus` re-subscribes after reconnect; QoS >= 1 subscriptions continue from the last unacknowledged message
  under `clientId`; QoS >= 1 publishes in flight are resent with the same message id. `retained` and the dead letter
  queue are local to the client.
- `remoteOf` throws `TypeError` for a class not decorated with `@Controller` and builds from the decorators; the
  server-only method bodies are expected to be stripped by the compiler.
- `wsLink` offers codecs as `subzero.v1+<codec>` subprotocols; the `subzero` name is the old protocol name, kept on the wire.
- `index.ts` imports siblings by the package's own name (`@coldmind/net.client/src/...`), resolved through `"./*": "./*"`.
- `signal.operation.ts` (timeouts, `milliseconds`: integer 1..2147483647) is internal and not exported.
