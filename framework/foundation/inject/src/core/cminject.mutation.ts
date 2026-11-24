import { unwrapDependencySlot, isDeferredToken } from '@coldmind/inject/lazy';
import { chain, attachCause } from '@coldmind/inject/internal/async';
import {
  ScopeType, ServiceLifetime, type IContainer, type BindingHandle, type RegisterOptions, type ReplacementOptions,
  type ServiceFactory, type ServiceLifecycle, type ParameterInjection, type InjectedSpec,
} from '@coldmind/inject/contracts';
import {
  DelayedConstructor, type Constructor, type InjectionToken, internToken, lookupToken, tokenName, retainTokenKey,
  releaseTokenKey,
} from '@coldmind/inject/token';
import {
  TypedBinding, createBinding, isConstructor, KIND_TAGS, lifetimeText, type Binding, type BindingOptions,
  type StoredRecord, type AnyFactory,
} from '@coldmind/inject/internal/binding';
import type { MutationHost, MutationScope, Resolver } from '@coldmind/inject/internal/host';
import { rootResolver } from '@coldmind/inject/internal/resolution';
import { classEdges } from '@coldmind/inject/internal/construction';
import { describe } from '@coldmind/inject/decorators';
import { MutationConflictError, NotRegisteredError, ScopeMissingError } from '@coldmind/inject/errors';

/** Definition installation and transactional replacement share one preflight/commit owner. */
export class Mutation {
  /** Stable exact-identity capabilities, not a mutable Container implementation. */
  constructor(private readonly host: MutationHost) {}

