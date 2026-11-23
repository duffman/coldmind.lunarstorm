# Phase 1 correctness and operational safety

This document describes the original v4 implementation. For current platform and
transaction behavior, see [the lockfile reference](LOCKFILE.md).

Phase 1 modifies the existing Rust package manager; it does **not** implement
npm Arborist, lifecycle scripts, authentication, workspaces or non-registry
package sources.

## Guarantees implemented

1. Root `optionalDependencies` are resolved, omitted on failures, and persisted.
2. Registry candidates are filtered by OS and CPU before selection.
3. A bounded (256 attempts per scope), deterministic fallback search retries
   alternate ordinary **and automatically installed peer** releases after
   peer/transitive failures. This
   is **not** globally complete dependency backtracking.
4. `add` and `remove` prefer compatible versions from the previous v3/v4 lock.
5. V3 validates all required ordinary edges by version, not just existence,
   as well as peer visibility, orphan nodes, integrity metadata, and platform.
6. `ci` requires a current lock for the active OS and CPU. Older lockfiles migrate
   during `install` via full re-resolution, never during `ci`.
7. OS file locks prevent simultaneous jetpack mutations to one project.
8. A durable journal and backup directory allow crash recovery across the two
   `node_modules` activation renames. Both the staging and old tree remain on
   the same filesystem.
9. The lockfile/manifest each use fsync + sibling-temp atomic rename.
10. Package tarball URLs are parsed rather than checked by string prefix;
    redirects are disabled to prevent escaping the origin/transport policy.

## Tests

`tests/phase1_regressions.rs`: root optionals, platform filtering, dependency
version tampering, peer-conflict fallback, transitive fallback, preservation of
pins during add/remove, orphan graph rejection, and lockfile manifest matching.

`src/install.rs` tests: advisory lock contention and recovery in both crash
windows (before activation and after activation); unowned backup rejection.

Previous peer and tarball integration tests are preserved.

Run the single gate:

```sh
./scripts/check.sh
```

The CI workflow runs on ubuntu-latest, macos-latest, and windows-latest.

## Explicit remaining work

- The bounded scope retry algorithm is not globally complete npm Arborist
  peer-set backtracking/hoisting.
- Changes to `node_modules`, `package.json` and the lockfile are **not** one
  atomic cross-file transaction, although individual JSON writes are atomic.
- Lifecycle scripts, npmrc auth, workspaces, aliases, overrides, Git and local
  package specs, and npm package-lock compatibility remain unsupported.
- A cross-platform lockfile is not yet supported: v4 remains target-specific.
- A build has to pass the multi-OS CI gate before a release can be described
  as production-verified. No such execution was available in the authoring
  environment.
