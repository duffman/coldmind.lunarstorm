# lunarstorm-project (platform/project)

The `project` package of the `lunar` command line, mounted by `platform/cli/app` behind the `project` feature (on by default).

```text
lunar project init [dir] --type module|program --mode lunar|node
lunar project init [dir] --template <product/name|folder>   e.g. desktop/app
lunar project templates                         the templates init can use
lunar project insight [dir]                     description, dependencies, files and lines per extension, build
lunar project compile [dir]                     build/<name>.jbn (runtime bytecode) + build/trust.json
lunar project sign [dir] --key <developer.json> build/<name>.jbn signed with an issued developer key
```

## Templates

The products own their templates; the binary passes them in:
`lunarstorm_project::command::package(&[("cli", lunarstorm_cli_templates::FILES), ("desktop", lunarstorm_desktop_templates::FILES)])`.

| template | crate (folder) | `lunar.type` |
|---|---|---|
| `cli/program`, `cli/module` | `lunarstorm-cli-templates` (`platform/cli/templates`) | `program`, `module` |
| `desktop/app` | `lunarstorm-desktop-templates` (`platform/desktop/templates`) | `desktop` |

A template is a folder: `template.json` plus the files to write.

```json
{ "description": "a program lunar runs", "type": "program", "main": "src/main.ts" }
```

- `type` is `program`, `module` or `desktop`; `main` must name a file of the template.
- `{{name}}` in a UTF-8 file becomes the package name; `gitignore` is written as `.gitignore` (a default is written when the template has none); names starting with `.` are never copied.
- `init` writes `package.json` itself from `type`, `main` and `--mode`, so templates hold no conditionals.
- `--type` picks `cli/<type>`; `--template` overrides it with a mounted name or a folder on disk.
- Adding a template: a new folder with a `template.json` in a templates crate; `build.rs` embeds it, no Rust edit.

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

- A `desktop` project is built by `lunar build -p` (lunarstorm-pipeline): `compile` and `sign` refuse it, and `init` refuses `--mode node` for it.

- `compile` signs with a per-project development key (`.lunar/dev.der`, owner-only, made on first use): a `.jbn` cannot be unsigned.
- `compile` and `sign` build through `lunarstorm_host_cli::compile`, the path `jbn build` uses, so packages link on the Runtime and host modules `run` provides.
- `sign` recompiles rather than re-signing the built file; builds are deterministic, so the result is the same.

## Build and test

```text
cargo test -p lunarstorm-project
```
