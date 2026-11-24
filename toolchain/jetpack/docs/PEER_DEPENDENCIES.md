# Peer dependency resolution

`jetpack` installs and validates **required** `peerDependencies`, including
transitive peers. It uses the Node `node_modules` visibility model and strict
peer conflict rejection. Peer data is persisted in `rustnpm-lock.json` v5.

## Placement

For a consumer at `node_modules/host/node_modules/plugin`:

- An ordinary dependency `react` belongs at
  `node_modules/host/node_modules/plugin/node_modules/react`.
- A peer dependency `react` belongs at
  `node_modules/host/node_modules/react`, or can reuse a compatible provider
  further up the ancestor chain (`node_modules/react`).
- A `react` package installed *inside* `plugin/node_modules` cannot satisfy a
  peer. The lockfile validator rejects this layout.

Required peers absent from the host scope and its ancestors are auto-installed
as siblings. When an ancestor has an incompatible version, the resolver can
install a compatible version in the nearer host scope. A conflicting package
**already installed in the host scope as an ordinary dependency** produces an
`ERESOLVE` error and cannot be silently replaced.

## Version constraints

1. All ordinary dependencies in a scope are selected **before** resolving any
   peers; dependency key order does not change the result.
2. The resolver collects peer constraints from every immediate sibling, plus
   root `package.json` peer declarations for the root scope.
3. An existing compatible provider is reused. Otherwise, the highest registry
   version satisfying **all** peer semver ranges is selected.
4. Auto-installed peers can declare further peers; the process repeats until
   stable. Peer cycles terminate when every requirement is satisfied.
5. Unresolvable ranges, incompatible declared host versions, or non-convergent
   peer graphs fail instead of producing a known-invalid tree.

**Supported peer specifications:** npm semver expressions (`^18`, `~5.1`,
`>=1 <3`, `^18 || ^19`, exact versions). Registry dist-tags, `npm:` aliases,
Git, `file:` and workspace specs as *peer requirements* are not yet supported.
They fail explicitly rather than being treated as `*`.

## Optional peers

`peerDependenciesMeta: { "react": { "optional": true } }` means:

- If no compatible provider is visible, the optional peer is **not** installed.
- If one is already visible, its version **must** satisfy the range.
- If another consumer requires the same peer, it may still be installed
  automatically, but the resulting version must satisfy the optional peer too.
- Invalid metadata referencing an undeclared peer is rejected.

This behavior is stricter than npm's default non-strict peer conflict fallback.
It is intentional to avoid shipping invalid dependency graphs.

## Example

Root dependencies:

```json
{
  "dependencies": {
    "react": "19.0.0",
    "host": "1.0.0"
  }
}
```

`host@1.0.0` requires `plugin@1.0.0`, which declares
`peerDependencies: { "react": "^18" }`. The resolver produces:

```text
node_modules/
  react/                  # 19.0.0 (explicit root dependency)
  host/
    node_modules/
      plugin/             # 1.0.0
      react/              # 18.x (auto-installed peer for plugin)
```

## Lockfile and `ci`

The lockfile records each package's `peerDependencies`,
`peerDependenciesMeta`, ordinary `dependencies` and `optionalDependencies`,
and a `peer: true` bit for automatically installed peers. Root peer declarations
are recorded too. `jetbunny ci` validates every peer's nearest visible provider,
version and optionality without fetching registry metadata. Cache misses can still
require tarball downloads. Missing, incompatible and incorrectly nested peers fail
validation before publication.

`jetbunny install` regenerates older lockfiles into v5. `jetbunny ci`
requires v5; regenerate the lock with `jetbunny install` first.

## Solver boundaries

- The resolver tries compatible ordinary and automatic-peer releases before dropping
  an optional consumer. Search is bounded to 256 attempts per scope and 20,000 overall.
- Graph-preserving hoisting runs after resolution. Globally complete npm Arborist
  backtracking and arbitrary cross-scope rearrangement are unsupported.
- A required peer can narrow a branch with an ordinary optional ancestor. Narrowing
  through shared automatic peers remains conservative.
- Peer dist-tags, non-registry requirements, workspaces and aliases are unsupported.
- Registry routing and bearer authentication apply to peer metadata and tarballs.
  Lifecycle scripts are never executed.

## Test coverage

`tests/peer_dependencies.rs` exercises automatic peers, compatible and incompatible
hosts, overlapping constraints, nested placement, scoped packages, optional peers,
cycles, transitive peers, optional-dependency rollback, root peers and lock tampering.
`tests/phase1_regressions.rs` covers fallback, artifact reuse and optional platform
constraints. JetBunny's `tests/package_manager.rs` exercises the commands against
local HTTP registries. The fixtures require no public network access.
