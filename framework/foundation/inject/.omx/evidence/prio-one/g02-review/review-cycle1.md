# G02 independent review cycle 1

Native code-reviewer g02_review; 8files, REQUEST CHANGES:2HIGH,1MEDIUM. Source tsc/diffchecks pass, all defects reproduced in read-only Bun probes.

HIGH: registration temporary keys are unretained until Binding ctor; nested failed registration inside delayed getter globally calls clearTokenCaches and collects outer in-progress keys. Scope-owned temporary retention and scoped failure cleanup needed; reentrant failure+explicitclear regressions.
HIGH: async instancehook awaits context.container.injectMembersAsync(instance), receives own pending completion and hangs. Ordinary capturedroot afterawait is indistinguishable from unrelated concurrency withoutambient; explicitcompletion-boundhookview required, notglobalbusy bypass.
MEDIUM: disposeWithTracking pins cold scope.order definitions (leaks despite []result), fails to pin inherited scoped record keys. Pin exactly actual returned DisposalInfo keys before retentionrelease; regressions empty/inherited/concurrent tracking.

All fixes delegated to g02_core_correctness; source gate remains active until red->green+focusedre-review.
