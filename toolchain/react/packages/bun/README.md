# Devtools Bun adapter

`@coldmind/devtools` connects Bun's native `onBeforeParse` hook to the shared
Rust engine in `crates/transform`. Bun still owns its bundler and dev-server
lifecycle; the adapter does not start Devtools's standalone Rust server.

```ts
import { devtools } from "@coldmind/devtools";

const plugin = devtools({ define: { __MODE__: JSON.stringify("development") }, inject: true });
const result = await Bun.build({ entrypoints: ["./src/main.ts"], plugins: [plugin] });
console.log(result.success, plugin.stats(), plugin.takeWarnings());
```

## Build and verification

From the product root, `packages/tools/devtools`:

```sh
bun run --filter @coldmind/devtools build:native
bun run --filter @coldmind/devtools typecheck
bun run --filter @coldmind/devtools test
bun examples/bun-transform/build.ts
```

The build explicitly enables the shared crate's `bun` feature and writes
`packages/bun/native/devtools.<platform>-<arch>.node`. Runtime-independent Rust
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

`@coldmind/devtools/tauri` exports `createTauriDevtools(invoke)`. Pass the host's
existing `@tauri-apps/api/core` invoke function; the client itself imports neither
Bun nor native bindings. The unified Rust plugin in `crates/tauri` registers
`plugin:devtools|transform`; windows must grant `devtools:default` or
`devtools:allow-transform`. It transforms supplied text, not files or executable code.

Implementation files are `src/bun-plugin.ts` and `src/tauri-client.ts`.
Existing package entry points and legacy source-path exports remain available.
