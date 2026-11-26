# lunar-extension-hostapi (extensions/hostapi)

HostApi is the extension through which a host gives its programs the host's own API. It opens one plugin host, `area` (`Many`, `Optional`); every API area a host offers is a plugin there, and the host's manifest selects the active ones. Concepts: `runtime/docs/extension-manifests.md`; plan: `doc/plans/hostapi.md`.

| module | exports |
|---|---|
| `lunarstorm:hostapi` | `areas`: the names of the active areas, in manifest order, frozen |
| `lunarstorm:hostapi/<area>` | the API of one active area |

## Public surface

- `HostApi<V, A>`: the extension (`NAME = "hostapi"`), `HostApi::new(areas)` with the areas as a tuple.
- `trait Area<V>`: `NAME` and `bindings(&Use) -> Result<Bindings<V>, String>`, the extension's own shape scoped to one area.
- `trait AreaSet<V>`: `()` and tuples up to 8.
- `Area::FUNCTIONS` (default empty): the functions the area's module exports, as `Function { name, summary, args: &[Arg { name, optional, multiple }] }`.
- `HostApi::spec()`: the whole API as JSON, every offered area in tuple order: `{"version": 1, "areas": [{"name", "module", "functions": [{"name", "summary", "args": [{"name", "optional", "multiple"}]}]}]}`. The dev console's API tree (`@lunarstorm/devmode`, prop `api`) reads it, so the API is described once.
- `NAME`, `PLUGIN_HOST`.

A host installs it in its extension set and lists the areas in its manifest:

```rust
type Extensions<V: Vm> = (HostApi<V, (Ui,)>,);
fn extensions<V: Vm>(&self) -> Self::Extensions<V> { (HostApi::new((Ui,)),) }
```

```json
{ "extensions": { "hostapi": { "plugins": ["ui"] } } }
```

## How it fits

- Depends on `lunarstorm-event-loop-runtime` only; dev-depends on `lunarstorm-engine-quickjs-adapter`.
- Used by `lunarstorm-host-command-line`, whose terminal API is the area `ui` (`platform/cli/host/src/terminal_ui_area.rs`).

## Build and test

```text
cargo test -p lunar-extension-hostapi   # tests/hostapi.rs: order of areas, inactive areas, area modules, errors, spec
```

## Decisions

- An area serves only `lunarstorm:hostapi/<area>` and below; HostApi refuses anything else (`extension hostapi: area <name>: module <specifier> is outside lunarstorm:hostapi/<name>`), so one area cannot shadow another.
- Areas are installed in manifest order, after `lunarstorm:hostapi`; two areas binding one `ops` name are refused.
- An area describes its functions in a `const` next to its bindings, not in a separate file; the spec lists every offered area, active or not, because it describes the host, not one program's manifest.
- `areas` is built natively at the first import of `lunarstorm:hostapi` and frozen; nothing is put on `ops`.
- At the runtime level an area is granted by being listed: its module exports everything it defines. Grants per export and data scopes come with the first host whose manifest the application writes (the desktop host).
