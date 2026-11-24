# wire: Action

A small, typed object for receiving, processing, and delivering data. One public `Action<T>` class, zero runtime dependencies, strict TypeScript, ES2022 ESM.

```ts
import { Action } from '@coldmind/wire';

const search = new Action<string>();

const queries = search
  .map(text => text.trim())
  .filter(text => text.length >= 2)
  .debounce(300)
  .distinct();

queries.onError(console.error);
queries.onData(query => console.log('Search:', query));

search.emit('Al').emit(' Alice ');
```

Each processing method creates a derived action. The source remains unchanged. `emit` always returns the receiving action; `onData`, `onError`, and `once` return idempotent unsubscribe functions.

## Run

From the repo root:

```sh
bun install
bun run typecheck
bun run demo            # packages/reactive/wire/examples/action.demo.ts
bun run demo:wire       # component + remote call + one trace: examples/wire/wire.rpc.demo.ts
bun run stage --report reports/stages.json
```

The wire uses standard ES2022, promises, timers, and AbortController. Event adapters accept a standard EventTarget. Consumers compiling its DOM event overloads need the TypeScript `DOM` library in `compilerOptions.lib`.

## Receive data

```ts
import { Action } from '@coldmind/wire';

interface User { id: number; name: string }

const users = new Action<User>();
const unsubscribe = users.onData(user => console.log(user.name));
users.onError(console.error);

users.emit({ id: 1, name: 'Alice' });
users.fromPromise(Promise.resolve({ id: 2, name: 'Bob' }));

users.fromCallback(emit => {
  const timer = setInterval(() => emit({ id: 3, name: 'Charlie' }), 1_000);
  return () => clearInterval(timer);
});

// At the end of this owner's lifetime:
unsubscribe();
users.dispose();
```

Register subscribers before attaching an instance source that can emit synchronously. The example's final disposal cancels the interval and ignores the pending promise if it has not settled yet. The full runnable example in `examples/action.demo.ts` waits for each demonstration before cleaning up.

Callback registration receives `(emit, error, signal)`. It returns a synchronous cleanup function or nothing. Its signal is aborted before its cleanup runs. Registration exceptions are sent to `onError`. Callback cleanup is still called if the action is disposed while registration is executing.

For browser events, known document, window, and HTML element event names infer the event type:

```ts
import { Action } from '@coldmind/wire';

const input = document.createElement('input');
const button = document.createElement('button');
button.textContent = 'Submit';
document.body.append(input, button);

const text = new Action<string>();
text.onData(console.log);
text.fromEvent(input, 'input', () => input.value);

const clicks = Action.fromEvent(button, 'click');
clicks.onData(event => console.log(event.clientX, event.clientY));

window.addEventListener('pagehide', () => {
  text.dispose();
  clicks.dispose();
}, { once: true });
```

Instance `fromEvent` requires a mapper producing `T`. Static `fromEvent` produces the event itself. Both accept `AddEventListenerOptions` including capture, passive, once, and signal. Arbitrary EventTargets use `Event` by default; an explicit subtype is a caller assertion, for example `Action.fromEvent<CustomEvent<User>>(target, 'user')`.

## Async processing

```ts
import { Action } from '@coldmind/wire';

const requests = new Action<string>();
const responses = requests.switchMap(async (url, signal) => {
  const response = await fetch(url, { signal });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.text();
});

responses.onError(console.error);
responses.onData(body => console.log(body));

const received = responses.next({ timeout: 5_000 });
requests.emit('data:text/plain,Hello%20Action');
await received;
requests.dispose();
```

`switchMap` aborts the preceding operation when a new input arrives. Superseded results and failures are ignored even if the producer ignores cancellation. Current-operation failures reach `onError` and do not end the action. Multiple subscribers share one operation per input. Unsubscribing the final observer aborts in-flight work.

`map` transforms synchronously. Returning a promise from `map` produces an `Action<Promise<...>>`; use `switchMap` to await it. `tap` preserves the original value. A synchronous tap exception drops that value on its branch; a returned promise is observed for rejection but does not delay delivery.

## One value and cancellation

```ts
import { Action } from '@coldmind/wire';

const values = new Action<number>();
const controller = new AbortController();

values.once(value => console.log('First:', value));
values.onData(console.log, { signal: controller.signal });

const pending = values.next({ signal: controller.signal, timeout: 1_000 });
values.emit(42);
console.log('Awaited:', await pending);

controller.abort();
values.dispose();
```

