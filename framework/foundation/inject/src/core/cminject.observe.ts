import {
  ServiceLifetime, type BindingDescriptor, type CacheableBinding, type DiIssue, type DiscoveredClass,
  type InstanceRecord, type InstanceState, type MultiBindingEntry, type OwnershipRecord, type RuntimeEdge,
  type RuntimeDependencyEdge, type RuntimeDependencyGraph, type RuntimeDependencyNode, type RuntimeDependencyTreeNode,
  type RuntimeResourceEdge, type RuntimeResourceGraph, type RuntimeResourceNode, type ScopeTreeNode,
  type ValidationOptions,
} from '@coldmind/inject/contracts';
import { lookupToken, pinTokenKey, releaseTokenKey, retainTokenKey, tokenName, type InjectionToken } from '@coldmind/inject/token';
import { DiGraph } from '@coldmind/inject/graph';
import { DiGraphError } from '@coldmind/inject/errors';
import { idOf, lifetimeText, type Binding, type StoredRecord } from '@coldmind/inject/internal/binding';
import type { ObserveHost, ObserveScope, Observed, ResourceObserved } from '@coldmind/inject/internal/host';
import { stamp } from '@coldmind/inject/internal/async';

/** Diagnostic logs keep the newest entries. Trimmed in batches, so each append is amortized O(1). */
const OBSERVATION_LIMIT = 10_000;

/** Retains raw diagnostic keys only when they leave the container. */
export function exposeIssues<T extends DiIssue[]>(issues: T): T {
  for (const issue of issues) {
    pinTokenKey(issue.at);
    if (issue.edge) pinTokenKey(issue.edge.key);
    for (const key of issue.path ?? []) pinTokenKey(key);
  }
  return issues;
}
/** Appends to a bounded diagnostic log, trimming it back to `OBSERVATION_LIMIT` entries. */
function record<T>(log: T[], entry: T): boolean {
  log.push(entry);
  if (log.length <= OBSERVATION_LIMIT * 2) return false;
  log.splice(0, log.length - OBSERVATION_LIMIT);
  return true;
}
/** Diagnostic indexing, bounded history and name/resource DTO projections; no registry mutation authority. */
export class Observation {
  /** Stable read/validator capabilities bound by the owning container. */
  constructor(private readonly host: ObserveHost) {}

  /** Bounded log of dependency edges observed during resolution. */
  readonly observed: Observed[] = [];
  /** Bounded log of instance-to-instance edges observed during resolution. */
  readonly resourceObserved: ResourceObserved[] = [];
  /** Edges already in `observed`, so each is logged once. Forgotten whenever the log is trimmed or edited. */
  private observedPairs?: WeakMap<Binding, Set<Binding>>;
  /** Edges already in `resourceObserved`, so each is logged once. Forgotten with `observedPairs`. */
  private resourcePairs?: WeakMap<StoredRecord, Set<StoredRecord>>;
  /** Graph issues recorded when registrations changed. */
  readonly graphIssues: DiIssue[] = [];
  /** Retained normalized keys parallel to the bounded historical issue log. */
  readonly graphIssueKeys: symbol[][] = [];
  /** Backing field of `diGraph`. */
  private graphState?: DiGraph;
  /** Bindings registered since the graph was last read. The graph indexes them on its next read, so registering stays cheap. */
  private graphQueue?: Binding[];
  /** How many bindings of `order` the graph has indexed. New registrations past it are indexed on the graph's next read. */
  private graphIndexed = 0;

