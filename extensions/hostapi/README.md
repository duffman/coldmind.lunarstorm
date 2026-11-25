# lunar-extension-hostapi (extensions/hostapi)

HostApi is the extension through which a host gives its programs the host's own API. It opens one plugin host, `area` (`Many`, `Optional`); every API area a host offers is a plugin there, and the host's manifest selects the active ones. Concepts: `runtime/core/doc/extensions.md`; plan: `doc/plans/hostapi.md`.

| module | exports |
|---|---|
| `lunarstorm:hostapi` | `areas`: the names of the active areas, in manifest order, frozen |
| `lunarstorm:hostapi/<area>` | the API of one active area |

## Public surface

- `HostApi<V, A>`: the extension (`NAME = "hostapi"`), `HostApi::new(areas)` with the areas as a tuple.
- `trait Area<V>`: `NAME` and `bindings(&Use) -> Result<Bindings<V>, String>`, the extension's own shape scoped to one area.
- `trait AreaSet<V>`: `()` and tuples up to 8.
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

- Depends on `lunarstorm-runtime` only; dev-depends on `lunarstorm-vm-quickjs`.
- Used by `lunarstorm-host-cli`, whose terminal API is the area `ui` (`runtime/hosts/cli/src/ui.rs`).

## Build and test

```text
cargo test -p lunar-extension-hostapi   # tests/hostapi.rs: order of areas, inactive areas, area modules, errors
```

## Decisions

- An area serves only `lunarstorm:hostapi/<area>` and below; HostApi refuses anything else (`extension hostapi: area <name>: module <specifier> is outside lunarstorm:hostapi/<name>`), so one area cannot shadow another.
- Areas are installed in manifest order, after `lunarstorm:hostapi`; two areas binding one `ops` name are refused.
- `areas` comes from `ops.hostapi_areas`, set once per Runtime; `js/hostapi.js` freezes a copy.
- At the runtime level an area is granted by being listed: its module exports everything it defines. Grants per export and data scopes come with the first host whose manifest the application writes (the desktop host).
