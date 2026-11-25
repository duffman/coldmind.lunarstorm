# The event loop

Source: `runtime/core/src/runtime.rs` (the loop), `io.rs` (sockets, timers), `pool.rs` (blocking
work), `js/bootstrap.js` (the JS side of every event).

One `Runtime<V, H>` is one thread, one VM instance and one mio reactor. JS runs on that thread
only, one callback at a time. Blocking work goes to a small thread pool, whose results come back
through the reactor's waker.

## One step (`Turn::step`, runtime.rs:1022)

```
step
 ├─ 0. stop?        worker close()/terminate() flag → Idle
 ├─ 0. debugger     Instance::debug_service(); a client "stop" → Idle
 ├─ 1. timers       every due timer, one JS entry each (run_timers)
 ├─ 2. poll         sleep until: next timer | I/O | waker   (or Idle if nothing can wake us)
 ├─ 3. I/O          accepts, reads, write flushes, pool results (on_event)
 ├─ 4. posted       Poster tasks, then lunarstorm:ipc messages, then worker messages/exits
 └─ 5. immediates   setImmediate callbacks queued before this phase
```

A **JS entry** is every call into JS from the loop: `fire` (runtime.rs:1150), a posted task,
an IPC message or an immediate. Each entry runs the same bracket:

```
entry()  → debugger may hold it (break on start)
beat()   → pulse odd (only when H::PULSE)
call JS
settle() → nextTick queue, then all promise jobs, repeated until both are empty;
           out-of-memory check before and after; debugger checkpoint;
           each still-unhandled rejection → Host::uncaught
beat()   → pulse even
drain_deferred() → close/drain/resume events queued by ops while JS ran
```

That is Node's ordering: `process.nextTick` callbacks before promise jobs, and both drained
after every callback, not once per phase.

## When the loop is idle

Phase 2 decides (runtime.rs:1042):

| condition | poll timeout |
|---|---|
| an immediate or a posted task is queued | zero |
| a timer is armed | until its deadline |
| `io.alive()` (a handle is open or a pool job is pending), or `held`, or `refs > 0` | forever |
| none of the above | return `State::Idle` |

Three things keep a loop alive besides sockets and timers:
- **`hold(true)`**. A host whose events arrive as posted tasks sets this, so the loop sleeps in
  the kernel instead of reporting idle. The desktop host does.
- **`Shared::refs`**. Native references: an `onMessage` receiver of `lunarstorm:ipc`, and one per
  worker until its exit is delivered.
- **`pool.pending`**. An fs or DNS job in flight.

`run()` loops `step` until it returns `Idle`, then calls `stop()`. The return value is
`process.exitCode`.

## Performance decisions you must not undo

All of these are written next to the code; the numbers come from the comments.

| decision | where | why |
|---|---|---|
| one JS entry point, `dispatch(kind, id, a..e)` with integer codes | `ev` runtime.rs:151, bootstrap.js:1344 | no per-event closures, allocations or handle lookups |
| one 64 KiB scratch read buffer for every socket | `SCRATCH` runtime.rs:137 | an HTTP request that arrives whole is parsed in place |
| short read ⇒ socket drained, skip the `WouldBlock` read | `tcp_read`, `http_pump` | one syscall less per read; off after the peer's close |
| `POLL_GAP` = 250 µs | runtime.rs:147, `poll_due` | a zero-timeout kernel poll costs about 13 µs on macOS; with work queued the loop polls at most every 250 µs |
| coalesced wakes | `Poster::post`, `Nudge`, pool `notified` | a burst of posts or results costs one wake syscall |
| sockets registered once for `READABLE \| WRITABLE` | `Io::adopt` | edge-triggered; no `reregister` per response |
| write first, buffer only the tail | `Conn::send` | one `writev`; copies only what the kernel refused |
| pipelined responses corked into one write | `flag::CORKED` | |
| stdout buffered per loop turn | `Io::print`, flushed in phase 2 | one host write per turn, not per `console.log` |
| `run()` enters the VM scope once | `Runtime::run` | a host with its own loop pays one scope entry per `poll` |
| glue compiled once per process | `GLUE_IMAGES` runtime.rs:93 | it was 150 ms per Runtime in debug builds (64 workers: 15 s → 0.35 s) |

