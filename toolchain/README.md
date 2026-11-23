<p align="center">
  <img src="res/logo.png" alt="Devermind logo" width="280" />
</p>

# Devermind

**A Rust-first development server and frontend build tool—with a CLI, an embedded
Tauri host, native Bun transforms, and an optional React development shell.**

Devermind serves source modules during development, watches the dependency graph
for changes, and produces bundled assets for production. Its Rust workspace owns
the compiler integration, configuration, server, and bundler. The CLI and runtime
adapters are ways to use the same product, not separate implementations.

**Status:** active development, with source-based installation. The Rust workspace
can be built directly. JavaScript packages still depend on parts of the enclosing
monorepo's development setup; standalone JavaScript installation is not yet ready.
Package names below identify source packages, not published registry releases.

## What it does

- **Develop:** compile JavaScript, TypeScript, and JSX modules on demand; resolve
  imports; serve HTML and assets; watch files and send hot updates.
- **Build:** bundle application entries, emit content-hashed JavaScript/CSS/assets,
  and copy public files.
- **Configure:** read `.devermind/config.toml` directly in Rust, without executing
  a JavaScript configuration file.
- **Embed:** mount the development frontend on an application's existing HTTP
  gateway instead of starting another listener.
- **Transform:** share text replacement, constant-definition, and constructor
  injection-metadata transforms between Rust, Bun, and Tauri hosts.
- **Wrap React applications:** provide error handling, a development overlay,
  connection status, and shared HMR/application messaging.

## Getting started

### Build the CLI

Use the Rust toolchain specified in [`rust-toolchain.toml`](rust-toolchain.toml).
From a checkout containing this source tree:

```sh
cargo install --locked --path crates/cli
devermind --help
```

Alternatively, run directly from the workspace:

```sh
cargo run --locked -p devermind-cli -- dev /path/to/app
```

Your application root needs an `index.html` with a module entry, for example:

```html
<div id="app"></div>
<script type="module" src="/src/main.ts"></script>
```

Install the application's own dependencies with its usual package manager.
Devermind does not install them for you.

### Develop, build, preview

```sh
devermind dev /path/to/app --port 5173
devermind build /path/to/app
devermind preview /path/to/app --port 4173
```

| Command | Purpose |
| --- | --- |
| `dev [root]` | Serve source modules, watch files, and send hot updates. |
| `build [root]` | Produce a production build; `--out DIR` and `--no-minify` override configuration. |
| `preview [root]` | Serve an existing build on loopback, including SPA routes. It does not build first. |
| `transpile <file\|->` | Compile one source module to JavaScript without bundling; write to stdout or `--out FILE`. |

The root defaults to the current directory. `devermind [root]` is shorthand for
`devermind dev [root]`. Development defaults to `127.0.0.1:5173`; preview defaults
to `127.0.0.1:4173`.

**Builds replace the selected output directory.** Keep output separate from source
files. Preview is a local verification server, not a production hosting service.

### Standalone TypeScript transpilation

Use the compiler without starting a server or bundling an application:

```sh
devermind transpile src/main.ts
devermind transpile src/App.tsx --out dist/App.js --sourcemap
cat src/main.ts | devermind transpile - --loader ts
devermind help transpile
```

The command handles TS, TSX, JSX, and JavaScript. File syntax is inferred from its
extension; stdin defaults to TypeScript. Output is JavaScript, with imports and CommonJS module syntax preserved.
There is no type-checking, declaration generation, dependency bundling, or loading
of `.devermind/config.toml`/Vite configuration for this operation.

Stdout contains JavaScript only. `--sourcemap` embeds a map on stdout, or writes
`<output>.map` when `--out` selects a file. Input files are protected against
accidental overwrite, including source/output aliases. See the [CLI reference](docs/cli.md)
for flags, errors, and pipeline usage.

## Native configuration

Create `.devermind/config.toml` in your project directory:

```toml
[server]
host = "127.0.0.1"
port = 5173

[build]
outDir = "dist"
minify = true

[resolve.alias]
"@" = "./src"

[define]
__APP_NAME__ = '"My app"'
__FEATURE_ENABLED__ = true
```

An optional top-level `root` selects the application directory containing
`index.html`. Relative roots and filesystem aliases resolve from the project
directory; output paths resolve from the effective application root.