`next()` resolves the next value and detaches before user code resumes. It rejects with the actual source error, the abort signal's reason, `ActionTimeoutError`, or `ActionDisposedError`. Without a timeout or signal it waits until data, error, or disposal. A plain promise cannot be forcibly cancelled; disposal disconnects its delivery and releases references.

## Compose actions

```ts
import { Action } from '@coldmind/wire';

const saved = new Action<{ id: number }>();
const removed = new Action<{ id: number }>();
const changes = Action.merge(saved, removed);

changes.onData(value => console.log('Changed:', value.id));
saved.emit({ id: 1 });
removed.emit({ id: 2 });

const audit = new Action<{ id: number }>();
audit.onData(value => console.log('Audit:', value.id));
audit.connect(changes);
saved.emit({ id: 3 });

audit.dispose();
changes.dispose();
saved.dispose();
removed.dispose();
```

Merge infers the union of input payload types. `first.merge(second, third)` also works. Ending one input does not end the merged action; ending all inputs does. An empty merge is already disposed. Each supplied occurrence is a separate input, so merging the same action twice forwards twice.

`connect` owns the subscription, never the source. Disposing a connected source detaches that connection and leaves its receiver usable. Repeated calls create independent connections. Explicit direct or indirect connection cycles throw `ActionCycleError`; loops created manually inside user callbacks remain the caller's responsibility.

## API

| Method | Result | Behavior |
| --- | --- | --- |
| `new Action<T>(options?)` / `Action.create<T>(options?)` | `Action<T>` | Create a writable channel. |
| `emit(data)` | `this` | Deliver synchronously; `Action<void>` accepts `emit()`. |
| `onData(handler, options?)` | `Unsubscribe` | Observe future values. |
| `onError(handler, options?)` | `Unsubscribe` | Observe nonterminal failures. |
| `once(handler, options?)` | `Unsubscribe` | Detach before handling the first value. |
| `next({ signal?, timeout? }?)` | `Promise<T>` | Await one value; reject on failure or termination. |
| `fromPromise(promise)` | `this` | Attach a hot, one-shot input. |
| `fromCallback(register)` | `this` | Attach a callback input immediately. |
| `fromEvent(target, type, map, options?)` | `this` | Attach a mapped event input immediately. |
| `connect(source)` | `this` | Own a subscription to another action. |
| `map(transform)` | `Action<R>` | Transform values. |
| `filter(predicate)` | `Action<T>` / `Action<S>` | Keep matches; preserve type-guard narrowing. |
| `tap(effect)` | `Action<T>` | Observe a value without transforming it. |
| `debounce(ms)` | `Action<T>` | Deliver the latest value after quiet time. |
| `distinct(equals?)` | `Action<T>` | Skip consecutive duplicates; default `Object.is`. |
| `switchMap(asyncMapper)` | `Action<R>` | Cancel preceding async work; deliver only current results. |
| `merge(...others)` | `Action<T \| Others>` | Merge inputs with inferred union types. |
| `dispose()` | `void` | Idempotently end this action and release owned resources. |
| `isDisposed` | `boolean` | Read terminal state. |

| Static factory | Startup | Replay |
| --- | --- | --- |
| `Action.fromPromise(promise, options?)` | Promise handlers attach immediately. | Retains that promise's fulfillment or rejection for late observers until disposal. A rejection with no error handler attached at settlement goes to `onUnhandledError`. |
| `Action.fromEvent(target, type, options?)` | First observer activates the event listener. | None. |
| `Action.fromCallback(register, options?)` | First observer invokes registration. | None. |
| `Action.merge(...sources)` | First observer connects the inputs. | No additional buffer; source replay can occur at activation. |

## Delivery and lifetime

Plain actions are hot and do not replay. Static promise factories are the exception; later manual emissions do not replace their retained promise settlement. A retained rejection that settles before any error handler is attached is reported once through `onUnhandledError` and remains available to later `onError` and `next()` observers. Derived actions do not add replay. A promise-backed action stays open after settlement and can still be used as a regular action.

Derived pipelines and static event/callback sources are lazy and shared. Both data and error subscriptions count as observers. The last unsubscribe tears down upstream subscriptions, timers, and active operations; the next subscription starts a fresh activation, including fresh debounce/distinct state. An error-only subscription can activate a synchronous source, so values emitted before a subsequent data subscription are not buffered. `next()` installs its value, error, and disposal handling atomically.

