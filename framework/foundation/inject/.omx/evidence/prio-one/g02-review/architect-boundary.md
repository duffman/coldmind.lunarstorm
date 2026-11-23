# G02 completion context boundary

Native architect completion_context_boundary: APPROVE IMPLEMENTATION DIRECTION; not planrestart, currentimplementation WATCH untilverified. This clarifies existing explicitResolver architecture and requirement1.3, not a claim of universal asynccontext inference.

Each hook gets an explicit live completion lineage (rootownedstate, attemptidentity, parentlink, validity). CompletionView shares existing target-backedforwarding, nofakeBinding/StoredRecord and no secondProxy. Typed contextual callbacks forinjectMembers*/instantiate* preserve lineage; resolution methods propagate both resolver andcompletion lineage tofactoryviews. Checkvalidancestor beforependingjoin, contextual reentry returns alreadyinjected/wrappedinstance without rerunninghook. Ordinaryexternal callersstilljoinfullpending. Expireeachhooklink on success/failure; validateattempt and ignoreexpiredlinks whilekeepingvalidparents so retainedfacade cannot bypassretry.

HookcontextJSDoc: use context.container forhookoperations including afterawait. Reinjectionthroughactivehookcontext isidempotent anddoesnotawaitthehookitself. Ordinarycapturedcontainerscarrynohookcontext; awaiting theirpendingcompletion fromsamehook isunsupported userpromisecycle. Do NOT claimserverambient orfactorydeadlineautomaticallycoversstandalonememberinjection; currentfill/ambient paths do not.

Verifybothruntimes: async hookreentry withunrelatedcallerremainingpending; nestedA->B->A; retainedcontexts success/failure/retry; manual+registeredclass paths; prior sync/perroot/retry/wrappers. ExistingforwardinglaterG05explicitIContainerdelegation; pullingforwardtypedboundary ifrequired isalreadyapprovedarchitectureC.

Antithesis: reject contextual reentry withLifecycleError. Tradeoff: breaks established synchronous idempotence. Synthesis: idempotence throughliveexplicitcontextonly, completewaitsforordinarycallers.
