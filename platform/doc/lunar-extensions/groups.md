# Extension groups

Status: **design, not implemented.** Builds on `README.md` (trait, registration), `permissions.md` and
`lifecycle.md`.

A **group** is a named set of extensions. Its members live under the group's namespace:

```js
await native.media.audio.play("a.ogg");      // group `media`, extension `audio`, command `play`
await native.media.video.open("b.mp4");
await native.store.get("theme");             // an ungrouped extension, as before
```

What a group gives:

| | |
|---|---|
| **one delivery** | one crate returns several related extensions; the application registers them with one call |
| **one grant** | `media.*` grants every command of every member |
| **one order** | members run `setup` in the order the group lists them, which is how one member depends on another |
| **one name** | `media.audio` and `media.video` cannot collide with an unrelated `audio` |

Tauri's docs describe permission sets (a group of grants). I found nothing there that groups plugins, so a
group is new here and is not a permission set: it groups the extensions themselves.

## Declaring a group

```rust
pub struct Group { /* name, members in order */ }

impl Group {
    pub fn new(name: &'static str) -> Group;
    pub fn extension(self, extension: impl Extension) -> Group;     // appended; order is setup order
}
```

A bundle crate (`lunar-extension-media`) exports the group:

```rust
pub fn group() -> Group {
    Group::new("media")
        .extension(lunar_extension_audio::init())
        .extension(lunar_extension_video::init())
}
```

The application registers it next to ungrouped extensions, and may build a group of its own:

```rust
Builder::new()
    .group(lunar_extension_media::group())
    .group(Group::new("tools").extension(a::init()).extension(b::init()))
    .extension(lunar_extension_store::init())
    .run()
```

Decision: the **group** assigns the prefix, not the extension. `Extension::name()` returns `audio` and does
not change, so the same crate works ungrouped (`native.audio`) or in a group (`native.media.audio`).
`Context::namespace()` returns the full name (`media.audio`); an extension uses it in messages and in
state it keys by namespace.

## Namespaces

- Full namespace: `<group>.<name>`; an ungrouped extension's is `<name>`.
- Group name: the same rule as an extension name, `[a-z][a-z0-9]{0,31}`.
- **One level only.** A group holds extensions, never groups. Decision: a group of groups needs a rule for
  which level owns a grant and a scope, and nothing asks for it. The wire stays `(namespace, method)`;
  only the namespace string gains one dot.
- Startup errors, before any window opens:
  - a group name that is a built-in namespace, an ungrouped extension's name, or another group's name;
  - an empty group;
  - two members of one group with the same name;
  - the same extension type registered twice in the process (its state would be managed twice).

## The JS side

- `native` today is a `Proxy` returning a function for `native.<ns>.<method>`
  (`runtime/host-desktop-backend/src/desktop_bridge_natives.rs`). It becomes a proxy that accumulates the property path and sends on
  **call**: the last segment is the method, the rest joined by `.` is the namespace. `native.window.create(...)`
  is unchanged: namespace `window`, method `create`.
- A call with fewer than two segments (`native.media()`, `native.window()`) throws
  `TypeError: native.window is a namespace, not a function`.
- Guest modules take the namespace as a path: `@extension/media/audio`, `@extension/store`.
- The platform generates `@extension/media`, re-exporting every member that has a guest module:
  `export * as audio from "@extension/media/audio";`. A group needs no guest code of its own.

## Permissions

| entry in `permissions.native` | grants |
|---|---|
| `media.audio.play` | one command (what the derivation from form classes produces) |
| `media.audio.*` | every command of `media.audio` |
| `media.*` | every command of every member of `media`; written by hand |

- The gate (`HostConfig::allows_native`) keeps its exact and `<namespace>.*` rule. It gains one rule: an entry
  `<prefix>.*` also grants a call whose namespace starts with `<prefix>.`. The dot is part of the match, so
  `shell.*` never grants `shellx.run`.
- **No group-level scope.** Scopes stay per extension (`media.audio`) and per command
  (`media.audio.play`). Decision: a scope entry is typed by the extension that reads it, and a group has no
  type to read it with. A group-wide data limit would force every member to accept one entry shape.
- Grouping is naming, not a boundary: a member can call another member's Rust API through its extension
  trait, and `media.*` is a convenience for the application developer, not isolation.

## Configuration

`zebra.json` `extensions` is keyed by full namespace; a group has no entry of its own:

```json
{ "extensions": { "media.audio": { "device": "default" }, "store": { "file": "state.json" } } }
```

## Lifecycle

- Registration order is global: a group counts as one slot, and its members run in declaration order inside it.
  `setup` runs in that order; `release_epoch` and `Drop` run in reverse.
- Hooks, state and channels are per extension and do not change. State is one map by type for the process,
  so members of a group share it exactly as any two extensions do.
- A member whose `setup` fails aborts startup: `extension media.audio: <message>`; later members of the
  group do not run.

## Changes to existing code

| where | change |
|---|---|
| `runtime/host-desktop-backend/src/desktop_bridge_natives.rs` | `native` accumulates the path and sends on call |
| `platform/desktop/src/manifest.rs` `allows_native` | the `<prefix>.*` rule |
| `platform/desktop/src/native.rs` | the plugin map is keyed by full namespace |
| `lunar-extension` | `Group`, `Context::namespace()` |
| `lunarstorm-platform` | `Builder::group` |
