---
name: lunarstorm-monorepo
description: Navigate and maintain the Lunarstorm monorepo (folder `hippo`): where a change belongs, how to add or move a crate or pnpm package, which files are embedded across the Rust/JS boundary, the build and test commands per area, and the hazards that look safe but are not. Use whenever someone adds, renames, moves or removes a package, touches the root Cargo.toml, pnpm-workspace.yaml, .gitignore or .cargo/config.toml, edits framework/desktop/forms.js, templates/zebra.framework or framework/vcl/react, asks "where does X live" or "how do I build/test X", or writes a package README.
---

# Lunarstorm monorepo

One Cargo workspace (root `Cargo.toml`, members listed explicitly) and one pnpm workspace
(`pnpm-workspace.yaml`, explicit globs). Name everything `lunarstorm`; binaries land in
`_build/`; Cargo output goes to `~/.cargo-target` (never pass `--target-dir`).
Spec: `doc/plans/main.md`. Understanding notes, if present: `.omx/understanding/` (MAP, HAZARDS).

## Where a change belongs
| change | package |
|---|---|
| new CLI command | the crate that owns the function; see skill `lunarstorm-cli` |
| parse rules, model errors | `toolchain/crates/model` (spec 4a, 2.2) |
| generated frontend/backend/host config | `toolchain/crates/pipeline` (4b-4e) |
| event loop, timers, modules, fs/net, Node builtins | `runtime/event-loop-runtime` (see Runtime layout); engine contract `runtime/engine-contract`; QuickJS `runtime/engine-quickjs-adapter` |
| desktop backend host | `runtime/host-desktop-backend`; CLI host `runtime/host-command-line` |
| windows, IPC, assets, dev change table | `platform/desktop` (`changes.rs`, `envelope.rs`, `manifest.rs`) |
| packaging/signing; browser host | `platform/pack`; `platform/web` |
| backend form/control runtime | `framework/desktop/forms.js` |
| React controls and shell | `framework/vcl/react` then rebuild `templates/zebra.framework/dist/vcl.js` |

## Commands
- All Rust: `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets`.
  One crate: `-p <name>` (names differ from folders: `cargo metadata --no-deps --offline`).
  `apps/lunarstorm` has one Cargo feature per package; test every feature set (`runtime`, `build`, `frontend`, all, none).
- JS: `pnpm install`, `pnpm typecheck`, `pnpm --filter <name> test`. `pnpm test:js` skips packages without a `test` script.
- Rebuild the WebView runtime: `cd templates/zebra.framework && bun run build`.
- Run a project: `cargo run --bin lunarstorm -- build -p <project>`, then `cargo run --bin lunarstorm-debug -- <project>/dist`.
- Builds sharing `~/.cargo-target` run one at a time; `touch` differing sources after building elsewhere.
- Long runs (full workspace test) go in the background.

## Runtime layout and naming (`runtime/`)
| folder | crate | holds |
|---|---|---|
| `engine-contract` | `lunarstorm-engine-contract` | the VM contract: `engine_traits.rs`, `native_declarations.rs`, `engine_conformance_suite.rs` |
| `engine-quickjs-adapter` | `lunarstorm-engine-quickjs-adapter` | the QuickJS adapter: `quickjs_scope_operations.rs`, `quickjs_module_loading.rs`, ... |
| `event-loop-runtime` | `lunarstorm-event-loop-runtime` | `runtime_instance.rs` (the `Runtime`), `event_loop/` (one step), `node_builtins/` (the Node surface, native), `internal_natives.rs` (`ops`) |
| `module-sources`, `jbn-package-format`, `typescript-declarations` | same names prefixed `lunarstorm-` | resolution and transforms, `.jbn` packages, the `.d.ts` emitter |
| `host-command-line`, `host-desktop-backend` | `lunarstorm-host-command-line`, `lunarstorm-host-desktop-backend` | the CLI host (`lunar_runtime_commands.rs`) and the desktop backend (bin `lunarstorm-desktop`) |
| `debugger`, `compiler/*` | `lunarstorm-debugger`, `qjs-*` | jetbunny's debugger and Rust JS compiler |
| `vendor/rquickjs-binding` | `rquickjs*` | the engine binding; `compiler/vendor/quickjs-ng` is a symlink to its `sys/quickjs` |
| `docs/` | | the runtime docs, start at `docs/README.md` |

Rules for files here:
- A file name says what the file holds, without its folder: `reactor_io_state.rs`, not `io.rs`;
  `posix_path.rs`, not `path.rs`. No `mod.rs`: a module with children is `name.rs` plus `name/`.
- Every `.rs` file starts with a `//!` header that says what it holds (tests and examples too).
  A file pulled in with `include!` takes a plain `//` header instead.
- Split a file past ~800 lines by concern; parts of one `impl` may live in child modules.
- `debugger` and `compiler/*` are verbatim ports: their module names stay jetbunny's (`debug`,
  `engines::quickjs_ng`, `js_emit`) and are bound to descriptive file names with `#[path]`, so
  upstream code and tests apply unchanged. Rename files there, not modules.
- The debugger's `build.rs` hashes every compiler `.rs` (recursively) into the engine build id.

## Add or move a package
1. Rust: add the path to `[workspace] members` in the root `Cargo.toml` (and the header comment table); `rust-version`/`edition` via `workspace = true` where the toolchain crates do. Excluded on purpose: `runtime/vendor/rquickjs-binding/wasm-test`.
2. JS: add a glob (or path) to `pnpm-workspace.yaml`; extend `@coldmind/config/tsconfig.base.json`; use `@coldmind/config/vitest`.
3. Depend by name, not by relative path, when a path leaves the package. Rust `include_str!` across packages is the exception that exists today (see hazards); do not add more.
4. Write `README.md`: what/why, public surface with real names, how it fits (dependents, spec section), build/test commands, decisions and hazards. Delete any sentence that fits any crate.
5. Update `doc/cli.md` if the package mounts commands; update the root README table for a new area.
6. Never delete a package: move it to `.archive/<YYYY-MM-DD>/<path>`.

## Hazards
- Embedded across the Rust/JS line: `framework/desktop/forms.js`, `framework/vcl/react/src/vcl.css`, `templates/zebra.framework/dist/vcl.js` (committed on purpose; test `toolchain/crates/pipeline/tests/embedded_inputs.rs` guards the git-ignore), `toolchain/react/packages/shell/src/*`. After editing, rebuild the embedder.
- No integer values in root `[profile.*]`: they live in `.cargo/config.toml` (trybuild + serde_json `arbitrary_precision`).
- `platform/examples/app` is hand-written; Rust tests name its controls. Do not regenerate.
- `cli-contract` has zero dependencies; Runtime/VM/Host calls are static (no `dyn`).
- A failed `pipeline::build` must leave the previous output untouched.
- Scratch files go in `<repo>/.scratch/` (gitignored), never `/tmp`.
- The toolchain "devtools" crates (`core`, `server`, `bundler`, `cli`, `tauri`, `build-server`) are an older product; `devtools-cli` is mounted on `lunarstorm` (feature `frontend`: `serve`, `bundle`, `preview`, `transpile`) and its end-to-end tests are `apps/lunarstorm/tests/frontend_*.rs`. See their READMEs before relying on the rest.

## Verify
State which suites ran and their counts; 0 tests is not a pass; name any feature set or engine not run.