Explicit CLI or host overrides win over configuration. Invalid native
configuration fails with an error rather than silently falling back. Restart a
running server after changing configuration.

String definitions contain **JavaScript expressions**, which is why the string
literal above includes its own quotes. See the [configuration reference](docs/configuration.md)
for supported settings, validation, and precedence.

## Ways to use Devermind

### Rust and Cargo

`crates/cli` owns the single `devermind` executable. Rust applications can instead
use `devermind_server` for development hosting, `devermind_bundler` for production
builds, or `devermind-native` for source transforms. The transform crate defaults
to runtime-independent Rust; Bun bindings are an explicit feature.

`devermind_core::{compile_source, compile_file}` with `CompileOptions::standalone()`
also expose the single-file transpiler directly to Rust callers, without the CLI
or server.

Standalone hosts retain a `DevServer` handle for the server's lifetime. Embedded
hosts retain an `Embedded` server and mount its router on their own listener.
See [architecture and ownership](docs/architecture.md).

### Tauri

`tauri-plugin-devermind` combines permission-controlled source transforms with an
optional embedded development frontend. Enable `dev-server` and configure
`Builder::development(frontend_root)` to prepare that frontend.

The host registers the plugin and mounts `development_router(app_handle)` on its
gateway. **The plugin does not start a second HTTP listener.** Production frontend
building remains a separate CLI or bundler step; adding a Cargo dependency alone
does not configure an application's build hooks.