## HTTP inside the loop

HTTP/1.1 is not built on `net` in JS. The reactor parses requests in Rust
(`lunarstorm-http1`, `crates/http1`). An HTTP connection has no JS object until a request
arrives. Then `dispatch(HTTP, listener, conn, method, url, fields, body)` builds
`IncomingMessage`/`ServerResponse` in JS, and the response goes out through `ops.httpRespond`,
or through `httpHead`, `httpChunk` and `httpEnd` when streamed.

- `flag::BUSY`: a request is with JS. The socket is not read while busy, so TCP flow control
  slows the client instead of this process buffering without bound.
- An async handler finishes outside the pump. `http_done` then queues `Deferred::Resume` so the
  pipelined requests behind it are served.
- No `request` listener means a 404 is sent at once, so the client is never left hanging
  (bootstrap.js, `case 8`).

## Handles and timers

- **One slab** (`Io::entries`) holds listeners, connections and pending connects. The slab key
  is the mio token and the handle id JS sees (`handles[key]` in bootstrap.js).
- **Tombstones**: a connection the reactor closed becomes `Entry::Closed` until JS has been told,
  so a stale event cannot hit a reused key.
- **Timer ids**: `key * 2^32 + seq` (`TIMER_SHIFT`, mirrored in bootstrap.js). A stale id never
  cancels a reused slot. At most 2^21 timers can be armed (`TIMER_KEYS`). An interval re-arms at
  `now + period`, so a slow callback delays the next run instead of making runs pile up.

## Blocking work: the pool (`pool.rs`)

- Threads start lazily, up to the core count, named `lunarstorm-worker-N`.

  > Not to be confused with `lunarstorm:worker`, which is JS on its own VM; see hosts.md.

- `ops.fs(op, path, data, req)`. With `req == 0` it runs inline (the `*Sync` API). Otherwise it
  queues `Work::Fs`, and the result arrives later as `dispatch(FS, req, value | undefined, message, code)`.
- DNS for `connect` to a name is a pool job (`Work::Resolve`). Address literals and `localhost`
  connect at once.
- Nothing is dropped: past the bounded queue (256), jobs wait in a reactor-side overflow queue.

## Embedding the loop

| call | for |
|---|---|
| `run()` | hosts whose only loop is this one (CLI) |
| `poll(Wait::Block)` | one step, sleeping; the debugger's `lunar debug` loop |
| `poll(Wait::NoBlock)` | hosts that interleave their own loop |
| `post(task)` / `poster()` | run a closure on the loop thread from any thread |
| `call(export, &[Arg])` + `result(id)` | call an export of the entry module; objects come back as JSON text |
| `hold(bool)` | keep a loop with no handles alive for posted work |
| `pulse()` + `interrupt()` | a watchdog thread reads the pulse; an odd value twice, a budget apart, means one entry ran too long |

## Measured (runtime/bench/results.txt, Apple M1)

| | lunarstorm | node 24 | bun 1.4 |
|---|---|---|---|
| http req/s | 91–93 k | 83–91 k | 101 k |
| http pipelined req/s | 234–236 k | 163 k | 145 k |
| server RSS after http runs | 2–3 MiB | 88 MiB | 68 MiB |
| promise jobs/s | 2.5 M | 21 M | 27 M |
| nextTick/s | 4.2 M | 22 M | 54 M |
| setImmediate turns/s | 77 k | 77 k | 2.4 M |
| timers/s | 1.4 M | 3.9 M | 4.2 M |

I/O-bound work is on par or better and costs 30–40x less memory. Anything that churns
microtasks is 8–10x slower than V8 or JSC. That is the interpreter, not the loop.
