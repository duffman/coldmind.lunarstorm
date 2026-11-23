# G13 independent code review cycle 1

Native code-reviewer g13_review, 16 files; REQUEST CHANGES. 0critical,1high,3medium. Typechecks and worker syntax/diffchecks passed; generated-file resolver failure reproduced by in-memory probe. Real-inject distribution fixture remains pending G07.

- HIGH ctsc/src/passes/cminject.paramtypes.ts:51 checks optional deps in argument TYPE instead of supplied runtime metadata. Annotated InjectableOptions with tags only silently disables emission. Test annotated options with/without explicit deps; inspect actual expressions/initializers.
- MEDIUM split/src/split.compiler.ts:45–47 onLoad-only generated file map cannot resolve nonexistent ctx.addFile paths before loading; client loader only consults transformed. Explicitly resolve generated paths/resolveDir, preserve directives/security, execute configured addFile fixture in both bundles.
- MEDIUM pass:62 fails to recognize computed static ['$inject'] and may overwrite explicit metadata. Handle statically resolvable computed names; preserve metadata in fixture.
- MEDIUM split compiler:22–36 reuses compiler.result.json without freshness correlation; zeroexit worker with nooutput accepts prior transforms. Fresh path/request ID and reject missing/mismatched output; regression after prior successful compile.

Main delegated all fixes to original g13_compiler performer; review evidence is not implementation completion. Full result preserved in native same-thread review.
