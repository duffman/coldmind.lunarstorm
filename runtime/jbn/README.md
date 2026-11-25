# lunarstorm-jbn

Signed `.jbn` packages: an application's compiled modules, its resources and the
import table that links them, in one file a host verifies before running.
Ported from jetbunny's `src/jbn` (header layout, canonical CBOR, Ed25519 trust,
per-blob zstd) without its engine coupling: the container records which engine
wrote the bytecode, and never touches one.

## Public surface

- `Builder::new(name, version, release, engine, entry)`, `.module(name, bytecode, imports)`,
  `.resource(name, bytes)`, `.metadata(key, value)`, `.compression(Compression::Zstd)`,
  `.build(&Signer)` → bytes.
- `Package::open(bytes, &TrustStore, &Limits)` → a fully verified package:
  `manifest()`, `bytecode(module)`, `import(module, specifier)`, `resource(name)`,
  `digest()`, `signer()`.
- `Signer::{generate, from_pkcs8_der, to_pkcs8_der, public_key}`,
  `TrustStore::{from_json, to_json}`, `Trusted { public_key, packages, minimum_release }`.

`runtime/core` builds packages (`Runtime::package`) and runs them
(`Source::Package`); `runtime/hosts/cli` mounts `lunarstorm jbn key`,
`lunarstorm jbn build` and `lunarstorm run --trust <trust.json> app.jbn`.

## Layout

| offset  | field                                                   |
|---------|---------------------------------------------------------|
| 0       | `JBNY`, version `[1,0,0,0]`                             |
| 8/16/24 | manifest, signature, payload lengths (u64 LE)           |
| 32      | SHA-256 of the manifest                                 |
| 64      | manifest (canonical CBOR, integer keys `0..n`)          |
|         | signature: CBOR `[public key, Ed25519 signature]`       |
| aligned | payload: blobs at 8-aligned offsets, SHA-256 each       |

## Decisions

- `Package::open` verifies everything (header, manifest digest, signer and
  release against the trust store, manifest consistency, every blob digest)
  before returning; bytecode from a `Package` goes to the engine unchecked.
- The signature covers `"lunarstorm-jbn-v1\0" ‖ SHA-256(manifest)`. jetbunny's
  detached COSE structures were dropped: nothing consumes COSE.
- Every blob belongs to exactly one module or resource; builds are deterministic.
- Module names are the canonical build-time paths, so stack traces of a packaged
  app name its source files. A package therefore carries the build machine's paths.
- A package is a closed world: its modules resolve through the import table, then
  host modules and builtins; never the file system.

## Build and test

```sh
cargo test -p lunarstorm-jbn
cargo test -p lunarstorm-runtime --test modules     # build, verify, run on QuickJS
cargo test -p lunarstorm --test commands jbn        # the CLI end to end
```
