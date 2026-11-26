# lunarstorm-desktop-types (platform/desktop/types)

The desktop product's project types: `app`, a forms project that `lunar build` builds
(`src/app.tsx`, `src/forms/main.view.tsx`, `src/forms/main.ts`), with the action `create`
(`lunar project app create notes`). It targets lunar only.

`app/` is also the desktop starter the model and `lunar build` tests read in place, so its
source files carry no `{{name}}` placeholder (only `README.md` does).

Each top-level folder except `src` holds one type's files; `build.rs` (the same script as
`lunarstorm-cli-types`) embeds them as `FILES`, and `types()` registers them with
`lunarstorm-project`.

```text
cargo build -p lunarstorm-desktop-types
```
