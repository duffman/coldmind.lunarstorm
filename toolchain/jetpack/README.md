# jetpack v0.4.0

The npm registry resolver and installer behind `jetbunny install|add|remove|ci|list`,
as a Rust library: bounded dependency version fallback, strict peer enforcement,
hoisting, validated v5 lockfiles and recoverable `node_modules` activation.
The public Rust library accepts an embedder-supplied `transport::Transport`.
JetBunny provides the CLI and uses its builtins' network runtime.

## Use

See [the package-manager guide](../../../docs/package-manager.md) for commands,
installation checks and registry configuration.

| Area | Current support |
| --- | --- |
| Commands | `jetbunny install`, `add`, `remove`, `ci` and `list` |
| Flags | `--registry URL`, plus `add --dev` / `-D` |
| Dependencies | Registry semver and dist-tags, scoped packages, dev and optional dependencies, strict peers |
| Locking | Custom v5 `rustnpm-lock.json`, artifact integrity, registry routes and pinned replay |
| Layout | Nested `node_modules`, graph-preserving hoisting and JetBunny bin shims |
| Registries | Project/user `.npmrc`, scoped routes, bearer tokens and custom CA roots |
| Runtime | ESM, literal CommonJS `require`, JSON, TypeScript and package export mappings |

The CLI/runtime admits at most 256 packages including the root project.
The resolver has a separate 2,048-package ceiling, depth 48, 256 attempts per scope
and 20,000 attempts overall. Runtime compatibility is described in
[the source-package guide](../../../docs/source-packages.md).

```rust,ignore
let mut registry = Registry::with_config(config.clone(), transport.clone())?;
let lock = resolve::resolve(&manifest, &mut registry)?;
install::install_locked(project, &lock, &guard, &InstallOptions {
    transport: transport.as_ref(), config: Some(&config), verify: Some(&check),
})?;
```

## Implemented

- Reads and updates `package.json` dependencies and devDependencies while preserving other JSON fields.
- Uses the official npm registry API (`Accept: application/vnd.npm.install-v1+json`).
- Resolves Node/npm semver ranges and registry dist-tags via `node-semver`.
- Resolves transitive dependencies and **root**/transitive optional dependencies.
- Tries alternative ordinary and automatic-peer versions on peer/transitive conflicts.
  Same-scope peer conflicts prioritize the involved packages. The resolver reports
  when it reaches the 256-attempt limit for a scope.
- On `add`/`remove`, retains compatible locked artifact URLs, integrity and dependency
  declarations. Changes to registry metadata for an existing pin produce a warning.
- Resolves and auto-installs **required peerDependencies**, with npm semver intersection
  across all immediate siblings, recursive peer-of-peer handling and cycle protection.
- Enforces strict conflicts (`ERESOLVE`), including against explicit root dependencies;
  never silently changes a declared dependency version to satisfy a peer.
- Supports `peerDependenciesMeta.optional` (absent optional peers are not installed;
  present incompatible peers are rejected).
- Resolves scoped peers correctly, including nested peer scope shadowing.
- Validates peer visibility and semver from the lockfile without metadata resolution during `ci`.
- Avoids infinite cycles when an ancestor already provides the exact selected release.
- Filters npm `os` and `cpu` constraints for required packages during resolution; optional
  packages for other platforms stay in the lock and are skipped at install.
- Installs a deterministic `node_modules` graph. Hoisting preserves each plugin's
  peer provider bindings and revalidates the resulting graph (`Lockfile::dedupe`).
- Validates SHA-512/SHA-384/SHA-256/SHA-1 Subresource Integrity and legacy npm SHA-1 shasum.
- Uses a persistent, integrity-checked tarball cache in the OS cache directory.
- Rejects unsafe archive paths, links, special files and archive-supplied `node_modules`.
  Per archive, limits compressed bytes to 256 MiB, regular-file data to 4 GiB, entry
  count to 100,000 and each supported extension header to 64 KiB. Across one install,
  extraction is limited to 8 GiB of regular-file data and 500,000 entries.
- Checks binary targets before discarding failed optional branches. Writes
  `node_modules/.bin` shims (`sh` and `.cmd`) for the surviving packages.
- Uses `rustnpm-lock.json` **v5**, with root optional dependencies and version checks
  for every dependency edge. Conditional optional packages are retained across targets.
  Every required pin must support the installing OS and CPU. `install` migrates older
  locks by re-resolving; `ci` requires v5 and the same registry routes.
- Fetches metadata and tarballs through the embedder's transport. The default scheduler
  runs up to eight GETs in flight per batch and returns results in request order.
  Batches hold up to 64 MiB of metadata or 128 MiB of tarball payloads. Each resolution can
  process up to 256 MiB of metadata. Larger tarballs are fetched individually after
  a typed body-size failure. Other tarball failures preserve their diagnostic.
  Follows up to five redirects and chooses the bearer token per hop.
- Accepts an optional per-package verify callback that runs before activation.
- `ci` uses pinned artifacts without resolving or mutating `package.json` or the lockfile.
- Extracts into a staging directory and checks required edges after optional packages are
  skipped. The project's advisory lock and journal cover activation and metadata updates,
  with automatic recovery before the next mutation reads project files.
- Prevents unsafe URL user-info spoofing; every redirect hop passes the same URL policy.
- Configures local or remote registries through CLI, environment and `.npmrc`,
  including scoped registry routing and full-origin/path-bound bearer authentication.
