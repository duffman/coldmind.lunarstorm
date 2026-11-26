# lunarstorm-desktop-templates (platform/desktop/templates)

The desktop product's templates for `lunar project init`: `desktop/app`, a forms project
that `lunar build` builds (`src/app.tsx`, `src/forms/main.view.tsx`, `src/forms/main.ts`).

`app/` is also the desktop starter the model and `lunar build` tests read in place, so its
source files carry no `{{name}}` placeholder (only `README.md` does).

Each folder holding a `template.json` is one template; `build.rs` (the same script as
`lunarstorm-cli-templates`) embeds them as `FILES`. No dependencies, so `lunar` mounts them
without linking the desktop host. The format is in `platform/project/README.md`.

```text
cargo test -p lunarstorm-project      init --template desktop/app
```
