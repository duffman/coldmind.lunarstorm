# Architect review cycle 1

Verdict: ITERATE. Sequential review after dedicated planner. Reviewer: native architect full_plan_architect. Scope: requirements, context, PRIO_ONE and PRD/test-spec with targeted source inspection. No product edits or validation performed by reviewer.

## Required lifecycle correction
registerInstance's factory closes over the supplied value (container.ts:665–678). Replaying its definition cannot reconstruct an owned object already disposed. Reject testing overrides of owned registerInstance BEFORE replacement/disposal with MutationConflictError. Snapshot restore retains unchanged live supplied records; if a captured owned supplied value has since been disposed, reject atomically in restore preflight before any mutation. External supplied values may be restored without disposal; factory-backed bindings rebuild normally. Do not silently change override retainPrevious:false.

Required tests: rejection leaves binding/instance/disposal count unchanged; unchanged snapshot preserves supplied identity; disposed-original restore conflicts atomically; external restore never disposes; factory-backed restore rebuilds. Distinguish cancelled unreturned owned transient cleanup from successfully returned ordinary transients, which preserve existing ownership.

## Other findings
Internal token retention/public pinning addresses cached key identity. Unified build controls address first caller/transients/init/dispose rather than only share. Typed host with one-way prototype attachment preserves instanceof and avoids cycles. Root-owned modules are defensible given explicit tests for descendants/shared imports/concurrency/rollback. Named capability maps avoid false nominal symbol typing. Native ctsc APIs and split's missing hook are correctly recognized.

## Antithesis / tradeoff / synthesis
Antithesis: unified registry/resource transaction engine first. Tension: shared machinery avoids semantic divergence, but upfront rewrite threatens sync paths and current ownership/registration. Synthesis: incremental delivery with common binding identity and preflight/commit/dispose invariants; explicitly reject irreconstructible resource restoration.

Full native review is in the same-thread agent result. This artifact preserves its actionable findings and provenance as lifecycle evidence, not security authority.
