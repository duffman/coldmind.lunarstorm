# @coldmind/protocol

The Coldmind wire protocol, version 1: the six frame types, frame validation, codecs (JSON, MessagePack), length-prefixed
framing, and the two built-in contracts every server and client share, `auth` and `bus`. Structure only: a frame is an
object, bytes and carriers (WebSocket, TCP, HTTP) are outside it. The prose spec is `PROTOCOL.md` in this folder.

## Public API

`src/index.ts` exports five modules (`frames`, `codec`, `contracts`):

- Frames: `PROTOCOL_VERSION`, `frames.{call,cancel,result,error,chunk,event}`, `parseInFrame(raw)` (returns `{ frame }`
  or `{ reject: ErrorFrame }`), `parseOutFrame(raw)` (frame or `null`), types `Frame`, `InFrame`, `OutFrame`,
  `CallFrame`, `AuthSlot`, `FrameId`, `SID_CLEARED`.
- Codecs: `jsonCodec`, `msgpackCodec`, `encode`/`decode` (msgpack), `subprotocol(codec)`, `negotiate(codecs, offered)`,
  `decodeOr`, `pack(wire)` and `Unpacker` (4-byte big-endian length prefix), types `Codec`, `Wire`.
- Contracts: `authContract` (`login`, `logout`, `whoami`), `busContract` (`subscribe`, `publish`, `request`, `reply`, `ack`,
  `unsubscribe`), `busMessage`, `busChunk`, types `Principal`, `BusMessage`, `BusChunk`.

```ts
const wire = jsonCodec.encode(frames.call(1, "orders.get", { id: "42" }, { timeoutMs: 5000 }));
const parsed = parseInFrame(jsonCodec.decode(wire));
if ("reject" in parsed) send(jsonCodec.encode(parsed.reject));
```

## How it fits

Depends on `@coldmind/rpc` (contracts) and `@coldmind/model`. `@coldmind/net.client` depends on it and builds `Client`
and `RemoteBus` on these frames; servers use `parseInFrame` as their single input. The VCL (Delphi-style component
library, `@coldmind/vcl.*`) reaches the server only through `net.client`, so this is the contract under its
remote controllers. `doc/plans/main.md` does not cover it: the desktop bridge (spec 4d) has its own message shape.

## Scripts

- `pnpm --filter @coldmind/protocol typecheck` runs `tsc --noEmit -p tsconfig.json`.
- No `test` or `build` script, and no tests in the package.

## Decisions and hazards

- The bus is not a second protocol: it is the `bus` controller; a subscription is a `call` that stays open
  (`noTimeout: true`) and each message is a `chunk`.
- Error codes the server sends: `BAD_FRAME`, `UNSUPPORTED_VERSION`, `UNKNOWN_PROC`, `INVALID_ARGS`, `UNAUTHENTICATED`,
  `FORBIDDEN`, `AUTH_FAILED`, `CANCELLED`, `TIMEOUT`, `BUSY`, `INTERNAL`; the client adds `CLOSED`, `TRANSPORT`,
  `INVALID_RESULT` (`rpc.errors.ts`).
- Frame ids: safe integer, or string of 1 to 128 characters. `timeoutMs`: integer 1..2147483647, counted from
  receipt, can only shorten the server's limit.
- `sid` on a final frame: string = new reference, `null` = session gone, absent = unchanged. Clients apply it only if the
  call was sent under the current `sid`.
- Codec names travel as `subzero.v1+<codec>` (WebSocket subprotocol, TCP preamble `SUBZERO/1`); the `subzero` prefix is
  the protocol's former name and is part of the wire format.
- `Uint8Array` payloads do not survive `jsonCodec`; use `msgpackCodec` for binary. Both drop `undefined` members.
- `Principal.claims` stay on the server: `auth.whoami` and `login` return only `id` and `roles`.
- `authContract` is in `Client`'s contract table from construction; registering another contract named `auth` throws.
