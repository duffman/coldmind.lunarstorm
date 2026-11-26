# lunarstorm-cli-types (platform/cli/types)

The cli product's project types: `program` (`src/main.ts`) and `module` (`src/index.ts`),
each with the action `create` (`lunar project program create hello`).

Each top-level folder except `src` holds one type's files; `build.rs` embeds them as
`FILES`, and `types()` registers the types with `lunarstorm-project`.
`platform/cli/app` and `platform/cli/lunarstation` pass `types` to
`lunarstorm_project::package`.

```text
cargo build -p lunarstorm-cli-types
```
