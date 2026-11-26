# lunar-extension (extensions/core)

The rules every lunar-extension follows, at every level of Lunarstorm: valid names, the manifest an application declares its extensions in, the plugin hosts an extension opens, and the load-time check that compares the manifest with what a host offers. It has no VM and no platform dependency, so the runtime level (`lunarstorm-event-loop-runtime`) and the platform level apply the same rules and report the same messages. Concepts: `runtime/docs/extension-manifests.md`.

## Public surface

- `valid_name(&str) -> bool`: `[a-z][a-z0-9]{0,31}`; `RESERVED = ["ipc", "worker", "internal"]`, the Runtime's own `lunarstorm:` modules.
- `Manifest { extensions: BTreeMap<String, Entry> }`, `Entry { plugins: Vec<String>, config: Option<Value> }`; `Manifest::parse(json)`, `Manifest::empty()`, `Manifest::using(name) -> Option<Use>`.
- `Use`: `plugins()` in manifest order, `has(plugin)`, `config()`.
- `Offer { name, plugin_hosts }`, `PluginHost { name, cardinality, requirement, plugins }`, `Cardinality::{One, Many}`, `Requirement::{Required, Optional}`.
- `check(&Manifest, &[Offer]) -> Result<(), Vec<String>>`: every mismatch, one message each.

```json
{ "extensions": { "hostapi": { "plugins": ["ui"], "config": {} } } }
```

| condition | message |
|---|---|
| reserved, invalid or duplicate extension name in the host | `host: extension name ipc is reserved`, `host: extension name Bad is invalid`, `host: extension dup is offered twice` |
| invalid or duplicate plugin name in the host | `host: extension <name>: plugin name <p> is invalid`, `host: extension <name> offers plugin <p> twice` |
| an extension the host does not offer | `manifest: this host has no extension <name>` |
| a plugin the extension does not offer | `manifest: extension <name> has no plugin <plugin>` |
| a plugin listed twice | `manifest: extension <name> lists plugin <plugin> twice` |
| a required plugin host left empty | `manifest: extension <name>: plugin host <slot> requires a plugin` |
| two plugins in a `One` plugin host | `manifest: extension <name>: plugin host <slot> takes one plugin` |
| a key this version does not know | `manifest: unknown field ...` (from `parse`) |

## How it fits

- Depends on `serde`, `serde_json`.
- Used by `lunarstorm-event-loop-runtime` (re-exported there; checked in `Runtime::start`) and through it by every extension (`extensions/hostapi`) and host.

## Build and test

```text
cargo test -p lunar-extension      # tests/check.rs: names, parsing, every check message
```

## Decisions

- Plugin names are unique within their extension across all its plugin hosts, so a manifest lists an extension's plugins as one flat list and a module path `lunarstorm:<extension>/<plugin>` is unambiguous.
- Unknown manifest keys are errors, never ignored: a permission form of a later version must not silently grant nothing.
- `check` reports all mismatches at once, host faults (`host:`) before manifest faults (`manifest:`).