- Reads `cafile=` from `.npmrc`; the embedder's transport trusts only those roots.
- Stores registry routes (not tokens) in the lockfile and rejects stale routes during `ci`.

## Explicit limitations

The supported installation contract is limited to registry packages usable by
JetBunny. The following capabilities are unsupported.

- npm-compatible `package-lock.json` import/export (the lockfile format is custom).
- Full npm Arborist peer-set backtracking, overrides, aliases (`npm:`),
  workspaces, `file:`, Git and direct tarball-URL dependencies. Peer dist-tags and non-semver specs
  are rejected with an explicit error.
- Basic username/password authentication, client certificates, global npmrc and npm publish support (bearer tokens, `cafile` and project/user `.npmrc` registry mappings are supported).
- Lifecycle scripts (`preinstall`, `install`, `postinstall`, etc.) are **never executed**. Packages needing native builds or postinstall setup will not work.
- `engines` and `libc` enforcement, bundled dependency installation and full npm
  optional-dependency behavior. Unsupported manifest fields can be preserved without
  applying their npm semantics.
- Hardlinks/symlinks inside published tarballs; these are rejected, not followed.
- Enforced offline mode, production-only installs (`--omit=dev`), store-level
  deduplication, global installs, update, audit and publish commands.
  `ci` skips metadata resolution; cache misses still need tarball downloads.
- The project uses its own binary collision handling: duplicate `.bin` names at the same scope result in an error.
- Bin paths must work in both POSIX and Windows shims. Spaces and apostrophes are supported.
  Paths containing control characters, double quotes, or percent signs are rejected.
- Installation replaces the existing `node_modules` directory (third-party content there is not preserved).
- The lockfile is registry-specific: changing registry routes requires re-resolution.

All root dev dependencies are installed. The bounded solver can reject graphs that
need global backtracking. Recursive peer-of-peer platform narrowing remains
conservative when a shared provider's consumers cannot be narrowed together.

## Architecture

| File | Responsibility |
| --- | --- |
| `src/transport.rs` | the embedder's transport, redirects and per-hop credentials |
| `src/manifest.rs` | `package.json` and package spec parsing |
| `src/registry.rs` | npm metadata, registry routing, semver resolution and platform checks |
| `src/config.rs` | `.npmrc`, loopback registry URLs, scoped registry routes and bearer tokens |
| `src/resolve.rs` | nested dependency/peer constraint resolution and lock graph |
| `src/peer.rs` | peer visibility, host scope and semver checks |
| `src/lib.rs` | documented public Rust library API |
| `src/lockfile.rs` | reproducible lockfile and safe install paths |
| `src/integrity.rs` | npm integrity checks |
| `src/install.rs` | secure archive extraction, caching, binaries and staging |

## Testing

```sh
cargo fmt -p jetpack -- --check
cargo test --locked -p jetpack --all-targets
cargo test --locked -p jetpack --doc
cargo clippy --locked -p jetpack --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked -p jetpack --no-deps --document-private-items
```

Or run `./scripts/check.sh` to execute the full quality gate.

Rustdoc HTML is generated under `target/doc/jetpack/index.html`.
See [`docs/PEER_DEPENDENCIES.md`](docs/PEER_DEPENDENCIES.md) for peer placement,
conflict rules, and unsupported Arborist features.

Phase 1 also adds `tests/phase1_regressions.rs` for lockfile version rejection,
platform mismatch, root optionals, deterministic fallback, stable pins, graph
reachability and rollback. The JetBunny workspace workflow runs the crate tests on
Linux and macOS, while the product CLI suite is configured for Linux, macOS and
Windows. Release support requires successful runs on the target platform.
See [`docs/LOCKFILE.md`](docs/LOCKFILE.md) for the current transaction contract.

`cargo test` exercises manifest parsing, integrity hashes, semver/tag resolution,
platform constraints, archive safety, lock path validation, binary path validation,
peer conflict detection, optional peers, transitive peers, scoped packages,
root peer requirements, peer cycles and offline lock tampering checks. The peer
integration suite uses a local mock npm registry over a loopback test transport
(`tests/support`). jetbunny's `tests/package_manager.rs` runs the commands end to end.

## Registry

The default registry is `https://registry.npmjs.org`. You can target a
self-hosted npm-compatible HTTP registry on your own machine:

```sh
jetbunny install --registry http://127.0.0.1:4873
```

Or configure `.npmrc` alongside `package.json`:

```ini
registry=http://127.0.0.1:4873/
@acme:registry=http://127.0.0.1:4873/
//127.0.0.1:4873/:_authToken=my-local-token
```

The scope-specific route also applies to transitively resolved dependencies and
peers. Registry metadata can provide absolute tarball URLs or relative paths;
tarball requests receive credentials only when their origin/path matches the
configured token scope. Registry routes (never secrets) are recorded in the
lockfile. Changing a registry regenerates the graph with `install`; `ci` fails closed.

Remote hosts use HTTPS. An HTTP bearer token is selected only for an explicitly
configured loopback registry origin. Requests whose literal URL host is local require
an explicitly configured registry origin. Request diagnostics redact credentials and
query strings. For `.npmrc` precedence, destination policy and unsupported settings,
see [`docs/REGISTRIES.md`](docs/REGISTRIES.md).

## Security notes

A matching integrity digest identifies the registry-selected artifact. Package trust
depends on the registry and the reviewed lockfile. Lifecycle scripts are disabled.
Archive extraction and staged dependency validation run before publication.

## License

MIT