Emission snapshots subscriptions in registration order. Removing a subscription skips it even if delivery has started; a subscription added during an emission does not receive that emission. Duplicate handler functions remain independent subscriptions. Reentrant emits on the same action queue FIFO and finish before the outermost `emit` returns. A recursive `emit` call itself returns after enqueueing; its delivery waits for the current value to finish. This prevents stack overflow but does not make an intentionally infinite feedback loop safe.

Every returned derived value is still an `Action`. Manually calling `emit` on one delivers an already-processed value to that action and its descendants; it does not run its incoming transform or emit upstream.

Disposing an action removes its inputs and observers, cancels its timers and operations, clears retained values, rejects pending waits, and disposes its currently active derived pipelines. Idle derived pipelines discover parent disposal on their next activation. Disposing a child never disposes a shared parent. Calling methods on a disposed action throws `ActionDisposedError`, except `dispose` is idempotent, `isDisposed` remains readable, and `next` returns a rejected promise. Static merge can include disposed inputs; instance merge requires a live receiver.

Keep an action for each ownership boundary and dispose it when that owner ends. Use AbortSignal for groups of subscriptions that have a shorter lifetime. Data is passed by reference; the package neither clones values nor performs runtime schema validation. A slow synchronous handler delays `emit`. This API does not provide backpressure for asynchronous consumers.

## Errors

Errors do not close an action. Source and operator errors reach downstream `onError` subscribers. Operator failures affect only that branch. A handler throwing does not stop other data handlers. Returning a promise from a data, error, or tap handler never creates an unobserved rejection.

Use `new Action<T>({ onUnhandledError })` to replace the default console diagnostic. Derived actions inherit their source's diagnostic configuration; merge inherits its first input's configuration. Error-handler failures go to that diagnostic sink without recursively calling error handlers. A failing diagnostic handler falls back to `console.error`.

Disposal detaches retained async handler results from the action graph. A handler promise that rejects **after disposal** uses the standalone console fallback; it cannot call disposed handlers or keep their custom diagnostic closures alive. Superseded/disposed source promises and `switchMap` operations instead ignore both late fulfillment and late rejection.

Timer arguments must be integers from 0 through 2147483647. Zero debounce still schedules asynchronously. Pending debounce values are discarded on unsubscribe or disposal. Cleanup callbacks execute once, in registration order; synchronous cleanup errors are reported while disposal continues.

## Logging, tracing, and measurements

```ts
import { Action, ActionTelemetry } from '@coldmind/wire';

const telemetry = new ActionTelemetry({
  minLevel: 'debug',
  maxMetrics: 256,
  onLog: record => console.log(JSON.stringify(record)),
  onSpan: span => console.log(JSON.stringify(span)),
  onMetric: metric => console.log(JSON.stringify(metric)),
  onDiagnostic: error => console.error('Telemetry exporter failed', error),
});

const input = new Action<number>({ name: 'prices', telemetry });
const doubled = input.map(value => value * 2);
doubled.onData(value => console.log(value));
input.emit(21);
input.dispose();
console.table(telemetry.metrics());
```

Tracing is opt-in per owner/action. Derived actions inherit telemetry and gain operator names. Publication spans measure synchronous delivery, including downstream work. `switchMap.work` spans measure asynchronous completion or cancellation. Debounce and switchMap preserve the originating trace across their asynchronous boundary. A `Client` created with `telemetry` and a `Server` with `telemetryAdapter` share the trace ID and link parent span IDs; see the guide.

Pass explicit parent contexts when starting independent async work. There is no ambient global context or AsyncLocalStorage dependency:

```ts
const root = telemetry.startSpan('import');
try {
  await telemetry.measure('parse', async span => {
    span.log('info', 'Parsing started', { format: 'json' });
    return JSON.parse('{"count":42}') as { count: number };
  }, { parent: root.context });
  root.end();
} catch (error) {
  root.end('error');
  throw error;
}
```

Metrics retain count, errors, cancellations, total, minimum, maximum, and mean duration per name. Unique metric names are bounded; `droppedMetrics` counts measurements of additional names. Long span names receive a deterministic hash suffix within 256 characters. `resetMetrics()` clears aggregates. Span and log history is streamed to your callbacks, never retained internally. Sink exceptions and promise rejections cannot replace an operation's result.

Payloads are not captured automatically. Values explicitly supplied to `log()` are your responsibility. Parent/child synchronous durations overlap; adding all operator durations double-counts nested work. `bun run bench` prints measured throughput and p50/p95/p99 round-trip latency for this environment. Instrumentation allocates spans and random IDs, so enable it where you need diagnostics.

## Component factory

