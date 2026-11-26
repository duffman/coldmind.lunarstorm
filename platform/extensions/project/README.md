# lunarstorm-project (platform/extensions/project)

The `project` package of the `lunar` command line, mounted by `apps/lunarstorm` behind the `project` feature (on by default).

```text
lunar project init [dir] --type module|program --mode lunar|node
lunar project insight [dir]                     description, dependencies, files and lines per extension, build
lunar project compile [dir]                     build/<name>.jbn (runtime bytecode) + build/trust.json
lunar project sign [dir] --key <developer.json> build/<name>.jbn signed with an issued developer key
```

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

- `compile` signs with a per-project development key (`.lunar/dev.der`, owner-only, made on first use): a `.jbn` cannot be unsigned.
- `compile` and `sign` build through `lunarstorm_host_command_line::compile`, the path `jbn build` uses, so packages link on the Runtime and host modules `run` provides.
- `sign` recompiles rather than re-signing the built file; builds are deterministic, so the result is the same.

## Build and test

```text
cargo test -p lunarstorm-project
```
