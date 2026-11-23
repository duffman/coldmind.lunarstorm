# Devermind Bun adapter

`@coldmind/devermind` connects Bun's native `onBeforeParse` hook to the shared
Rust engine in `crates/transform`. Bun still owns its bundler and dev-server
lifecycle; the adapter does not start Devermind's standalone Rust server.

```ts
import { devermind } from "@coldmind/devermind";

const plugin = devermind({ define: { __MODE__: JSON.stringify("development") }, inject: true });
const result = await Bun.build({ entrypoints: ["./src/main.ts"], plugins: [plugin] });
console.log(result.success, plugin.stats(), plugin.takeWarnings());
```

## Build and verification

From the product root, `packages/tools/devermind`:

```sh
bun run --filter @coldmind/devermind build:native
bun run --filter @coldmind/devermind typecheck
bun run --filter @coldmind/devermind test
bun examples/bun-transform/build.ts
```

The build explicitly enables the shared crate's `bun` feature and writes
`packages/bun/native/devermind.<platform>-<arch>.node`. Runtime-independent Rust
and Tauri hosts use the same crate without that feature. Cargo target-directory
configuration is respected; the build script does not assume a local target tree.

## Transform behavior

- `replace`: literal text replacement, including strings and comments.
- `define`: scope-aware global constants, with replacement values expressed as JavaScript source.
- `inject`: constructor-type injection metadata for decorated/marked classes, or all classes with `{ all: true }`.

Define and injection share a parse. Unsupported constructor parameter types
produce warnings rather than invented runtime tokens. `stats()` reads native
counters; `takeWarnings()` drains pending warnings. Bun invokes only the first
matching native hook per file, so combine stages in one plugin instance.

## Tauri frontend API

`@coldmind/devermind/tauri` exports `createTauriDevermind(invoke)`. Pass the host's
existing `@tauri-apps/api/core` invoke function; the client itself imports neither
Bun nor native bindings. The unified Rust plugin in `crates/tauri` registers
`plugin:devermind|transform`; windows must grant `devermind:default` or
`devermind:allow-transform`. It transforms supplied text, not files or executable code.

Implementation files are `src/bun-plugin.ts` and `src/tauri-client.ts`.
Existing package entry points and legacy source-path exports remain available.
