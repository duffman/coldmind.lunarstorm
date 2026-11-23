# Research: migration-guide external API audit

## Request type and result

Version-aware official-documentation audit, retrieved **2026-10-27**. Input documents were read only; no product, docs, package, configuration or git state was changed.

**External claims are supported, with one wording correction recommended before publication:** qualify the `clearInstances` sentence in `docs/migrate-tsyringe.md:54` as cminject behavior. The source library behaves differently. This research does not certify the target cminject examples or pending feature integrations.

## Version certainty

- InversifyJS: requested package **8.2.3** confirmed by official npm release metadata (`inversify-release.json`). Official container documentation identifies itself as **8.x**, with an 8.2 banner. This supports the guide's 8.x vocabulary, not a claim that every future patch is identical.
- TSyringe: official npm metadata for **4.10.0** supplies commit **e033769d97cfb6cc4a8569e2b50eb32015453302** (`tsyringe-release.json`). The README and narrow source check were fetched at that full SHA. The guessed `v4.10.0` GitHub tag did not exist; no tag-based version claim is made.
- Nest: current official documentation was checked; no Nest package is installed or pinned by this task. The site offers a separate Version 11 navigation, so the unversioned current pages must not be described as specifically Nest 11 or any exact installed version.

## Claim audit

| Draft location | Material source claim | Finding and reusable source |
| --- | --- | --- |
| `migrate-inversify.md:3,7–12` | Bind classes/identifiers; sync `get`; async `getAsync`/`getAllAsync`; hierarchy | Supported by [8.x container API](https://inversify.io/docs/api/container/). `get` constructs/resolves synchronous bindings; async APIs return promises. |
| `migrate-inversify.md:13,47` | Snapshot stack workflow differs from target | Supported: source `snapshot()` and `restore()` return void, restoring the last snapshot. [Snapshot API](https://inversify.io/docs/api/container/#snapshot). Target snapshot semantics still need local tests. |
| `migrate-inversify.md:41,45` | Explicit lifetime migration; activation/deactivation exist | Supported; source binding default is transient and activation/deactivation are documented. [Container options and hooks](https://inversify.io/docs/api/container/). Target hooks are migration choices, not asserted equivalents. |
| `migrate-tsyringe.md:3,7–12` | Class/value/factory/token providers; resolve/resolveAll | Supported in the [4.10.0 README](https://github.com/microsoft/tsyringe/blob/e033769d97cfb6cc4a8569e2b50eb32015453302/README.md#providers). Factory receives the dependency container; a token provider aliases another token. |
| `migrate-tsyringe.md:13–16` | ResolutionScoped differs from ContainerScoped | Supported: per-resolution-chain versus per-container cache, with distinct child instances. [Pinned scope documentation](https://github.com/microsoft/tsyringe/blob/e033769d97cfb6cc4a8569e2b50eb32015453302/README.md#available-scopes). |
| `migrate-tsyringe.md:18,46,48` | Global registration effects and legacy constructor decorators | Supported. [Pinned decorator documentation](https://github.com/microsoft/tsyringe/blob/e033769d97cfb6cc4a8569e2b50eb32015453302/README.md#decorators) and [configuration](https://github.com/microsoft/tsyringe/blob/e033769d97cfb6cc4a8569e2b50eb32015453302/README.md#installation) describe global helpers, legacy decorator flags and reflection. |
| `migrate-tsyringe.md:52` | Source delayed object need not match target lazy object | Supported: TSyringe delay creates a proxy whose first use constructs the service. [Pinned delay documentation](https://github.com/microsoft/tsyringe/blob/e033769d97cfb6cc4a8569e2b50eb32015453302/README.md#the-delay-helper-function). |
| `migrate-tsyringe.md:54` | Unqualified clearInstances keeps supplied values | **Ambiguous attribution; correct wording.** TSyringe removes value-provider registrations. [Pinned implementation](https://github.com/microsoft/tsyringe/blob/e033769d97cfb6cc4a8569e2b50eb32015453302/src/dependency-container.ts#L405-L420). |
| `migrate-nest.md:5,9–12` | useClass/useValue/useFactory/useExisting | Supported by [custom-provider documentation](https://docs.nestjs.com/fundamentals/custom-providers). Existing-provider aliases can preserve singleton identity; the guide correctly requires identity tests rather than asserting a universal target equivalence. |
| `migrate-nest.md:11,17` | inject list supplies ordered factory arguments | Supported by [factory-provider documentation](https://docs.nestjs.com/fundamentals/custom-providers#factory-providers-usefactory). |
| `migrate-nest.md:13–17` | Provider exports are visibility boundaries | Supported: providers are scoped to their declaring module and exported to expose them elsewhere. [Export custom provider](https://docs.nestjs.com/fundamentals/custom-providers#export-custom-provider). Target root-owned registration remains separately unverified here. |
| `migrate-nest.md:51` | Implicit contrast with Nest request-scope propagation | Supported: request scope propagates to consumers. [Scope hierarchy](https://docs.nestjs.com/fundamentals/injection-scopes#scope-hierarchy). Do not infer that transient scope propagates identically. |
| `migrate-nest.md:53` | Framework lifecycle phases are not generic container readiness | Supported by [Nest lifecycle events](https://docs.nestjs.com/fundamentals/lifecycle-events). Hooks distinguish module initialization, application bootstrap and shutdown. Request-scoped classes do not receive these application hooks. |
| All guides' TypeScript fences and target column | cminject behavior and executable examples | **Not externally verifiable.** Retain as target patterns pending local typecheck/runtime execution after G08–G12; research approval is not implementation completion. |

## Exact recommended edits for the docs owner

1. **Required clarity repair, `migrate-tsyringe.md:54`:** replace the ambiguous sentence with: “cminject's `clearInstances` keeps supplied instances and external values; it is not unregister-all. TSyringe 4.10 instead removes value-provider registrations when clearing instances, so test this migration boundary explicitly.” The cminject half must remain conditional on its local verification gate. Evidence: pinned source lines405–420 above.
2. **Version grounding, `migrate-tsyringe.md:3`:** link to the pinned 4.10.0 README above and say this guide was checked against 4.10.0; advise readers to check their own version. The current moving README is not proof of exact-version behavior.
3. **Optional helpful clarity, `migrate-tsyringe.md:52`:** explicitly name “TSyringe's delay proxy” before contrasting the target. This makes the already-correct warning easier to apply.
4. **Optional lifecycle caveat, `migrate-nest.md:53`:** add that Nest's listed application lifecycle hooks do not run for request-scoped classes, whereas target request cleanup must be tested explicitly. Link the lifecycle source above.
5. **Version caveat, `migrate-nest.md:5`:** retain generic Nest wording or add the retrieval date; do not label these unversioned pages as a verified exact Nest major.

No correction to the existing InversifyJS source vocabulary is required. The existing caution that mappings are choices rather than equal scope/ownership semantics is important and should stay.

## Source-reference evidence

Pinned supplemental source (used only to remove README ambiguity about value-provider registrations):
`microsoft/tsyringe@e033769d97cfb6cc4a8569e2b50eb32015453302:src/dependency-container.ts:L405-L420`.

Local evidence copies contain official release metadata and the two pinned upstream files. No third-party evidence, dependency adoption recommendation, target implementation inspection, product edits or runtime-suite claims were used. No verbatim upstream quotations are included in this report.
