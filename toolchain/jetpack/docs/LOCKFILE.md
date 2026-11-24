# rustnpm-lock.json (v5)

The lockfile records Jetpack's deterministic dependency graph using its own schema.

The existing filename and install recovery paths are retained across the rename.

The root contains the following fields.

| Field | Meaning |
| --- | --- |
| `lockfileVersion` | Schema version `5` |
| `dependencies` | Original root dependency ranges |
| `devDependencies` | Original root development ranges |
| `optionalDependencies` | Root optional dependencies; override ordinary entries |
| `peerDependencies` | Root peer constraints |
| `peerDependenciesMeta` | Root optional peer flags |
| `registrySources` | Default and scoped registry URLs (no authentication secrets) |
| `packages` | Locked records indexed by the exact installation path |

Package records contain `name`, `version`, `resolved`, `integrity`, `dependencies`,
`optionalDependencies`, `peerDependencies`, `peerDependenciesMeta`, `bin`, `optional`,
`peer`, `os` and `cpu`. Records are ordered deterministically by path.

## Offline integrity checks

`Lockfile::validate()` rejects incorrect root or transitive dependency
**versions**, missing required dependencies, incorrect peer versions, illegal peer
placement, orphan packages, unsafe package paths, invalid package URLs, invalid
integrity metadata, and required packages that do not support the validating host.
Optional packages for other platforms, and packages nested in them, are kept in the
graph and skipped at install. A lock can be replayed on another OS and CPU when its
required pinned releases support that target. Required release selection uses the
resolving host, so packages with different required releases for different targets
need target-specific resolution. The platform model covers `os` and `cpu`.
Each location pins one release. Conditional branches carry the platform constraints
needed by their required descendants and automatic peers.
An existing v5 lock that omitted a branch needs a fresh resolution to record it.

Optional dependencies may be omitted, but if a provider is installed and visible,
its version must satisfy the declared constraint. Root optional dependencies are
tracked and compared to `package.json` during `ci`.
Before activation, the installer checks required dependencies and peers again using the
staged packages after platform and optional failures have been processed.

The lock file pins dist-tags to versions selected at resolution time. `ci` cannot
reconfirm that `latest` still points to the same version without contacting a
registry; this is intentional reproducibility.

`ci` requires **v5 and the same registry routes**. `install` regenerates an older
lockfile or one for another registry. An `add` or `remove` operation prefers compatible
versions already present in the lock and retains the artifact URL and digest of reused
pins. A version change selects a new artifact from registry metadata.

After resolution, hoisting moves packages to higher `node_modules` scopes or merges
copies of the same pinned artifact. Each change must keep the graph valid and preserve
the provider instance visible to every consumer of a peer dependency.

## Installation crash recovery

Mutating CLI commands hold the OS advisory `.rustnpm-install.lock` before reading
`package.json` and the lockfile. They hold it through publication and cleanup.

`.rustnpm-transaction.json` stores a version 3 transaction identifier. The authoritative
recovery record lives outside the checkout in the current user's state directory under
`jetpack/transactions`, with the local data directory as a fallback. It records the
canonical project directory, its filesystem identity, transaction phases, and the
identities of staged, backup, and published outputs. This state is required even when
the checkout contains a readable journal.

The new `node_modules` and metadata updates are prepared under `.rustnpm-staging`.
During publication, original outputs move to `.rustnpm-backup` before staged replacements
move into place. `install` publishes the tree and lockfile. `add` and `remove` also
publish `package.json`. `ci` publishes the tree alone.

The next mutating command checks recovery ownership and validates every affected output
before changing files. An interrupted publication restores original outputs. A committed
publication keeps the new outputs and completes cleanup. Restoration and cleanup can
resume after interruption. Copied journals, version 1 or 2 journals without ownership
records, substituted directories, and changed output identities stop recovery and leave
project files available for inspection. A crash before a newly created staging directory
has its identity durably recorded also requires inspection. Recovery state belongs to the
user and project that created it, so moving a checkout or changing users can require
manual recovery of an unfinished transaction.

On Unix, publication syncs staged file contents and descendant directories before recording
a commit, and syncs directories around output moves. Windows flushes staged files and
uses volume/file identities for recovery. Windows directory-rename durability under power
loss remains unverified and is outside the current durability guarantee.

## Registry source pinning

`registrySources` records normalized URLs for the default registry and each
configured npm scope. It contains **no bearer tokens or credentials**. Package
records still pin each tarball's exact `resolved` URL and SRI integrity.
`install` re-resolves when the configuration changes. `ci` refuses to replay a
lockfile with different registry routing, even if a local cache has its tarballs.
Updating an older lock requires `jetbunny install` with the intended registry
configuration; `ci` accepts only v5.
