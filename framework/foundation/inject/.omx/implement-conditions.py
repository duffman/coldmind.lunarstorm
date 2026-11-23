from pathlib import Path
# A group preserves all single candidate identities in registration order.
f=Path('src/core/cminject.binding.ts');s=f.read_text().replace('export interface RegistrationSet {','export interface RegistrationSet {\n  singles?: Binding[];');f.write_text(s)
f=Path('src/core/cminject.mutation.ts');s=f.read_text();start=s.index('    const entry = this.host.registry.get(binding.key);');end=s.index('    this.host.epoch++;',start)
s=s[:start]+'''    const entry = this.host.registry.get(binding.key);
    const candidates = entry && 'singles' in entry && entry.singles ? entry.singles : entry?.single ? [entry.single] : [];
    if (!replacing) {
      if (binding.kind === 'single') {
        if (entry?.multi.length || (!binding.when && candidates.some(item => !item.when))) {
          binding.releaseTokens();
          throw new MutationConflictError(`Ambiguous unconditional registration for ${tokenName(binding.token)}`);
        }
        if (candidates.length) this.host.registry.set(binding.key, { single: candidates[0], singles: [...candidates, binding], multi: [] });
        else this.host.registry.set(binding.key, binding);
      } else {
        if (candidates.length) { binding.releaseTokens(); throw new MutationConflictError(`Service already registered for token: ${tokenName(binding.token)}`); }
        if (!entry || 'factory' in entry) this.host.registry.set(binding.key, { multi: [binding] });
        else entry.multi.push(binding);
      }
      this.host.order.push(binding);
    } else {
      this.host.indexGraph();
      if (replacing.kind === 'single') {
        const index = candidates.indexOf(replacing);
        if (index < 0) throw new MutationConflictError('Replacement registration changed during staging');
        const next = candidates.map(item => item === replacing ? binding : item);
        if (next.filter(item => !item.when).length > 1) throw new MutationConflictError('Ambiguous unconditional replacement');
        this.host.registry.set(binding.key, next.length === 1 ? binding : { single: next[0], singles: next, multi: [] });
      } else {
        if (!entry || 'factory' in entry) throw new NotRegisteredError(binding.token, tokenName(binding.token));
        const index = entry.multi.indexOf(replacing);
        if (index < 0) throw new MutationConflictError('Replacement registration changed during staging');
        entry.multi[index] = binding;
      }
      const index = this.host.order.indexOf(replacing);
      if (index >= 0) this.host.order[index] = binding;
    }
'''+s[end:]
s=s.replace("if (binding.kind === 'single') this.host.registry.delete(binding.key);",'''if (binding.kind === 'single') {
      const group = this.host.registry.get(binding.key);
      const candidates = group && 'singles' in group && group.singles ? group.singles.filter(item => item !== binding) : [];
      if (!candidates.length) this.host.registry.delete(binding.key);
      else this.host.registry.set(binding.key, candidates.length === 1 ? candidates[0]! : { single: candidates[0], singles: candidates, multi: [] });
    }''')
s=s.replace("for (const binding of [...(local.single ? [local.single] : []), ...local.multi])", "for (const binding of [...('singles' in local && local.singles ? local.singles : local.single ? [local.single] : []), ...local.multi])")
f.write_text(s)
f=Path('src/core/cminject.host.ts');s=f.read_text().replace('selectSingle(key: symbol, staged?: ReadonlyMap<symbol, Binding>): Binding | undefined;', 'selectSingle(key: symbol, staged?: ReadonlyMap<symbol, Binding>, context?: Resolver): Binding | undefined;');f.write_text(s)
f=Path('src/core/cminject.container.ts');s=f.read_text().replace('selectSingle: (key, staged) => target.locateSingle(key, staged)', 'selectSingle: (key, staged, context) => target.locateSingle(key, staged, context)')
s=s.replace('private locateSingle(key: symbol, staged?: ReadonlyMap<symbol, Binding>): Binding | undefined {','private locateSingle(key: symbol, staged?: ReadonlyMap<symbol, Binding>, context?: Resolver): Binding | undefined {')
s=s.replace('      const binding = c.localSingle(key);\n      if (binding) return binding;', '''      const entry = c.registry.get(key);
      if (!entry?.single) continue;
      if (!context) return entry.single;
      const candidates = 'singles' in entry && entry.singles ? entry.singles : [entry.single];
      const steps: ResolutionPathStep[] = [];
      for (let frame = context.frame; frame; frame = frame.parent) steps.push({ bindingId: frame.binding.id, tokenName: tokenName(frame.binding.token), scopeId: frame.target.identity.id });
      const path = steps.reverse(); const condition = { path, requester: path.at(-1), scope: this.identity };
      const match = candidates.find(binding => binding.when?.(condition)) ?? candidates.find(binding => !binding.when);
      if (!match) throw new NotRegisteredError(entry.single.token, `${tokenName(entry.single.token)} (no matching conditions or fallback)`);
      return match;''')