  /** Installs a binding in the registry and graph, replacing `replacing` when given. */
  install(binding: Binding, replacing?: Binding): void {
    const entry = this.host.registry.get(binding.key);
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
    this.host.epoch++;
    // A new registration is indexed from `order` on the graph's next read; a replacement re-indexes the token.
    if (replacing) this.host.rebuildGraphKey(binding.key);
  }
  /** Shared registration path for every lifetime and for single and multi kinds. */
  register<T, D>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D> | InjectedSpec<T> | RegisterOptions | undefined,
    lifetime: ServiceLifetime, kind: 'single' | 'multi', options?: RegisterOptions,
    internal?: BindingOptions): BindingHandle<T, D> {
    this.host.active();
    if (factory && typeof factory === 'object' && 'inject' in factory) {
      const definition = factory;
      const adapted: ServiceLifecycle<T, D> = {
        factory: (container: IContainer, _data?: unknown) => chain(this.host.factoryArgs(container, definition.inject), args => Reflect.apply(definition.factory, undefined, args) as T),
        onInit: definition.onInit, onDispose: definition.onDispose, onHealthCheck: definition.onHealthCheck,
      };
      return this.register(token, adapted, lifetime, kind, { ...options, deps: definition.inject }, internal);
    }
    // Common case: a plain factory for a new single token. Same result as the general path, without its option handling.
    if (kind === 'single' && options === undefined && internal === undefined && typeof factory === 'function') {
      const key = internToken(token);
      const held = retainTokenKey(key);
      try {
        if (this.host.registry.get(key) === undefined) {
          // Registry storage erases D; the public typed token/factory pair established this data contract.
          const binding = new TypedBinding<T, D>(this.host.owner, token, key, lifetime, kind, factory as AnyFactory, undefined);
          this.host.registry.set(key, binding);
          this.host.order.push(binding);
          this.host.epoch++;
          return binding;
        }
      } finally { if (held) releaseTokenKey(key); }
    }
    // No factory, or options in its place: the token is a class that implements itself.
    if (factory === undefined || (typeof factory === 'object' && !('factory' in factory))) {
      return this.registerConstructor<T, D>(token, factory, lifetime, kind);
    }
    const tag = KIND_TAGS[lifetime][kind === 'multi' ? 1 : 0];
    const binding = createBinding(this.host.owner, token, factory, lifetime, kind, tag, options, internal);
    this.install(binding);
    return this.host.handle(binding);
  }
  /** Registers a class under itself (or under a delayed reference to it) with an explicit lifetime. */
  private registerConstructor<T, D = void>(token: InjectionToken<T>, options: RegisterOptions | undefined, lifetime: ServiceLifetime, kind: 'single' | 'multi', tag?: string): TypedBinding<T, D> {
    const ctor = token instanceof DelayedConstructor ? token.get() : token;
    if (!isConstructor(ctor)) throw new TypeError(`${tokenName(token)} needs a factory: only a class can be registered without one`);
    const descriptor = describe(ctor);
    const params: ParameterInjection[] = options?.deps
      ? options.deps.map((dep, index) => ({ index, token: dep, optional: false, multi: false }))
      : descriptor.params;
    const binding = createBinding<T, D>(this.host.owner, token, this.host.classFactory<T, D>(ctor, descriptor, params), lifetime, kind,
      tag ?? KIND_TAGS[lifetime][kind === 'multi' ? 1 : 0], options,
      { deps: classEdges(descriptor, params), declared: true, ctor, descriptor });
    this.install(binding);
    return binding;
  }
  /** The binding `autoInject` registers for an unregistered class token, or `undefined` when auto-injection does not apply. */
  autoBinding(token: InjectionToken, key: symbol): Binding | undefined {
    const setting = this.host.autoInject;
    if (!setting) return undefined;
    const ctor = token instanceof DelayedConstructor ? token.get() : token;
    if (typeof ctor !== 'function' || this.host.selectMulti(key).length) return undefined;
    if (!isConstructor(ctor)) return undefined;
    const lifetime = describe(ctor).registration?.lifetime
      ?? (setting === true ? undefined : setting.lifetime) ?? ServiceLifetime.Transient;
    this.registerConstructor(ctor, undefined, lifetime, 'single', 'auto');
    return this.host.localSingle(key);
  }

  /** Ids of the bindings that depend on `binding`, transitively. */
  private dependentBindings(binding: Binding): Set<string> {
    const containers = this.host.subtree();
    const all = containers.flatMap(c => c.order);
    const edges = containers.flatMap(c => c.observed);
    const found = new Set<string>();
    const nextKeys = new Set<symbol>([binding.key]);
    const nextIds = new Set<string>([binding.id]);
    let changed = true;
    while (changed) {
      changed = false;
      for (const candidate of all) {
        if (candidate.id === binding.id || found.has(candidate.id)) continue;
        const declared = candidate.deps.some(edge => nextKeys.has(edge.key));
        const dynamic = edges.some(edge => edge.from === candidate.id && nextIds.has(edge.to));
        if (!declared && !dynamic) continue;
        found.add(candidate.id); nextKeys.add(candidate.key); nextIds.add(candidate.id);
        changed = true;
      }
    }
    return found;
  }
  /** Waits for in-flight builds of a binding in this subtree, ignoring their failures. */
  private async awaitBuilds(binding: Binding): Promise<void> {
    const builds = this.host.subtree().flatMap(c => {
      const pending = c.pending.get(binding);
      return pending ? [pending.catch(() => undefined)] : [];
    });
    await Promise.all(builds);
  }
  /** Ready records in this subtree for the given binding ids. */
  private recordsFor(ids: ReadonlySet<string>): StoredRecord[] {
    return this.host.subtree().flatMap(c => [...c.records.values()].filter(r => ids.has(r.bindingId) && r.state === 'ready'));
  }
  /** Removes a record and its ordering entries, without disposing the instance. */
  private discardRecord(record: StoredRecord): void {
    const container = this.host.recordScope(record);
    container.records.delete(record.binding);
    container.pending.delete(record.binding);
    const index = container.disposalOrder.indexOf(record.binding);
    if (index >= 0) container.disposalOrder.splice(index, 1);
    for (const c of this.host.view()) {
      for (let i = c.resourceObserved.length - 1; i >= 0; i--) {
        if (c.resourceObserved[i]!.from === record.instanceId || c.resourceObserved[i]!.to === record.instanceId) { c.resourceObserved.splice(i, 1); c.forgetObserved(); }
      }
    }
  }
  /** Transactionally replace a binding and optionally rebuild its live dependents. */
  async replaceBinding<T, D = void>(bindingId: string,
    factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>, lifetime?: ServiceLifetime,
    options: ReplacementOptions = {}): Promise<BindingHandle<T, D>> {
    return this.host.transaction(staged => this.replaceBindingIn(bindingId, factory, lifetime, options, staged));
  }
  private async replaceBindingIn<T, D>(bindingId: string,
    factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>, lifetime: ServiceLifetime | undefined,
    options: ReplacementOptions, staged: Map<symbol, Binding>): Promise<BindingHandle<T, D>> {
    this.host.active(rootResolver(staged));
    this.host.guardMutation(bindingId);
    const old = this.host.order.find(b => b.id === bindingId);
    if (!old) throw new NotRegisteredError(`binding ${bindingId}`, `binding ${bindingId}`);
    const desiredLifetime = lifetime ?? old.lifetime;
    const scopes = this.host.subtree();
    if (options.rejectIfLive && scopes.some(c => c.pending.has(old) || ['ready', 'creating', 'initializing'].includes(c.records.get(old)?.state ?? ''))) {
      throw new MutationConflictError(`Cannot replace a live registration: ${tokenName(old.token)}`);
    }
    await this.awaitBuilds(old);
    if (this.host.ended) throw this.host.lifetimeReason;
    const ids = this.dependentBindings(old);
    const oldLive = this.recordsFor(new Set([old.id]));
    // Binding ids select an erased registration; the public handle preserves its original T/D pair.
    const candidate = createBinding(this.host.owner, old.token as InjectionToken<T>, factory, desiredLifetime, old.kind,
      options.retainPrevious ? 'replace-retained' : 'replace', undefined,
      { deps: options.deps ? options.deps.map((token, index) => ({ token: unwrapDependencySlot(token), lazy: isDeferredToken(token), index, optional: false, multi: false })) : [...old.deps],
        declared: options.deps ? true : old.declared, generation: old.generation + 1, owner: old.owner, timeoutMs: old.timeoutMs, module: old.module, when: old.when });
    let committed = false;
    const retainedIssueSnapshots: symbol[][][] = [];
    try {
      candidate.replacesId = old.id;
      staged.set(old.key, candidate);
      const stageTargets: Array<{ scope: MutationScope; data: unknown }> = [];
      if (oldLive.length) {
        if (desiredLifetime === ServiceLifetime.Singleton || desiredLifetime === ServiceLifetime.Transient) {
          stageTargets.push({ scope: this.host, data: oldLive[0]!.data });
        } else {
          for (const record of oldLive) stageTargets.push({ scope: this.host.recordScope(record), data: record.data });
        }
      } else if (options.rebuild) {
        if (desiredLifetime === ServiceLifetime.Tenant || desiredLifetime === ServiceLifetime.Request) {
          const kind = desiredLifetime === ServiceLifetime.Tenant ? ScopeType.Tenant : ScopeType.Request;
          const suitable = scopes.filter(c => c.scopeType === kind);
          if (suitable.length) for (const scope of suitable) stageTargets.push({ scope, data: undefined });
          else {
            const ancestor = this.host.nearestScope(kind);
            if (!ancestor) throw new ScopeMissingError(kind === ScopeType.Tenant ? 'tenant' : 'request', tokenName(old.token));
            stageTargets.push({ scope: ancestor, data: undefined });
          }
        } else stageTargets.push({ scope: this.host, data: undefined });
      }
      // Include suitable ancestor scopes too when a candidate changes to Tenant/Request.
      const touchedScopes = [...new Set([...scopes, ...stageTargets.map(item => item.scope.route(candidate))])];
      // Index queued bindings first, so the issue count below is the one to roll back to.
      for (const c of touchedScopes) c.indexGraph();
      const snapshot = touchedScopes.map(c => {
        const issueKeys = c.graphIssueKeys.map(keys => keys.map(key => { retainTokenKey(key); return key; }));
        retainedIssueSnapshots.push(issueKeys);
        return { c, records: new Map(c.records), pending: new Map(c.pending),
          disposalOrder: [...c.disposalOrder], observed: [...c.observed], resourceObserved: [...c.resourceObserved],
          epoch: c.epoch, issues: [...c.graphIssues], issueKeys };
      });
      // Staging lives in candidate cache slots; rollback restores the pre-stage snapshot.
      let installed = false;
      const rollback = async (error: unknown): Promise<never> => {
        const newRecords = touchedScopes.flatMap(c => [...c.records.values()].filter(r =>
          !snapshot.find(s => s.c === c)?.records.has(r.binding) && r.state === 'ready'));
        for (const record of [...newRecords].reverse()) await this.host.disposeRecord(record, true);
        if (installed) {
          if (!this.host.ended) this.install(old, candidate);
          else old.releaseTokens();
        }
        for (const snap of [...snapshot].reverse()) {
          // Disposal cannot see originals detached before the transaction failed. If it won
          // during awaited candidate cleanup, claim those originals in reverse build order
          // instead of resurrecting them. Existing cleanup claims and external ownership win.
          if (snap.c.ended) {
            const listed = new Set(snap.disposalOrder);
            const originals = snap.disposalOrder.slice().reverse().flatMap(binding => {
              const record = snap.records.get(binding); return record ? [record] : [];
            });
            originals.push(...[...snap.records.values()].filter(record => !listed.has(record.binding)).reverse());
            for (const record of originals) if (record.state === 'ready' && !record.cleanupStarted) {
              const failure = await this.host.disposeRecord(record, false);
              if (failure?.error) attachCause(error, failure.error);
            }
            continue;
          }
          snap.c.records.clear(); for (const [id, rec] of snap.records) snap.c.records.set(id, rec);
          snap.c.pending.clear(); for (const [id, p] of snap.pending) snap.c.pending.set(id, p);
          snap.c.disposalOrder.splice(0, snap.c.disposalOrder.length, ...snap.disposalOrder);
          snap.c.observed.splice(0, snap.c.observed.length, ...snap.observed);
          snap.c.resourceObserved.splice(0, snap.c.resourceObserved.length, ...snap.resourceObserved);
          snap.c.forgetObserved();
          snap.c.epoch = snap.epoch; snap.c.restoreGraphIssues(snap.issues, snap.issueKeys);
        }
        throw error;
      };
      try {
        for (const { scope, data } of stageTargets) await scope.entry(candidate, data, rootResolver(staged));
        await this.awaitBuilds(old);
        if (this.host.ended) throw this.host.lifetimeReason;
        const targetLive = this.recordsFor(new Set([old.id]));
        for (const record of targetLive) this.discardRecord(record);
        this.install(candidate, old); installed = true;
        const dependentsLive = this.recordsFor(ids);
        for (const record of dependentsLive) this.discardRecord(record);
        if (options.rebuild) {
          const rebuilt = [...dependentsLive].sort((a, b) => a.createdAt - b.createdAt);
          for (const oldRecord of rebuilt) await this.host.recordScope(oldRecord).entry(oldRecord.binding, oldRecord.data, rootResolver(staged));
        }
        for (const record of [...dependentsLive].sort((a, b) => b.createdAt - a.createdAt)) await this.host.disposeRecord(record, true);
        if (!options.retainPrevious) for (const record of targetLive.reverse()) await this.host.disposeRecord(record, true);
        // Remap historical observations from the previous binding generation.
        for (const scope of touchedScopes) for (const obs of scope.observed) {
          if (obs.from === old.id) { obs.from = candidate.id; obs.fromGeneration = candidate.generation; }
          if (obs.to === old.id) { obs.to = candidate.id; obs.toGeneration = candidate.generation; }
        }
        committed = true;
        // Retained records belong to this same replacement token and therefore have the handle's T.
        const handle = this.host.handle<T, D>(candidate, options.retainPrevious ? targetLive.map(r => r.instance as T) : []);
        this.host.remapBinding(old.id, handle);
        return handle;
      } catch (error) { return await rollback(error); }
    } finally {
      for (const snapshot of retainedIssueSnapshots) for (const keys of snapshot) for (const key of keys) releaseTokenKey(key);
      if (committed) old.releaseTokens(); else candidate.releaseTokens();
    }
  }

  /** Replaces the single registration for a token. Staged builds run before the swap; a failure rolls back. */
  async replace<T, D = void>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>,
    lifetime: ServiceLifetime, options: ReplacementOptions = {}): Promise<BindingHandle<T, D>> {
    // Route before acquiring the hierarchy transaction; parent delegation must not acquire it twice.
    const key = lookupToken(token);
    if (key !== undefined && !this.host.registry.get(key)) for (let parent = this.host.parent; parent; parent = parent.parent) {
      if (parent.registry.get(key)?.single) return parent.replace(token, factory, lifetime, options);
    }
    return this.host.transaction(staged => this.replaceIn(token, factory, lifetime, options, staged));
  }
  private async replaceIn<T, D>(token: InjectionToken<T>, factory: ServiceFactory<T, D> | ServiceLifecycle<T, D>,
    lifetime: ServiceLifetime, options: ReplacementOptions, staged: Map<symbol, Binding>): Promise<BindingHandle<T, D>> {
    this.host.active(rootResolver(staged));
    const key = lookupToken(token) ?? Symbol(tokenName(token));
    const group = this.host.registry.get(key);
    if (!group?.single) {
      if (group?.multi.length) throw new MutationConflictError(`Cannot replace multi-registration with single registration for token: ${tokenName(token)}`);
      const candidate = createBinding(this.host.owner, token, factory, lifetime, 'single', 'replace',
        options.deps ? { deps: options.deps } : undefined);
      staged.set(candidate.key, candidate);
      this.install(candidate);
      try {
        if (options.rebuild) await this.host.entry(candidate, undefined, rootResolver(staged));
        return this.host.handle<T, D>(candidate);
      } catch (error) { await this.unregisterBinding(candidate.id, false, rootResolver(staged)); throw error; }
    }
    return this.replaceBindingIn(group.single.id, factory, lifetime, options, staged);
  }


  /** @internal Rolls back only registrations instantiated during a failed runtime start. */
  async rollbackNewInstances(preexisting: ReadonlySet<string>): Promise<void> {
    const newlyCreated = [...this.host.records.values()].filter(r => !preexisting.has(r.bindingId));
    for (const record of newlyCreated.reverse()) {
      this.discardRecord(record);
      if (record.state === 'ready') await this.host.disposeRecord(record, true);
    }
  }

  /** Removes the single registration for a token and disposes its live instances. */
  async unregister(token: InjectionToken): Promise<void> {
    this.host.active();
    const operation = this.host.beginOperation();
    try { await this.unregisterIn(token, { operation, suppressInterceptors: false }); }
    finally { operation.finish(); }
  }
  private async unregisterIn(token: InjectionToken, context: Resolver): Promise<void> {
    const key = lookupToken(token) ?? Symbol(tokenName(token));
    const local = this.host.registry.get(key);
    if (!local?.single && !local?.multi.length) {
      if (this.host.parent?.isRegisteredRecursive(token)) throw new MutationConflictError(`Unregister ${tokenName(token)} on the scope that registered it`);
      return;
    }
    for (const binding of [...('singles' in local && local.singles ? local.singles : local.single ? [local.single] : []), ...local.multi]) await this.unregisterBinding(binding.id, false, context);
  }
  /** Removes one binding by id. Returns `false` when no such binding exists. */
  async unregisterBinding(bindingId: string, restoring = false, context?: Resolver): Promise<boolean> {
    if (!restoring) { this.host.active(context); this.host.guardMutation(bindingId); }
    const operation = restoring || context?.staged || context?.operation ? undefined : this.host.beginOperation();
    try { return await this.unregisterBindingIn(bindingId); }
    finally { operation?.finish(); }
  }
  private async unregisterBindingIn(bindingId: string): Promise<boolean> {
    const binding = this.host.order.find(b => b.id === bindingId);
    if (!binding) return false;
    await this.awaitBuilds(binding);
    const failures: unknown[] = [];
    for (const record of this.recordsFor(new Set([binding.id]))) {
      this.discardRecord(record); const result = await this.host.disposeRecord(record, true); if (result?.error) failures.push(result.error);
    }
    for (const scope of this.host.subtree()) { scope.records.delete(binding); scope.pending.delete(binding); }
    // Index what is pending first: the graph reads new bindings by their position in `order`.
    this.host.indexGraph();
    if (binding.kind === 'single') {
      const group = this.host.registry.get(binding.key);
      const candidates = group && 'singles' in group && group.singles ? group.singles.filter(item => item !== binding) : [];
      if (!candidates.length) this.host.registry.delete(binding.key);
      else this.host.registry.set(binding.key, candidates.length === 1 ? candidates[0]! : { single: candidates[0], singles: candidates, multi: [] });
    }
    else {
      const group = this.host.registry.get(binding.key);
      if (!group || 'factory' in group) return false;
      group.multi.splice(group.multi.indexOf(binding), 1);
      if (!group.multi.length) this.host.registry.delete(binding.key);
    }
    this.host.order.splice(this.host.order.indexOf(binding), 1);
    binding.releaseTokens();
    this.host.finishGraphIndex();
    this.host.epoch++;
    this.host.rebuildGraphKey(binding.key);
    for (const scope of this.host.subtree()) {
      for (let i = scope.observed.length - 1; i >= 0; i--) {
        if (scope.observed[i]!.from === binding.id || scope.observed[i]!.to === binding.id) { scope.observed.splice(i, 1); scope.forgetObserved(); }
      }
    }
    if (failures.length) throw new AggregateError(failures, 'Registration cleanup failed');
    return true;
  }

  /**
   * Registers an existing instance as a singleton. Without `ownerPolicy` the instance is reported as owned and the container
   * disposes it when it is disposed. `{ ownership: 'external' }` never disposes it. `clearInstances` keeps a supplied
   * instance either way: the container did not build it and cannot build it again.
   */
  registerInstance<T>(token: InjectionToken<T>, instance: T, ownerPolicy?: { ownership: 'owned' | 'external'; reason?: string }): BindingHandle<T> {
    this.host.active();
    const policy = ownerPolicy ?? { ownership: 'owned' as const, reason: 'supplied through registerInstance' };
    const binding = createBinding<T, void>(this.host.owner, token, () => instance, ServiceLifetime.Singleton, 'single', 'registerInstance',
      { deps: [] }, { owner: policy });
    this.install(binding);
    const record = this.host.makeRecord(binding, undefined);
    record.instance = instance; record.produced = true; record.building = false; record.state = 'ready';
    this.host.records.set(binding, record); this.host.disposalOrder.push(binding);
    return this.host.handle(binding);
  }

  /** Registers a decorated or `$injectable` class, taking its lifetime and dependencies from its descriptor. */
  registerClass<T>(ctor: Constructor<T>): BindingHandle<T> {
    this.host.active();
    const descriptor = describe(ctor);
    const registration = descriptor.registration;
    if (!registration) throw new Error(`Class ${ctor.name} has no registration. Decorate it with @Injectable() or a lifetime decorator.`);
    // A descriptor token is an authored alias for this registered implementation class.
    const token = (registration.token ?? ctor) as InjectionToken<T>;
    const lifetime = registration.lifetime ?? ServiceLifetime.Singleton;
    const binding = createBinding<T, void>(this.host.owner, token, this.host.classFactory<T, void>(ctor, descriptor, descriptor.params), lifetime, registration.multi ? 'multi' : 'single',
      lifetimeText[lifetime] + (registration.multi ? 'Multi' : ''), undefined,
      { deps: classEdges(descriptor, descriptor.params), declared: true, ctor, descriptor });
    this.install(binding);
    return this.host.handle(binding);
  }
}
