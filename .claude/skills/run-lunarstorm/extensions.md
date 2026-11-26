# Extensions: how a host adds functionality to its Runtimes

A **lunar-extension** is a named package of modules, natives and per-Runtime state that a host
installs. The host's **manifest** selects which extensions and plugins are active. An extension
can open **plugin hosts** (named slots) that **plugins** implement.

- Built: the runtime level.
- Design only: the platform level (`platform/doc/lunar-extensions/*`, "design, not implemented"), meaning `native.<ns>.<cmd>` as extensions, an object-safe trait, and threads.

Concepts: `runtime/docs/extension-manifests.md`. Plan: `doc/plans/hostapi.md`.

## Pieces

| piece | where | what |
|---|---|---|
| `lunar-extension` | `extensions/core/src/lib.rs` | no VM: name rule `[a-z][a-z0-9]{0,31}`, `RESERVED = ipc, worker, internal`, `PluginHost{name, cardinality One/Many, requirement Required/Optional, plugins}`, `Offer`, `Manifest` (`parse`, `using(name) -> Option<Use>`), `Use` (`plugins`, `has`, `config`), `check(&Manifest, &[Offer])` reporting every error |
| `Extension<V>` / `ExtensionSet<V>` | `runtime/event-loop-runtime/src/extension_manifest_check.rs` | `const NAME`, `plugin_hosts()`, `bindings(&Use) -> Result<Bindings<V>, String>`; sets are `()` and tuples up to 8; `install` runs only for extensions the manifest uses, errors prefixed `extension <name>:` |
| `Bindings<V>` | `runtime/event-loop-runtime/src/host_bindings_registry.rs` | `function(name, f)` → `ops.<name>`; `values(f)` → raw values on `ops`; `module(specifier, source)` → a JS host module; `merge(owner, other)` fences modules to `lunarstorm:<owner>[/...]` and refuses duplicate modules/functions |
| `HostApi<V, A>` | `extensions/hostapi` (`lunar-extension-hostapi`) | the host's own API as plugins: plugin host `area` (Many, Optional); each `Area` is fenced to `lunarstorm:hostapi/<area>`; `lunarstorm:hostapi` exports `areas` (frozen, manifest order) |
| `Ui` area | `platform/cli/host/src/terminal_ui_area.rs` + `terminal_ui_module.rs` (native) | `lunarstorm:hostapi/ui` (alias `lunarstorm:ui`): `style`, `progress`, `spinner` |

**Manifest format** (`deny_unknown_fields`; `extensions` is the only key):

```json
{ "extensions": { "hostapi": { "plugins": ["ui"], "config": null } } }
```

The CLI host's manifest is `platform/cli/host/manifest.json`, embedded with `include_str!`. The
desktop host uses the empty default.

**Enforced today:**
- An extension the manifest doesn't use adds nothing, so its specifiers don't resolve.
- Plugins must exist; the One and Required rules hold.
- Names must be valid and unreserved.
- The namespace fence.

**Planned only:** per-export grants, scopes, `setup` and release hooks. The `"permissions"` key in
the concepts doc is rejected by the parser.

## Start-up and call path

`Runtime::start` runs these steps before the VM exists:
1. `host.extensions()`, fresh per Runtime, so each worker gets its own state.
2. `check(manifest, offers)`.
3. `install` into the bindings, after the host's own.
4. `ops::install` (core natives).
5. The extension installers.
6. The glue.

A call from a script into an extension:

```
script → import { f } from 'lunarstorm:<ext>'      (host module, JS source, compiled on first import)
       → f() → ops.<ext>_f(...)                     (JS frame + property lookup on the hidden ops)
       → rquickjs callable class → Native adapter   (Box<dyn>, args copied with refcount bumps)
       → your Rust closure Fn(&Call<V>)             (captures per-Runtime state)
```

An extension's JS reaches `ops` only through `import { ops } from 'lunarstorm:internal'`, which is
allowed for host modules and refused for program files. `lunarstorm:ipc` and `lunarstorm:worker`
are faster **core native modules** (`Scope::native_module` → `Load::Native`), whose exports are
the Rust natives with no JS wrapper. Extensions cannot declare native modules today.

## Recipe: a new runtime-level extension `foo`

1. **Create the crate.** Make `extensions/foo/Cargo.toml` with `name = "lunar-extension-foo"`, dependency `lunarstorm-event-loop-runtime = { path = "../../runtime/event-loop-runtime" }` and dev-dependency `lunarstorm-engine-quickjs-adapter`. Add it to the root `Cargo.toml` `members` and its header table.
2. **Write the JS module.** `extensions/foo/js/foo.js`:
   `import { ops } from 'lunarstorm:internal'; export const bar = (x) => ops.foo_bar(x);`
3. **Implement the extension.** In `src/lib.rs`, `impl<V: Vm> Extension<V> for Foo { const NAME: &'static str = "foo"; fn bindings(&self, using: &Use<'_>) -> Result<Bindings<V>, String> { Ok(Bindings::new().function("foo_bar", |call| ...).module("lunarstorm:foo", include_str!("../js/foo.js"))) } }`.
   - Prefix every `ops` name with `foo_`. `merge` does not see the core ops (`print`, `now`, …) or names set through `values`, so a clash silently replaces them.
   - Keep state in `Rc`/`Cell` captured by the closures; it is per Runtime.
   - Read configuration with `using.config()`.
   - For plugins, override `plugin_hosts()` and fence each plugin with `merge("foo/<plugin>")` as HostApi does.
   - For a host-specific API, write a HostApi `Area` instead, like `platform/cli/host/src/terminal_ui_area.rs`.
4. **Wire it into a host.**
   - In `platform/cli/host/src/command_line_host_services.rs`, set `type Extensions<V: Vm> = (HostApi<V, (Ui,)>, Foo);` and build it in `extensions()`. Tuple order is install order.
   - Add `"foo": {}` to `platform/cli/host/manifest.json`.
   - Add the dependency to `platform/cli/host/Cargo.toml`.
   - Optionally list it in the help's MODULES rows (`lunar_runtime_commands.rs`).
5. **Test**, modelled on `runtime/event-loop-runtime/tests/extension_loading.rs` and `extensions/hostapi/tests/hostapi.rs`, with a `TestHost`, `Manifest::parse`, `Runtime::<QuickJs, _>::new`, `start`, `load(Source::Text)` and `run`. Cover:
   - the module works;
   - unused means inert;
   - a bad manifest makes `start()` fail;
   - fence refusals;
   - per-Runtime state.

   Also keep the host's `the_manifest_matches_the_host` passing, and add an end-to-end case in `platform/cli/app/tests/`.

## Operate

```sh
cargo test -q -p lunar-extension-hostapi                  # 6 tests
cargo test -q -p lunarstorm-event-loop-runtime --test extensions     # 8 tests
~/.cargo-target/debug/lunar -e 'import ui from "lunarstorm:ui"; import { areas } from "lunarstorm:hostapi"; console.log(Object.keys(ui), areas)'
```