Transform IPC requires the webview capability `devermind:default` or
`devermind:allow-transform`. The command transforms supplied source text, not
arbitrary filesystem paths. Its frontend client is exported from
`@coldmind/devermind/tauri`; see the [adapter guide](packages/bun/README.md#tauri-frontend-api).

### Bun

`@coldmind/devermind` connects Bun's native `onBeforeParse` hook to the shared Rust
transform engine:

```ts
import { devermind } from "@coldmind/devermind";

const result = await Bun.build({
  entrypoints: ["./src/main.ts"],
  plugins: [devermind({ define: { __MODE__: JSON.stringify("production") } })],
});

if (!result.success) throw new AggregateError(result.logs, "Build failed");
```

Build the native adapter with `bun run build:native` after setting up the
JavaScript workspace. Bun owns its bundler and server lifecycle; this plugin does
**not** launch Devermind's Rust development server. See the [Bun adapter guide](packages/bun/README.md)
for transform options and native-library requirements.

### React shell

`@coldmind/devermind-shell` is an optional wrapper around your React application:

```tsx
import { startShell } from "@coldmind/devermind-shell";

startShell({
  root: document.getElementById("root")!,
  load: () => import("./App.tsx"),
});
```

The loaded module's default export is the root component. The shell provides an
error boundary, an isolated overlay, server reconnection handling, and one
WebSocket for hot updates and application channels. Its terminal-style console
is an application-message interface—**not an OS shell or PTY**.

See the [shell guide](packages/shell/README.md) and [React example](examples/react-shell).

## Requirements and compatibility

- Native TOML configuration and Rust source compilation do not require Bun or Node.
- The Bun adapter requires Bun and a native library built for the host platform.
- The React shell requires React 19 or newer.
- Development CSS compilation uses the application's installed Tailwind toolchain
  (`tailwindcss`, `@tailwindcss/node`, `@tailwindcss/oxide`) through Bun or Node.
  Production Tailwind builds also require those packages and a JavaScript runtime.
- The full integration runner requires Bun, Node 22+ with native fetch/WebSocket,
  and the pinned Rust toolchain. Its Tailwind fixture currently requires Unix.

For migration only, an absent native config permits discovery of supported
`vite.config.*` files. That adapter executes trusted configuration through Bun;
it is **not** Devermind's native format or a general Vite plugin runtime.
Vite plugin hooks, arbitrary plugin transforms, and automatic environment-file
loading are not supported. See [compatibility details](docs/configuration.md#optional-vite-compatibility).

Development endpoints are intended for trusted local projects. Keep the default
loopback binding unless you deliberately need network access; this is not an
authenticated remote build service.

## Development and verification

Until the standalone JavaScript workspace is extracted, install JavaScript
dependencies with `bun install` from the enclosing framework root. Then, from
this product's root:

```sh
bun run test:integration
bun run typecheck
cargo clippy --workspace --all-targets
```

The integration gate rebuilds the CLI and native Bun library, runs Rust and adapter
tests, and exercises actual HTTP/WebSocket traffic, watcher updates,
build-to-preview output, Tailwind, and Tauri IPC permissions. Failed phases and
missing prerequisites exit nonzero. Results go to `test-results/integration.json`;
`bun run test` invokes the same full gate.

**Coverage boundaries:** shell wire tests use Happy DOM, not a browser executing
the generated modules. Tauri tests use its mock runtime with real HTTP and IPC
dispatch, not a packaged native webview. Windows, installer packaging, and
application backends need separate validation. See the [testing guide](docs/testing.md).

## Repository layout

| Path | Responsibility |
| --- | --- |
| `crates/build-server` | Generic development serving and production bundling coordinator |
| `crates/core` | Native configuration and compiler facade |
| `crates/transform` | Shared transforms and optional Bun bindings |
| `crates/server` | HTTP serving, module pipeline, watching, and HMR |
| `crates/bundler` | Production frontend bundling |
| `crates/cli` | The single `devermind` executable |
| `crates/tauri` | Tauri frontend hosting and transform IPC |
| `packages/bun` | Bun plugin and Tauri frontend client |
| `packages/shell` | React wrapper and hot runtime |
| `examples` | Bun transform and React shell examples |
| `vendor` | Compiler, resolver, environment, and module-graph sources |

Contributions should include regression tests for changed behavior. Keep runtime
ownership explicit, document non-trivial lifecycle and concurrency decisions,
and avoid unrelated formatting changes. See [architecture](docs/architecture.md),
[configuration](docs/configuration.md), and [migration notes](docs/migration.md).

## Provenance and acknowledgements

Devermind builds on existing open-source projects. It does not implement every
compiler, parser, or runtime from scratch. The distinction between vendored
source, library dependencies, and Devermind's integration code is intentional:

| Part | Origin and role |
| --- | --- |
| Per-file compiler, import resolver, environment helpers, and module graph | Adapted from [oj](https://github.com/lovablelabs/oj), by Raphael Amorim and Lovable Labs. These sources live in `vendor/oj_compiler`, `vendor/oj_resolver`, `vendor/oj_env`, and `vendor/oj_graph`, including local changes. |
| JavaScript/TypeScript parsing, ASTs, scope analysis, and syntax transforms | [Oxc](https://github.com/oxc-project/oxc), used as Rust dependencies by the vendored compiler and shared transform engine. |
| Production JavaScript bundling | [Rolldown](https://github.com/rolldown/rolldown), used by `crates/bundler`; Devermind supplies configuration, HTML entry handling, and stylesheet/asset integration. |
| Browser React Refresh runtime | React Refresh code from [React](https://github.com/facebook/react), with the Meta Platforms copyright notice retained in `crates/server/assets/refresh-runtime.js`. |
| Fallback browser HMR client and refresh preamble | oj-derived code in `crates/server/assets/client.js` and `refresh-preamble.js`, retaining Raphael Amorim's MIT/SPDX notices and adapted for Devermind. |
| CSS and utility generation | The application's installed [Tailwind CSS](https://github.com/tailwindlabs/tailwindcss) compiler and scanner; Devermind orchestrates compilation and dependency invalidation. |
| Async HTTP/WebSocket serving and filesystem watching | Tokio, Axum, and `notify` library dependencies underneath Devermind's server lifecycle and module pipeline. |
| Bun native interface | Bun's `onBeforeParse` hook through `bun-native-plugin` and Node-API bindings through the `napi`/`napi-derive` crates. |
| Desktop and React hosting | Tauri supplies the desktop/plugin framework; React supplies rendering. Devermind supplies the host adapter, transform commands, development shell, overlay, and shared transport. |

Devermind's project-specific work connects these pieces: native TOML
configuration, the unified CLI, embedded hosting, shared transform stages,
production asset handling, the React shell, and cross-runtime regression tests.
The checked-in `vendor/` sources are build inputs; `reference/oj` is an upstream
reference checkout, not a second compiler implementation used at runtime.

HMR messaging follows a Vite-compatible protocol. This is protocol compatibility,
not a dependency on the Vite development server or its plugin system.

The oj MIT license and copyright notice are retained in
[`vendor/LICENSE`](vendor/LICENSE). Other upstream components retain their own
licenses and copyright notices; this overview does not replace those notices.
