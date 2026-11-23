from pathlib import Path
# Review: conservative graph validation includes all possible candidates, without evaluating predicates.
f=Path('src/core/cminject.graph.ts');s=f.read_text();pos=s.index('  /** Every provider of a key');s=s[:pos]+'''  private locateCandidates(key: symbol): Vertex[] {
    const providers = this.localProviders(key);
    return providers.length ? providers.map(node => ({ graph: this, node })) : this.parent?.locateCandidates(key) ?? [];
  }
'''+s[pos:];s=s.replace("edge.multi ? this.locateAll(edge.key) : [this.locate(edge.key)].filter((v): v is Vertex => v !== undefined)", "edge.multi ? this.locateAll(edge.key) : this.locateCandidates(edge.key)");s=s.replace('      const nearest = resolver.locate(edge.key);\n      if (nearest) out.push(nearest);','      out.push(...resolver.locateCandidates(edge.key));');f.write_text(s)
# Review: get/peek must use the selected candidate, not whichever was cached first.
f=Path('src/core/cminject.container.ts');s=f.read_text();a=s.index('  private lookupCached(');b=s.index('  /** Synchronous read of a built singleton',a);s=s[:a]+'''  private lookupCached(key: symbol): StoredRecord | undefined {
    const binding = this.locateSingle(key, undefined, this.rootCtx());
    if (!binding) return undefined;
    for (let scope: Container | undefined = this; scope; scope = scope.parent) {
      const record = scope.records.get(binding);
      if (record?.state === 'ready') return record;
    }
    return undefined;
  }
'''+s[b:]
s=s.replace('      scope.disposing = true; scope.ambientState = undefined;','      scope.disposing = true; scope.ambientState = undefined;\n      if (!scope.parent) scope.moduleRegistry?.invalidate();')
s=s.replace('    const identities = new Map<string, ModuleOwnedBinding>();','    const identities = new Map<string, ModuleOwnedBinding>();\n    const cleanupErrors: unknown[] = [];')
s=s.replace('await scope.mutation.unregisterBinding(binding.id, true);','try { await scope.mutation.unregisterBinding(binding.id, true); } catch (error) { cleanupErrors.push(error); }')
s=s.replace('await scope.disposeRecord(record, true); scope.records.delete(record.binding);','const result = await scope.disposeRecord(record, true); if (result?.error) cleanupErrors.push(result.error); scope.records.delete(record.binding);')
s=s.replace('    } finally { this.rootContainer.registryRestoring = false; }','      if (cleanupErrors.length) throw new AggregateError(cleanupErrors, \'Snapshot cleanup failed\');\n    } finally { this.rootContainer.registryRestoring = false; }')
s=s.replace('  dependencyEdges(): RuntimeEdge[]', '  dependencyDrift(): DiIssue[] { return this.observation.drift(); }\n  dependencyEdges(): RuntimeEdge[]');f.write_text(s)
f=Path('src/core/cminject.module.ts');s=f.read_text().replace('  private closed = false;','  private closed = false;\n  private loading = 0;');s=s.replace('    this.active();\n    return [...this.entries.values()]','    this.active();\n    if (this.loading) throw new MutationConflictError(\'Cannot snapshot pending module loads\');\n    return [...this.entries.values()]',1)
s=s.replace('    const acquired = await Promise.all(modules.map(module => this.acquire(module, plan)));','    this.loading++;\n    let acquired: ModuleEntry[];\n    try { acquired = await Promise.all(modules.map(module => this.acquire(module, plan))); } finally { this.loading--; }');f.write_text(s)
f=Path('src/core/cminject.mutation.ts');s=f.read_text();needle='    await this.awaitBuilds(binding);\n    for (const record of this.recordsFor(new Set([binding.id])))';s=s.replace(needle,'    await this.awaitBuilds(binding);\n    const failures: unknown[] = [];\n    for (const record of this.recordsFor(new Set([binding.id])))')
s=s.replace('this.discardRecord(record); await this.host.disposeRecord(record, true);','this.discardRecord(record); const result = await this.host.disposeRecord(record, true); if (result?.error) failures.push(result.error);')
# Insert only into unregisterBinding, not other mutation return paths.
a=s.index('  async unregisterBinding(');b=s.index('  /**\n   * Registers an existing instance',a);part=s[a:b].replace('    return true;','    if (failures.length) throw new AggregateError(failures, \'Registration cleanup failed\');\n    return true;');s=s[:a]+part+s[b:];f.write_text(s)
f=Path('src/core/cminject.construction.ts');s=f.read_text().replace("if (out instanceof Promise) throw new LifecycleError('Deferred getSync requires synchronous construction');", "if (out instanceof Promise) { void out.catch(() => undefined); throw new LifecycleError('Deferred getSync requires synchronous construction'); }");f.write_text(s)
f=Path('src/core/cminject.contracts.ts');s=f.read_text().replace('  dependencyEdges(): RuntimeEdge[];', '  dependencyDrift(): DiIssue[];\n  dependencyEdges(): RuntimeEdge[];');f.write_text(s)
f=Path('src/core/cminject.view.ts');s=f.read_text().replace('  dependencyEdges(): RuntimeEdge[]', '  dependencyDrift(): DiIssue[] { return this.target.dependencyDrift(); }\n  dependencyEdges(): RuntimeEdge[]');f.write_text(s)
f=Path('src/cminject.inspect.ts');s=f.read_text();a=s.index('    drift: container.dependencyEdges()');b=s.index('\n  };',a);s=s[:a]+'    drift: container.dependencyDrift(),'+s[b:];f.write_text(s)
# Regression cases for the review's ownership and validation findings.
f=Path('tests/bun/core/delivery.test.ts');s=f.read_text()+'''
test('snapshot restores external values without disposal and module ownership survives restoration', async () => {
  const root=new Container(); let disposed=0; const value={onDispose(){disposed++;}}; root.registerInstance('external',value,{ownership:'external'});
  const module=defineModule({name:'restore-module',register(c){c.singleton('module-value',()=>1);}}); await root.load(module); const snapshot=root.snapshot();
  await root.replace('external',()=>({onDispose(){disposed++;}}),0); await root.restore(snapshot); expect(root.get('external')).toBe(value); expect(disposed).toBe(0);
  const [handle]=await root.load(module); await handle!.unload(); expect(root.has('module-value')).toBe(false); await root.dispose(); expect(disposed).toBe(0);
});
test('module unload aggregates cleanup failures and still removes every binding', async () => {
  const root=new Container(); const module=defineModule({name:'cleanup',register(c){c.singleton('bad',()=>({onDispose(){throw new Error('cleanup');}}));c.singleton('good',()=>({}));}});
  const [handle]=await root.load(module); root.resolveSync('bad');root.resolveSync('good');await expect(handle!.unload()).rejects.toBeInstanceOf(AggregateError);expect(root.has('bad')).toBe(false);expect(root.has('good')).toBe(false);await root.dispose();
});
test('static conditional candidates remain subject to conservative captive validation', async () => {
  const root=new Container();root.singleton('choice',()=>1);root.request('choice',()=>2,{when:()=>false});root.singleton('consumer',()=>3,{deps:['choice']});expect(()=>root.validate()).toThrow(DiGraphError);await root.dispose();
});
''';f.write_text(s)
Path('tests/bun/standard/deferred.test.ts').write_text('''import { expect, test } from 'vitest';
import { Container, createToken, Inject, lazy, provider, type Lazy, type Provider } from '@coldmind/inject';
test('standard fields and static members accept deferred declarations', async () => {
  const Value=createToken<number>('value');class Standard { @Inject(lazy(Value)) value!: Lazy<number>; @Inject(provider(Value)) next!: Provider<number>; }
  class Static { static $members={value:lazy(Value)};value!:Lazy<number>; }
  const root=new Container();let calls=0;root.transient(Value,()=>++calls);root.scoped(Standard);root.scoped(Static);const scope=root.createScope();const standard=scope.resolveSync(Standard);expect(calls).toBe(0);expect(await standard.next()).toBe(1);expect(standard.value.getSync()).toBe(2);expect(scope.resolveSync(Static).value.getSync()).toBe(3);await root.dispose();
});
test('legacy invocation preserves marker identity rather than unwrapping it eagerly',async()=>{
  const Value=createToken<number>('legacy-value');class Legacy { value!:Lazy<number>; }
  Inject(lazy(Value))(Legacy.prototype,'value');const root=new Container();root.singleton(Value,()=>7);root.scoped(Legacy);const service=root.resolveSync(Legacy);expect(service.value.peek()).toBeUndefined();expect(service.value.getSync()).toBe(7);await root.dispose();
});
''')
# Direct review record, not delegated approval or passing tests.
Path('.omx/evidence/prio-one/solo-final/critical-review.md').write_text('''# Direct critical review against PRIO_ONE

Reviewed all 28 original items after the integrated code pass and first typecheck/fix pass. No tests were run before this review.

Corrected: module-handle invalidation at actual root shutdown; pending-module snapshot rejection; disposal-error aggregation during unload/restore; exact-candidate cached reads; conservative validation of all conditional providers; generation-aware drift API reused by inspect; deferred synchronous rejection draining; exact typed-factory overloads across real Container/view/public interface. Added basic regression cases for review findings.

Mapping: 0.1 coverage config/existing floors;0.2 tests/FEATURES final count refresh pending execution;0.3 existing characterization;0.4 scripts/bench*.ts;1.1 token retention;1.2 React adapter;1.3 construction completion;1.4 resolution controls;1.5 contracts/decorators/type fixtures;2.1 extracted core modules;2.2 explicit view;2.3 root ambient/server;2.4 package exports/import checkers;3.1 inject build conditions;3.2 decorate build;3.3 tests/node;4.1 module registry+registration facade;4.2 snapshots/testing entry;4.3 lazy/provider construction+decorators;4.4 tuple overloads/operational adapter;4.5 graph/observe/inspect;4.6 token qualify;4.7 candidates/selection;4.8 module composition/type fixtures;4.9 ctsc pass/split integration;5.1 docs+TypeDoc config;5.2 examples+docs tests;6.1 records updated with final results still pending.

This is source review, not a claim of runtime or distribution success. Next: affected typechecks, then final test/build/coverage/benchmark execution and failure-driven fixes. No subagents used in this implementation/review.
''')
print('Direct review findings repaired; runtime tests remain unrun.')
