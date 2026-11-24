# @coldmind/inject

Typed hierarchical dependency injection with synchronous fast paths, async construction, explicit lifetimes, modules and browser-safe core APIs.

```sh
bun add @coldmind/inject
# or: npm install @coldmind/inject
```

```ts
import { Container, createToken } from '@coldmind/inject';
const Message = createToken<string>('Message');
const container = new Container();
container.singleton(Message, () => 'Hello');
console.log(await container.resolve(Message));
await container.dispose();
```

Bun >=1.3 uses source TypeScript; Node >=22 uses compiled ESM/declarations. Compiled distribution is verified on Node22.0 and Node24; source/runtime suites also pass on Bun1.3.

| Entry | Purpose |
| --- | --- |
| `@coldmind/inject` | Container, tokens, decorators, modules, deferred dependencies |
| `/contracts`, `/token`, `/errors`, `/graph`, `/decorators`, `/container`, `/module`, `/lazy` | Explicit public API surfaces |
| `/server` | DiRuntime and server-only async context |
| `/testing` | Test roots and awaited overrides |
| `/hot`, `/inspect`, `/react` | Hot replacement, introspection, React adapter |
| `/internal/*` | Explicit implementation routes, not supported application APIs |

[Getting started](docs/getting-started.md) · [Lifetimes](docs/lifetimes.md) · [Async/cycles](docs/async-and-cycles.md) · [Modules](docs/modules.md) · [Testing](docs/testing.md) · [Decorators](docs/decorators.md) · [Server](docs/server.md) · [React](docs/react.md) · [Introspection](docs/introspection.md) · [Errors](docs/errors.md)

[Inversify migration](docs/migrate-inversify.md) · [TSyringe migration](docs/migrate-tsyringe.md) · [Nest wiring migration](docs/migrate-nest.md) · [API](docs/api/index.html)

Generate API reference with `bun run docs:api`. Run tests only in the final validation phase of the active delivery task.
