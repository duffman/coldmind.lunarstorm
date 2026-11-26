# lunarstorm-project (platform/project)

The `project` package of the `lunar` command line, mounted by `platform/cli/app` behind the `project` feature (on by default).

```text
lunar project types                             the registered types and their actions
lunar project <type> <action> [args]            e.g. lunar project program create hello
lunar project insight [dir]                     description, dependencies, files and lines per extension, build
lunar project compile [dir]                     build/<name>.jbn (runtime bytecode) + build/trust.json
lunar project sign [dir] --key <developer.json> build/<name>.jbn signed with an issued developer key
```

## Types and actions

The products register project types; the binary passes their registrations in:
`lunarstorm_project::package(vec![lunarstorm_cli_types::types, lunarstorm_desktop_types::types])`.

| type | crate (folder) | actions |
|---|---|---|
| `program`, `module` | `lunarstorm-cli-types` (`platform/cli/types`) | `create` |
| `app` | `lunarstorm-desktop-types` (`platform/desktop/types`) | `create` |

```rust
pub fn types() -> Vec<Type> {
    vec![Type::new("program", "a program lunar runs").action(create("program", "src/main.ts", &["lunar", "node"], FILES))]
}
```

- A type is a name, one line about it and its actions; an action is a plain cli-contract `Command` (own arguments, own handler), mounted as `lunar project <type> <action>`.
- A type must not be named `types`, `insight`, `compile` or `sign`.
- `create(kind, main, modes, files)` is the action that makes a project: `package.json` with `lunar.type` = the type's name, then the type's files (`"<kind>/<path>"` in `files`). `{{name}}` in a UTF-8 file becomes the package name; `gitignore` is written as `.gitignore` (a default when the type has none); it never overwrites a file and refuses a directory that already has a `package.json`. `--mode` takes `modes`, the first being the default.

## Developer keys

- Lunarstorm holds a root Ed25519 key; its public half is `developer::ROOT`, built into `lunar`.
- A developer key file holds the developer's key pair and a certificate from the root over the developer id, expiry and public key. `sign` refuses a key whose certificate does not verify against `ROOT`, or that has expired.
- A signed package carries the certificate in its manifest metadata (`developer`, `developer.expires`, `developer.key`, `developer.certificate`); `developer::signed_by(package, &ROOT)` checks it.
- Issuing is Lunarstorm's side, not a `lunar` command:

```text
cargo run -p lunarstorm-project --example issue -- root <root.der>                     # once; prints ROOT
cargo run -p lunarstorm-project --example issue -- developer <root.der> <id> <days> > developer.json
```

## Decisions

- `compile` and `sign` take only a `program` or `module` project; another type has its own build (an `app` is built by `lunar build -p`, lunarstorm-pipeline).
- Types are registered by the products, not listed here: the binary decides which products' types it mounts.

- `compile` signs with a per-project development key (`.lunar/dev.der`, owner-only, made on first use): a `.jbn` cannot be unsigned.
- `compile` and `sign` build through `lunarstorm_host_cli::compile`, the path `jbn build` uses, so packages link on the Runtime and host modules `run` provides.
- `sign` recompiles rather than re-signing the built file; builds are deterministic, so the result is the same.

## Build and test

```text
cargo test -p lunarstorm-project
```