```ts
import { actionPort, defineActionComponent } from '@coldmind/wire';

const multiply = defineActionComponent({
  name: 'multiply',
  inputs: { value: actionPort<number>() },
  outputs: { result: actionPort<number>() },
  setup(ctx) {
    ctx.outputs.result.connect(ctx.inputs.value.map(value => value * 2));
  },
});
const component = multiply.create({ telemetry });
component.outputs.result.onData(value => console.log(value));
await component.start();
component.inputs.value.emit(21); // result emits 42
await component.destroy();
```

Use this factory for most components. `actionPort<T>()` declares topology and value types; every `create()` makes fresh Action instances. Ports remain ordinary Actions, compatible with all existing operators, sources, and connections. Configure a component before starting input producers.

`ctx.own(cleanup)` owns a subscription, timer, or other resource. `ctx.action<T>(name)` creates another owned channel. `ctx.resource(acquire, dispose)` owns acquired resources and disposes acquisitions that complete after destruction. `ctx.signal` cancels external work. Setup may return a cleanup function or its promise.

Choose operators according to semantics: `switchMap` cancels superseded work and suits search/preview requests. It can discard a write response after the server has committed the write; use explicit request sequencing for side-effecting commands. The component base does not impose a scheduling policy.

## Class template

```ts
import { ActionComponent, actionPort } from '@coldmind/wire';
import type { ActionComponentContext, ActionLifecycleOptions } from '@coldmind/wire';

class MultiplyComponent extends ActionComponent<{ value: number }, { result: number }> {
  readonly factor: number;
  constructor(factor: number, options: ActionLifecycleOptions = {}) {
    if (!Number.isFinite(factor)) throw new TypeError('factor must be finite');
    super({ name: 'multiply', inputs: { value: actionPort<number>() }, outputs: { result: actionPort<number>() } }, options);
    this.factor = factor;
  }
  protected override configure(ctx: ActionComponentContext<{ value: number }, { result: number }>): void {
    ctx.outputs.result.connect(ctx.inputs.value.map(value => value * this.factor));
  }
}
```

Use a class for shared behavior or instance configuration. Override `configure()`; it runs during `start()`, after subclass fields exist. The copy-ready class is `examples/components/multiply.component.ts`.

Owners expose `state`, `signal`, `start()`, and `destroy()`. Lifecycle callbacks are exactly `onCreate`, `onStart`, `onError`, and `onDestroy`; they are observers, not resource setup hooks. Their returned promises are observed for rejection but not awaited as part of startup/teardown. Operational work belongs in component setup. Startup and destruction are idempotent. Destroyed owners cannot restart. Teardown aborts first, then attempts all cleanup in reverse order within a shared budget; cleanup failures reject destruction with AggregateError after other cleanup is attempted.

## Remote calls, topics and events

The wire has no client or server of its own. Remote calls go through `Client` and `Server` (`@coldmind/net.client`, `@coldmind/net.server`), and everything that streams out of a client is available as an `Action`:

```ts
import { topicAction, eventAction, stateAction, streamAction } from '@coldmind/net.client';

topicAction<Price>(bus, 'price/+')           // a subscription on a local bus or a RemoteBus
eventAction(client, counterChanged)          // a server event
stateAction(client)                          // "up" | "down" | "closed"
streamAction<Row>((onChunk, signal) => api.export({}, { onChunk, signal }))   // chunks of a streamed call
```

All four are lazy and shared: the subscription or call exists while the action has observers. Typed topics with a payload parser, lifecycle and telemetry are `defineSignalTopic` + `SignalActionBridge`. See [the network guide](../../net/README.md#actions-on-the-network).

## Stage harness

`@coldmind/wire-stage` provides `defineStage`, typed setup rigs, named resources/actions, trace snapshots, assertions, cancellation and automatic teardown. See [the stage README](../wire-stage/README.md).

## Files

| File | Responsibility |
| --- | --- |
| `core/action.ts` | Public Action class and typed factories. |
| `core/action.core.ts` | Observers, ordered dispatch, diagnostics, replay, lifecycle, dependency graph. |
| `core/action.scope.ts` | Resource ownership and idempotent cleanup. |
| `core/action.sources.ts` | Event, promise, and callback adapters. |
| `core/action.operators.ts`, `core/action.operation.ts` | Lazy processing, cancellation, connections, and merge. |
| `telemetry/action.telemetry.ts` | Logs, spans, trace contexts, metrics. |
| `component/action.lifecycle.ts`, `component/action.component.ts` | Owners, ports, component factory and class template. |
| `core/action.types.ts`, `core/action.errors.ts` | Public contracts, named errors. |