s=s.replace('  validate(): void { return this.observation.validate(); }','  validate(options?: ValidationOptions): void { return this.observation.validate(options); }');f.write_text(s)
f=Path('src/core/cminject.view.ts');s=f.read_text().replace('  validate(): void { return this.target.validate(); }','  validate(options?: ValidationOptions): void { return this.target.validate(options); }');f.write_text(s)
f=Path('src/core/cminject.resolution.ts');s=f.read_text().replace('this.host.selectSingle(key, ctx.staged)', 'this.host.selectSingle(key, ctx.staged, ctx)');f.write_text(s)
# Graph metadata never evaluates user predicates.
f=Path('src/core/cminject.contracts.ts');s=f.read_text().replace('export interface DiNode {','export interface DiNode {\n  conditional?: boolean;\n  multi?: boolean;\n  module?: string;');s=s.replace('export interface RuntimeDependencyNode {','export interface RuntimeDependencyNode {\n  module?: string;');f.write_text(s)
f=Path('src/core/cminject.observe.ts');s=f.read_text();s="import type { ValidationOptions } from '@coldmind/inject/contracts';\n"+s
s=s.replace('lifetime: binding.lifetime, deps: binding.deps, declared: binding.declared', 'lifetime: binding.lifetime, deps: binding.deps, declared: binding.declared, conditional: !!binding.when, multi: binding.kind === \'multi\', module: binding.module')
s=s.replace('    const single = this.host.registry.get(key)?.single;\n    if (single) this.addGraphNode(single);', "    for (const single of this.host.order.filter(binding => binding.key === key && binding.kind === 'single')) this.addGraphNode(single);")
s=s.replace('token: binding.token, lifetime: binding.lifetime, kind:', 'token: binding.token, module: binding.module, lifetime: binding.lifetime, kind:')
s=s.replace('bindingId: b.id, token: tokenName(b.token), ownerScopeId:', 'bindingId: b.id, token: tokenName(b.token), module: b.module, ownerScopeId:')
s=s.replace("if (liveIds.has(observed.from) && liveIds.has(observed.to))", "if (bindings.some(b => b.id === observed.from && b.generation === observed.fromGeneration) && bindings.some(b => b.id === observed.to && b.generation === observed.toGeneration))")
s=s.replace('  validate(): void {\n    this.host.active();\n    const issues = this.diGraph.issues();','''  validate(options: ValidationOptions = {}): void {
    this.host.active();
    const issues = this.diGraph.issues();
    if (options.requireDeps) for (const binding of this.host.order) if (!binding.declared) issues.push({ kind: 'undeclared', at: binding.key, message: `${tokenName(binding.token)}${binding.module ? ` [${binding.module}]` : ''} has no declared deps or inject tuple` });
    if (options.includeObserved) {
      issues.push(...this.drift());
      const graph = new DiGraph({ onIssue: () => undefined, onExpose: pinTokenKey });
      const bindings = this.host.view().flatMap(scope => scope.order);
      const edges = this.dependencyEdges();
      for (const binding of bindings) {
        const dynamic = edges.filter(edge => edge.type === 'dynamic' && edge.from.bindingId === binding.id && edge.from.generation === binding.generation).flatMap(edge => {
          const target = bindings.find(item => item.id === edge.to.bindingId && item.generation === edge.to.generation);
          return target ? [{ token: target.token, key: target.key, index: binding.deps.length, optional: false, multi: false }] : [];
        });
        graph.add({ key: binding.key, identity: binding.graphId, token: binding.token, lifetime: binding.lifetime, declared: binding.declared, deps: [...binding.deps, ...dynamic], conditional: !!binding.when, multi: binding.kind === 'multi' });
      }
      issues.push(...graph.issues().filter(issue => issue.kind === 'cycle'));
    }''')
pos=s.index('  /** Checks the declared-dependency graph');s=s[:pos]+'''  /** Current-generation dynamic edges absent from authored dependency lists. */
  drift(): DiIssue[] {
    const bindings = this.host.view().flatMap(scope => scope.order);
    return this.dependencyEdges().filter(edge => edge.type === 'dynamic').flatMap(edge => {
      const from = bindings.find(binding => binding.id === edge.from.bindingId && binding.generation === edge.from.generation);
      const to = bindings.find(binding => binding.id === edge.to.bindingId && binding.generation === edge.to.generation);
      return from && to && !from.deps.some(dep => dep.key === to.key) ? [{ kind: 'undeclared-edge' as const, at: from.key, message: `${tokenName(from.token)}${from.module ? ` [${from.module}]` : ''} resolves undeclared ${tokenName(to.token)}`, edge: { token: to.token, key: to.key, index: from.deps.length, optional: false, multi: false } }] : [];
    });
  }
'''+s[pos:]
f.write_text(s)
f=Path('src/core/cminject.graph.ts');s=f.read_text();needle='    for (const node of this.local.values()) found.push(...this.captivesOf(node));';s=s.replace(needle,'''    for (const identities of this.byToken.values()) {
      const unconditional = [...identities].map(id => this.local.get(id)!).filter(node => !node.conditional && !node.multi);
      if (unconditional.length > 1) found.push({ kind: 'ambiguous', at: unconditional[0]!.key, message: `Ambiguous unconditional providers for ${this.describe(unconditional[0]!.token)}` });
    }
'''+needle)
s=s.replace('message: `${this.describe(node.token)} depends on', 'message: `${this.describe(node.token)}${node.module ? ` [${node.module}]` : \'\'} depends on');f.write_text(s)
f=Path('src/cminject.inspect.ts');s=f.read_text().replace('  issues: DiIssue[];', '  issues: DiIssue[];\n  drift: DiIssue[];');s=s.replace('    issues: container.graph().issues(),','''    issues: container.graph().issues(),
    drift: container.dependencyEdges().filter(edge => edge.type === 'dynamic').flatMap(edge => {
      const nodes = container.getDependencyGraphSnapshot().nodes;
      const from = nodes.find(node => node.bindingId === edge.from.bindingId && node.generation === edge.from.generation);
      const to = nodes.find(node => node.bindingId === edge.to.bindingId && node.generation === edge.to.generation);
      return from && to ? [{ kind: 'undeclared-edge' as const, at: Symbol(from.token), message: `${from.token} resolves undeclared ${to.token}` }] : [];
    }),''');f.write_text(s)
print('Wrote ordered conditional candidates, observed graph validation and drift; no checks run.')