  /** Declared-dependency graph for this container, layered over the parent's. Created on first use: most scopes register nothing. */
  private get diGraph(): DiGraph {
    // Reading the parent first indexes its queued bindings too, so lookups through the parent see them.
    const parentGraph = this.host.parentGraph();
    const graph = this.graphState ??= new DiGraph({ parent: parentGraph, describe: tokenName, onIssue: i => { this.recordIssue(i); }, onExpose: pinTokenKey });
    const queue = this.graphQueue, order = this.host.order;
    if (queue) {
      this.graphQueue = undefined;
      for (const binding of queue) graph.add({ key: binding.key, identity: binding.graphId, token: binding.token, lifetime: binding.lifetime, deps: binding.deps, declared: binding.declared, conditional: !!binding.when, multi: binding.kind === 'multi', module: binding.module });
    }
    for (let i = this.graphIndexed; i < order.length; i++) {
      const binding = order[i]!;
      graph.add({ key: binding.key, identity: binding.graphId, token: binding.token, lifetime: binding.lifetime, deps: binding.deps, declared: binding.declared, conditional: !!binding.when, multi: binding.kind === 'multi', module: binding.module });
    }
    this.graphIndexed = order.length;
    return graph;
  }
  /** Internal indexing does not expose/pin every node key. */
  indexedGraph(): DiGraph { return this.diGraph; }
  /** Registration removal has already indexed its pre-mutation order. */
  finishIndex(): void { this.graphIndexed = this.host.order.length; }
  /** Releases graph/history state without rebuilding an unused graph during owner teardown. */
  disposeGraph(): void {
    this.graphQueue = undefined; this.graphIndexed = 0;
    if (this.graphState) for (const key of this.host.registry.keys()) this.graphState.remove(key, true);
    this.clearGraphIssues();
  }
  /** Instance clearing drops runtime observations but not historical declared-graph diagnostics. */
  clearRuntime(): void { this.observed.length = 0; this.resourceObserved.length = 0; this.forgetObserved(); }

