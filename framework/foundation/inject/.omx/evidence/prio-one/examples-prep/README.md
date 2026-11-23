# G15 / PRIO5.2 executable example drafts

Thirteen new `examples/cminject.<guide>.ts` files each export `runExample(): Promise<string>`. `manifest.json` maps every named guide to its example and exact expected result, and lists pending public API assumptions. These are actual deterministic demonstrations, not future-feature descriptions.

The examples use package public entries only, avoid network/credentials/global application mutation, and dispose roots, SSR scopes and request leases through `finally`. Root disposal owns ordinary child scopes; successfully returned transients in these examples contain no resources. The module example composes named typed definitions, loads the composed module once, and checks explicit unload. The testing example checks definition reconstruction, snapshot removal and atomic rejection of an owned supplied-instance override. The React example performs real `renderToString` on a pre-resolved request scope; it does not depend on DOM globals or JSX. Migration examples import no source-container libraries.

## Verification boundary

- In-place TypeScript **5.9.3** `transpileModule` in standard-decorator mode: **13/13 files, zero syntax diagnostics** (`syntax.log`). No downloaded compiler or private cache was used.
- All thirteen guide files exist and have balanced Markdown fences. Example entrypoint shapes and the public-import allowlist were checked.
- This is **syntax-only preparation**, not semantic typecheck, runtime or build verification. No examples were executed against the current incomplete exports. No active runner, tests, configuration, package manifest, source API, documentation, existing basic example or workflow state was changed.
- Future G15 integration must execute each manifest entry on Bun and Node after G07–G13, assert the exact result, run semantic typechecking and the normal package checks, and adapt any assumptions explicitly listed in `manifest.json` to final integrated API signatures.

## Source contracts read

All thirteen `docs/*.md` guides; `.omx/plans/prd-prio-one.md` (module/testing/deferred/conditions contracts and G15); `.omx/plans/test-spec-prio-one.md` (4.1–4.7 and 5.2); architecture review notes; prepared module definition implementation/evidence; current public barrels, contracts, decorator/server/React/inspection implementations. The module leaf is prepared separately and is not yet integrated into the public barrel or container loader.
