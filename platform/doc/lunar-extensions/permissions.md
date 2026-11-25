# Extension permissions

Status: **design, not implemented.** Today's gate is `HostConfig::allows_native` in
`platform/desktop/src/manifest.rs`; spec: `doc/plans/main.md` §2.6.

Two separate things control an extension, as they do for the built-ins:

| | answers | written by | stored in |
|---|---|---|---|
| **grant** | may the backend call `store.set` at all | derived from what the form classes reference | `permissions.native` |
| **scope** | which data may `store.set` touch | the application developer | `permissions.scopes` |

## Grants

- The entry is `<namespace>.<command>`, or `<namespace>.*`: the format `allows_native` checks today. For a
  grouped extension the namespace is `<group>.<name>`, and `<group>.*` grants the whole group (`groups.md`).
- Spec §2.6: "The native API permissions are derived from what the form classes reference." A reference to
  `native.store.set` yields `store.set`. An extension namespace takes the same path as `clipboard.readText`; the
  derivation needs no extension-specific code.
- Default is deny. A command the application never references is never granted.
- The gate runs before the router (`runtime/lunarstorm.rs`) and refuses with
  `native method store.set is not permitted by the host configuration`. It does not know whether the extension
  exists, so an unregistered extension and a missing grant look the same until the gate passes.

Decision: no `allow-<cmd>` / `deny-<cmd>` files, no permission sets, no `default` permission (Tauri has all
three). Tauri lists grants by hand because the caller is an untrusted WebView and each grant is a security
review. Here the caller is the application's own backend and the grant is derived from its source, so a
hand-written list would only repeat the code it describes. The alternative of generating the ids from
`commands()` was dropped for the same reason.

## Scopes

A grant lets the backend call `store.set`; a scope limits what that call may do. The built-ins already have
them: `permissions.fs` (filesystem roots), `permissions.executables`, `permissions.env`.

`zebra.json`, one new field in `permissions`:

```json
{
  "permissions": {
    "native": ["store.get", "store.set"],
    "scopes": {
      "store": { "allow": [{ "key": "theme" }, { "key": "ui.*" }], "deny": [{ "key": "ui.secret" }] }
    }
  }
}
```

- Key: the full namespace for the whole extension (`store`, `media.audio`), `<namespace>.<command>` for one
  command. A call sees both; **deny wins** over allow, and a command-level entry never widens the
  extension-level one. There is no key for a group (`groups.md`).
- Entries are extension-defined JSON objects. The extension reads them typed:

  ```rust
  #[derive(serde::Deserialize, schemars::JsonSchema)]
  struct Entry { key: String }

  let scope = call.scope::<Entry>()?;          // Scope<Entry>: allows() and denies()
  if !scope.permits(|e| matches(&e.key, key)) { return call.done.complete(Err("key is not in scope".into())); }
  ```

- No scope entry for an extension means `allows()` is empty, which permits nothing. An extension that needs no scope
  says so by never calling `call.scope`.
- The extension exports a JSON schema for `Entry` (`Extension` gains no method for it: it is a free function
  `lunar_extension::schema::<Entry>()` the extension's `build.rs` writes to `schema.json`). The pipeline
  validates `scopes.<extension>` against it when the configuration loads, so a typo in `"ky"` fails at load, not
  at the call.
- The extension must check the scope in the command. The platform does not: it cannot know what an entry means.
  Tauri's docs state the same limit: an unchecked scope protects nothing.

## What is not protected

- An extension is Rust running in the platform process with the process's full rights. Grants and scopes limit
  the **backend script**, not the extension. Installing an extension is trusting it.
- Scopes are only as strict as the extension's check.

## Dev and reload

- `permissions` is in the §6.1 row `<application> identity, permissions, package.json` → `dev.restart`. Both
  `native` and `scopes` take that row; neither changes under a running backend.
- An extension's own state is per process; scopes are per application run. A backend reload (§6.2) keeps both.

## Errors

| situation | message |
|---|---|
| no grant | `native method <ns>.<cmd> is not permitted by the host configuration` |
| granted, no such extension | `unknown native namespace <ns>` |
| granted, no such command | `unknown method <ns>.<cmd>` |
| call outside the scope | extension's own message, by convention `<what> is not in scope` |
| scope entry does not match the extension's schema | configuration load fails: `zebra.json: scopes.<extension>: <detail>` |