  /** Queues a binding for the declared-dependency graph. It is indexed, and its issues recorded, on the graph's next read. */
  private addGraphNode(binding: Binding): void { (this.graphQueue ??= []).push(binding); }
  /** Re-indexes every binding for a token in the dependency graph. */
  rebuildGraphKey(key: symbol): void {
    this.diGraph.remove(key, !!this.host.registry.get(key)?.single || !!this.host.registry.get(key)?.multi.length);
    for (const entry of this.host.registry.get(key)?.multi ?? []) this.addGraphNode(entry);
    for (const single of this.host.order.filter(binding => binding.key === key && binding.kind === 'single')) this.addGraphNode(single);
  }
  /** Logs the edge from the resolving binding to this one, once per pair, for dependency snapshots. */
  observe(source: Binding, binding: Binding): void {
    if (source.observedLast(binding, this.observedPairs)) return;
    const pairs = this.observedPairs ??= new WeakMap();
    let seen = pairs.get(source);
    if (!seen) pairs.set(source, seen = new Set());
    source.noteObserved(binding, pairs);
    if (seen.has(binding)) return;
    seen.add(binding);
    const trimmed = record(this.observed, { from: source.id, to: binding.id, fromGeneration: source.generation, toGeneration: binding.generation,
      type: source.deps.some(edge => edge.key === binding.key) ? 'declared' : 'dynamic',
      scopeId: this.host.identity.id, observedAt: stamp() });
    if (trimmed) this.forgetObserved();
  }
  /** Logs the edge between two stored instances, once per pair. Transient instances are never stored, so they are skipped. */
  observeResource(from: StoredRecord, to: StoredRecord): void {
    if (from.binding.lifetime === ServiceLifetime.Transient || to.binding.lifetime === ServiceLifetime.Transient) return;
    const pairs = this.resourcePairs ??= new WeakMap();
    let seen = pairs.get(from);
    if (!seen) pairs.set(from, seen = new Set());
    if (seen.has(to)) return;
    seen.add(to);
    if (record(this.resourceObserved, { from: idOf(from), to: idOf(to), scopeId: this.host.identity.id, observedAt: stamp() })) this.forgetObserved();
  }
  /** Forgets which edges are logged, after a log was trimmed or edited, so live edges are logged again. */
  forgetObserved(): void { this.observedPairs = undefined; this.resourcePairs = undefined; }
  /** Classes visible from this container whose methods or tags match `key`, root first. */
  discover(key: string): DiscoveredClass[] {
    this.host.active();
    return this.host.ancestors().flatMap(c => c.order.flatMap(b => {
      if (!b.ctor || !b.descriptor) return [];
      const methods = b.descriptor.methods.filter(m => m.decorator === key);
      if (!methods.length && !b.descriptor.tags.includes(key)) return [];
      return [{ binding: this.toDescriptor(b), ctor: b.ctor, methods, tags: [...b.descriptor.tags] }];
    }));
  }
  /** This scope and its ancestors, root first, with depth and parent links. */
  getScopeChain(): ScopeTreeNode[] {
    return this.host.ancestors().map((container, depth, arr) => ({ scope: { ...container.identity }, depth,
      ...(depth ? { parent: { ...arr[depth - 1]!.identity } } : {}) }));
  }
  /** Public descriptor for a binding. */
  private toDescriptor(binding: Binding): BindingDescriptor {
    return {
      id: binding.id, generation: binding.generation, ownerScopeId: binding.ownerScopeId,
      token: binding.token, module: binding.module, lifetime: binding.lifetime, kind: binding.kind, kindTag: binding.kindTag,
      owner: { ...binding.owner }, registeredAt: binding.registeredAt, updatedAt: binding.updatedAt,
    };
  }
  /** Descriptor of the single binding the token resolves to, or `undefined`. */
  bindingOf(token: InjectionToken): BindingDescriptor | undefined {
    this.host.active(); const binding = this.host.selectSingle(lookupToken(token) ?? Symbol(tokenName(token)));
    return binding && this.toDescriptor(binding);
  }
  /** Descriptors of every multi binding for a token, in resolution order. */
  multiBindingsOf(token: InjectionToken): MultiBindingEntry[] {
    this.host.active(); return this.host.selectMulti(lookupToken(token) ?? Symbol(tokenName(token))).map((binding, order) => ({ ...this.toDescriptor(binding), order }));
  }
  /** Lifecycle state of the token's instance: the nearest local record, then any descendant's. */
  instanceState(token: InjectionToken): InstanceState {
    this.host.active();
    const key = lookupToken(token) ?? Symbol(tokenName(token));
    if (!this.host.isRegisteredRecursive(token)) return 'uninitialized';
    for (let c: ObserveScope | undefined = this.host; c; c = c.parent) {
      const local = [...c.records.values()].find(r => r.key === key);
      if (local) return local.state;
    }
    const children = this.host.descendants().flatMap(c => [...c.records.values()].filter(r => r.key === key));
    if (children.some(r => r.state === 'ready')) return 'ready';
    return children[0]?.state ?? 'uninitialized';
  }
  /** Built instances owned by this container, with their descriptors. */
  cachedBindings(): CacheableBinding[] {
    this.host.active();
    return [...this.host.records.values()].filter(r => r.binding.ownerContainer === this.host.owner)
      .map(r => ({ ...this.toDescriptor(r.binding), record: this.recordPublic(r) }));
  }
  /** Public view of an internal instance record. */
  private recordPublic(record: StoredRecord): InstanceRecord {
    return { bindingId: record.bindingId, generation: record.generation, state: record.state,
      instance: record.instance, createdAt: record.createdAt, lastTouchedAt: record.lastTouchedAt };
  }
  /** Ready instances held by this container. */
  ownershipRecords(): OwnershipRecord[] {
    return [...this.host.records.values()].filter(r => r.state === 'ready').map(r => ({
      scopeId: this.host.identity.id, bindingId: r.bindingId, generation: r.generation,
    }));
  }
  /** Dependency edges observed at runtime across this container and its view, without duplicates. */
  dependencyEdges(): RuntimeEdge[] {
    const seen = new Set<string>();
    const edges: RuntimeEdge[] = [];
    for (const container of this.host.view()) for (const entry of container.observed) {
      const k = `${entry.from}/${entry.fromGeneration}:${entry.to}/${entry.toGeneration}`;
      if (seen.has(k)) continue;
      seen.add(k);
      edges.push({ from: { bindingId: entry.from, generation: entry.fromGeneration },
        to: { bindingId: entry.to, generation: entry.toGeneration }, type: entry.type });
    }
    return edges;
  }
  /** Nodes and edges describing the bindings and their observed dependencies. */
  getDependencyGraphSnapshot(): RuntimeDependencyGraph {
    const scopes = this.host.view();
    const bindings = scopes.flatMap(c => c.order);
    const nodes: RuntimeDependencyNode[] = bindings.map(b => ({
      bindingId: b.id, token: tokenName(b.token), module: b.module, ownerScopeId: b.ownerScopeId,
      scopeId: b.ownerScopeId, kind: b.kind, kindTag: b.kindTag,
      generation: b.generation, registeredAt: b.registeredAt, updatedAt: b.updatedAt,
    }));
    const edges: RuntimeDependencyEdge[] = [];
    const unique = new Set<string>();
    const emit = (edge: RuntimeDependencyEdge) => {
      const key = `${edge.from}:${edge.to}:${edge.type}:${edge.scopeId}`;
      if (!unique.has(key)) { unique.add(key); edges.push(edge); }
    };
    for (const b of bindings) for (const dep of b.deps) {
      const matches = dep.multi ? this.host.ownerOf(b).ancestors().flatMap(c => c.registry.get(dep.key)?.multi ?? [])
        : this.host.ownerOf(b).ancestors().slice().reverse().find(scope => scope.order.some(binding => binding.key === dep.key && binding.kind === 'single'))?.order.filter(binding => binding.key === dep.key && binding.kind === 'single') ?? [];
      for (const target of matches) emit({ from: b.id, to: target.id, type: 'declared', scopeId: b.ownerScopeId });
    }
    for (const c of scopes) for (const observed of c.observed) {
      if (bindings.some(b => b.id === observed.from && b.generation === observed.fromGeneration) && bindings.some(b => b.id === observed.to && b.generation === observed.toGeneration))
        emit({ from: observed.from, to: observed.to, type: 'runtime', scopeId: observed.scopeId });
    }
    return { nodes, edges };
  }
  /** Dependency edges as `from`/`to` binding-id pairs. */
  getDependencyGraph(): { from: string; to: string }[] {
    return this.getDependencyGraphSnapshot().edges.map(e => ({ from: e.from, to: e.to }));
  }
  /** Dependency tree rooted at the token's binding. */
  getDependencyTree(token: InjectionToken): RuntimeDependencyTreeNode[] {
    const key = lookupToken(token) ?? Symbol(tokenName(token));
    const snapshot = this.getDependencyGraphSnapshot();
    const starts = [...this.host.view().flatMap(c => c.order)].filter(b => b.key === key);
    const build = (bindingId: string, walked: Set<string>): RuntimeDependencyTreeNode => {
      const item = snapshot.nodes.find(n => n.bindingId === bindingId)!;
      return {
        bindingId, token: item.token, scopeId: item.scopeId,
        children: walked.has(bindingId) ? [] : snapshot.edges.filter(e => e.from === bindingId)
          .map(e => build(e.to, new Set([...walked, bindingId]))),
      };
    };
    return starts.map(b => build(b.id, new Set()));
  }
  /** Scopes, bindings, live instances and the edges between them. */
  getResourceGraphSnapshot(): RuntimeResourceGraph {
    this.host.active();
    const view = this.host.view();
    const bindings = this.getDependencyGraphSnapshot();
    const resources: RuntimeResourceNode[] = view.flatMap(c => [...c.records.values()]
      .filter(r => r.state === 'ready' && r.binding.lifetime !== ServiceLifetime.Transient)
      .map((r): RuntimeResourceNode => ({ instanceId: idOf(r), bindingId: r.bindingId, generation: r.generation,
        token: tokenName(r.binding.token), lifetime: lifetimeText[r.binding.lifetime],
        ownerScopeId: r.binding.ownerScopeId, resolutionScopeId: c.identity.id,
        tenantId: c.identity.tenantId, requestId: c.identity.requestId,
        ownership: r.binding.owner.ownership, state: r.state, createdAt: r.createdAt, lastTouchedAt: r.lastTouchedAt })));
    const live = new Set(resources.map(r => r.instanceId));
    const resourceEdges: RuntimeResourceEdge[] = view.flatMap(c => c.resourceObserved.filter(e => live.has(e.from) && live.has(e.to))
      .map(e => ({ ...e, type: 'runtime' as const })));
    return { epoch: view.reduce((sum, c) => sum + c.epoch, 0), scopes: view.map(c => ({ ...c.identity })),
      bindings: bindings.nodes, bindingEdges: bindings.edges, resources, resourceEdges };
  }
  /** The declared-dependency graph for this container. */
  graph(): DiGraph { this.host.active(); return this.diGraph; }
  /** Stores bounded historical diagnostics without pinning internal keys permanently. */
  private recordIssue(issue: DiIssue): void {
    const keys = new Set([issue.at, ...(issue.edge ? [issue.edge.key] : []), ...(issue.path ?? [])]);
    this.graphIssueKeys.push([...keys].filter(key => retainTokenKey(key)));
    const length = this.graphIssues.length + 1;
    if (record(this.graphIssues, issue)) {
      for (const dropped of this.graphIssueKeys.splice(0, length - this.graphIssues.length)) for (const key of dropped) releaseTokenKey(key);
    }
  }
  /** Releases every historical diagnostic key when its owning scope is disposed. */
  private clearGraphIssues(): void {
    for (const keys of this.graphIssueKeys) for (const key of keys) releaseTokenKey(key);
    this.graphIssueKeys.length = 0; this.graphIssues.length = 0;
  }
  /** Restores rollback history before releasing transaction-owned snapshots. */
  restoreGraphIssues(issues: readonly DiIssue[], issueKeys: readonly symbol[][]): void {
    const retained = issueKeys.map(keys => keys.map(key => { retainTokenKey(key); return key; }));
    this.clearGraphIssues();
    this.graphIssues.push(...issues); this.graphIssueKeys.push(...retained);
  }
  /** Graph issues recorded at mutation time for this container. */
  issuesObserved(): DiIssue[] { void this.diGraph; return exposeIssues([...this.graphIssues]); }
  /** Current-generation dynamic edges absent from authored dependency lists. */
  drift(): DiIssue[] {
    const bindings = this.host.view().flatMap(scope => scope.order);
    return this.dependencyEdges().filter(edge => edge.type === 'dynamic').flatMap(edge => {
      const from = bindings.find(binding => binding.id === edge.from.bindingId && binding.generation === edge.from.generation);
      const to = bindings.find(binding => binding.id === edge.to.bindingId && binding.generation === edge.to.generation);
      return from && to && !from.deps.some(dep => dep.key === to.key) ? [{ kind: 'undeclared-edge' as const, at: from.key, message: `${tokenName(from.token)}${from.module ? ` [${from.module}]` : ''} resolves undeclared ${tokenName(to.token)}`, edge: { token: to.token, key: to.key, index: from.deps.length, optional: false, multi: false } }] : [];
    });
  }
  /** Checks the declared-dependency graph and throws `DiGraphError` if it is not sound. */
  validate(options: ValidationOptions = {}): void {
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
    }
    for (const binding of this.host.order) {
      if (!binding.ctor || !binding.descriptor) continue;
      for (const validator of binding.descriptor.validators) {
        const messages = this.host.validator(binding, validator);
        for (const message of typeof messages === 'string' ? [messages] : messages ?? []) {
          issues.push({ kind: 'decorator', at: binding.key, message: `${tokenName(binding.token)}: ${validator.decorator}: ${message}` });
        }
      }
    }
    if (issues.length) throw new DiGraphError(exposeIssues(issues));
  }
}
