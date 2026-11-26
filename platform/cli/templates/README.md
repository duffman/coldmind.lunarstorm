# lunarstorm-cli-templates (platform/cli/templates)

The cli product's templates for `lunar project init`: `cli/program` (`src/main.ts`) and
`cli/module` (`src/index.ts`). `--type program|module` picks them.

Each folder holding a `template.json` is one template; `build.rs` embeds every folder as
`FILES`, which `platform/cli/app` and `platform/cli/lunarstation` pass to
`lunarstorm-project`. The format is in `platform/project/README.md`.

```text
cargo test -p lunarstorm-project      init through the real command-line host
```
